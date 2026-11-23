//! Removing a device of the account (docs/design/syncengine.md §2, build step 9).
//!
//! From any device of the account, a removal is asked for in the sync log: every device shows it at once, any of them
//! can take it back, and it takes effect a day later (a stolen device cannot remove the others quietly). With the
//! recovery phrase it takes effect at once. It is carried out by an install holding the account key: the list is signed
//! again with the device marked removed, the sync key is replaced (signed by the account, so a removed device cannot
//! hand out a key of its own) and given to the devices that remain, and a signed wipe is sent to the removed device.
//! Friends hear of the new list as of any other, and drop the keys and links they had with that device; a device's
//! links are closed once the list no longer has it.
//!
//! A device that holds the account key cannot be removed by a request from another device (it would be the one to
//! carry it out); that needs the phrase.

use super::{Core, StorageOp};
use crate::error::ErrorCode;
use privcord_crypto::{
    devices::DeviceList,
    identity::Identity,
    synclog::{account_ruling, kind},
};
use privcord_domain::UserId;
use serde_json::json;

/// When each of our other devices last got in touch (`{ device: ms }`).
const KV_DEVICE_SEEN: &str = "devices.seen";

/// How long a removal asked for from a device waits, so it can be seen and taken back (risks 45, 104).
pub const REMOVE_AFTER_MS: i64 = 24 * 60 * 60 * 1000;
/// A device not heard from for this long is shown with "Remove or reconnect" (risk 113).
pub const STALE_AFTER_MS: i64 = 90 * 24 * 60 * 60 * 1000;

impl Core {
    /// Asks for a device of the account to be removed. Every device shows it; it happens in 24 hours unless taken
    /// back. Not this device (erasing this device is the kill switch).
    pub fn request_device_removal(&mut self, device: &str) -> Result<(), ErrorCode> {
        self.removable(device)?;
        if self.pending_removals().iter().any(|(d, _)| d == device) {
            return Ok(());
        }
        let now = self.now();
        self.append_items(vec![(kind::REMOVE, json!({ "device": device, "at": now }))]);
        Ok(())
    }

    /// Takes back a removal asked for (from any device of the account).
    pub fn cancel_device_removal(&mut self, device: &str) -> Result<(), ErrorCode> {
        let Some((_, at)) = self
            .pending_removals()
            .into_iter()
            .find(|(d, _)| d == device)
        else {
            return Ok(());
        };
        self.append_items(vec![(
            kind::REMOVE_CANCEL,
            json!({ "device": device, "at": at }),
        )]);
        Ok(())
    }

    /// Removes a device at once, with the account's recovery phrase (works on any device of the account; the account
    /// key is used for this and not kept).
    pub fn remove_device_now(
        &mut self,
        device: &str,
        phrase: &str,
        passphrase: &str,
    ) -> Result<(), ErrorCode> {
        self.removable(device)?;
        let account = Identity::from_phrase(phrase, passphrase).map_err(|e| match e {
            privcord_crypto::CryptoError::InvalidPhrase => ErrorCode::InvalidPhrase,
            other => other.into(),
        })?;
        if account.uid().ok().map(|u| u.to_string()) != self.uid.as_ref().map(ToString::to_string) {
            return Err(ErrorCode::InvalidPhrase);
        }
        self.carry_out_removal(&account, device)
    }

    /// Removals asked for and not yet carried out: `(device, when asked)`.
    pub fn pending_removals(&self) -> Vec<(String, i64)> {
        let active = |d: &str| {
            self.device_list
                .as_ref()
                .and_then(|l| l.get(d))
                .is_some_and(privcord_crypto::devices::DeviceEntry::active)
        };
        self.sync
            .as_ref()
            .map(|s| s.replica().state().pending_removals())
            .unwrap_or_default()
            .into_iter()
            .filter(|(d, _)| active(d))
            .collect()
    }

    /// When a device of the account was last heard from (by what it wrote to the log), if ever.
    pub fn device_last_seen(&self, device: &str) -> Option<i64> {
        if self.device_id.as_deref() == Some(device) {
            return Some(self.now());
        }
        // What it wrote to the log, or when it last got in touch (a device that only reads writes nothing).
        let wrote = self
            .sync
            .as_ref()
            .and_then(|s| s.replica().last_heard(device));
        wrote.max(self.seen_map().get(device).copied())
    }

    fn seen_map(&self) -> std::collections::BTreeMap<String, i64> {
        self.ports
            .storage
            .kv(KV_DEVICE_SEEN)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// A device of ours got in touch (a sync message over a link or the relay): remember when, at most once a minute.
    pub(super) fn note_device_seen(&mut self, device: &str) {
        let now = self.now();
        let mut map = self.seen_map();
        if map.get(device).is_some_and(|t| now - t < 60_000) {
            return;
        }
        map.insert(device.to_owned(), now);
        if let Ok(value) = serde_json::to_vec(&map) {
            let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                key: KV_DEVICE_SEEN.into(),
                value,
            }]);
        }
    }

    fn removable(&self, device: &str) -> Result<(), ErrorCode> {
        let list = self.device_list.as_ref().ok_or(ErrorCode::NoIdentity)?;
        if self.device_id.as_deref() == Some(device)
            || !list
                .get(device)
                .is_some_and(privcord_crypto::devices::DeviceEntry::active)
        {
            return Err(ErrorCode::UnknownDevice);
        }
        Ok(())
    }

    /// Each tick: carry out removals that are due (an install holding the account key), erase this install if the
    /// account's list removed it, and close links to devices no list has any more.
    pub(super) fn tick_removals(&mut self) {
        if self.removed_here() {
            self.kill_switch_app();
            self.removed_from_account = false;
            return;
        }
        self.close_removed_links();
        let now = self.now();
        let own = self.device_id.clone();
        let due: Vec<String> = self
            .pending_removals()
            .into_iter()
            .filter(|(d, at)| now - at >= REMOVE_AFTER_MS && Some(d) != own.as_ref())
            .map(|(d, _)| d)
            .collect();
        if due.is_empty() {
            return;
        }
        let Some(account) = self.identity.as_ref().and_then(super::devices::copy_key) else {
            return;
        };
        for d in due {
            let _ = self.carry_out_removal(&account, &d);
        }
    }

    /// Whether the account's list (as this install knows it) has removed this device.
    fn removed_here(&self) -> bool {
        if self.removed_from_account {
            return true;
        }
        match (&self.device_list, &self.device_id) {
            (Some(l), Some(id)) => l.get(id).is_some_and(|d| !d.active()),
            _ => false,
        }
    }

    /// Links to a device (ours or a friend's) that its account's list no longer has are closed.
    fn close_removed_links(&mut self) {
        let gone: Vec<UserId> = self
            .peers
            .keys()
            .filter(|a| Self::device_of(a).is_some() && self.keys_of_address(a).is_none())
            .cloned()
            .collect();
        for a in gone {
            self.link_down(&a);
            self.ports.transport.close(&a);
            self.peers.remove(&a);
        }
    }

    /// The removal itself: a new list, a new sync key for the devices that remain, a wipe for the removed one.
    fn carry_out_removal(&mut self, account: &Identity, device: &str) -> Result<(), ErrorCode> {
        let list = self.device_list.clone().ok_or(ErrorCode::NoIdentity)?;
        let removed = list.get(device).cloned().ok_or(ErrorCode::UnknownDevice)?;
        let now = self.now();
        let next = list
            .with_removed(account, device, now)
            .map_err(|_| ErrorCode::UnknownDevice)?;
        self.adopt_own_list(&next);
        self.rotate_sync_key(account, &next)?;
        // The removed device erases itself when this reaches it (if it ever does: that cannot be guaranteed).
        self.send_to_device(&removed, &json!({ "t": "wipe", "list": next.to_json() }));
        self.drop_device_keys(&self.uid.clone().ok_or(ErrorCode::NoIdentity)?, &next);
        Ok(())
    }

    /// Stores a new list of our own (already checked) and tells the sync log which devices write to it now.
    pub(super) fn adopt_own_list(&mut self, next: &DeviceList) {
        let _ = self.ports.storage.apply(&[StorageOp::SetKv {
            key: super::devices::KV_DEVICE_LIST.into(),
            value: next.to_json().into_bytes(),
        }]);
        if let Some(s) = self.sync.as_mut() {
            s.devices_changed(next);
        }
        self.device_list = Some(next.clone());
    }

    /// A list of a person (ours or a friend's) removed devices: the keys exchanged with those devices' addresses are
    /// forgotten, so nothing is sealed to them again and nothing from them is opened.
    pub(super) fn drop_device_keys(&mut self, person: &UserId, list: &DeviceList) {
        let gone: Vec<UserId> = list
            .devices
            .iter()
            .filter(|d| !d.active())
            .filter_map(|d| UserId::new(format!("{person}.{}", d.id)).ok())
            .collect();
        self.forget_keys_of(&gone);
    }

    /// A new sync key, signed by the account, for the devices on `list`.
    fn rotate_sync_key(&mut self, account: &Identity, list: &DeviceList) -> Result<(), ErrorCode> {
        let version = self
            .sync
            .as_ref()
            .and_then(|s| s.sync_key())
            .map_or(1, |k| k.version + 1);
        let key = privcord_crypto::selfbox::SyncKey::random(version, &mut *self.ports.entropy)?;
        let sig = account_ruling(account, &super::sync::key_body(&key))?
            .get("accountSig")
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or(ErrorCode::Crypto)?;
        self.take_sync_key(key, Some(sig));
        let own = self.device_id.clone().unwrap_or_default();
        let message = self.sync_key_message(list);
        for d in list.active().filter(|d| d.id != own) {
            self.send_to_device(d, &message);
        }
        Ok(())
    }
}
