//! This install's own device keys and the account's device list (docs/design/syncengine.md §2).
//!
//! Every install gets device keys of its own the first time it runs with an identity, and a device list with itself
//! as the only device, signed by the identity (now the account). That is the whole migration: friends still know the
//! person by the same uid, nothing they hold changes, and a second device can later be added to the list.
//!
//! The keys are kept per account: an install that restores a different identity gets new device keys (a device is
//! never shared between accounts, and a relinked device always gets a fresh id).

use super::{Core, StorageOp};
use crate::error::ErrorCode;
use privcord_crypto::{
    card::ContactCard,
    devices::{AccountKeys, Author, DeviceList},
    identity::{Identity, StoredSecrets},
};
use privcord_domain::signaling::PutKind;
use privcord_domain::{blob::PublicJwk, UserId};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

pub(super) const KV_DEVICE_KEYS: &str = "devices.self";
pub(super) const KV_DEVICE_LIST: &str = "devices.list";
/// The name the first device gets until the person names it.
pub(super) const FIRST_NAME: &str = "This device";
/// What a device still carrying that name is shown as: it was never named, and "This device" is about the reader (the
/// row already says "(this device)" where it is).
pub(super) const FIRST_NAME_ELSEWHERE: &str = "Original device";
/// On a device linked by QR code: the account's public keys (`{ "sign", "dh" }`). Its private keys are not here.
pub(super) const KV_ACCOUNT_PUBLIC: &str = "account.public";
/// On a device linked by QR code: the account's contact card, as the account signed it when the device was linked.
pub(super) const KV_ACCOUNT_CARD: &str = "account.card";
/// A friend's device list, the newest seen (`devices.of.<uid>`).
const KV_CONTACT_LIST: &str = "devices.of";
/// Which version of our list each friend has been sent (`{ uid: version }`).
const KV_TOLD: &str = "devices.told";
/// Between an account and a device in an address (`uid.<device>`); neither a uid nor a device id has one.
const ADDR_SEP: char = '.';
/// Friends whose app understands device lists (`["uid", ...]`): we have seen the mark from them.
const KV_AWARE: &str = "devices.aware";
/// The mark: on every sealed payload and on the privacy frame of every link.
const AWARE_FIELD: &str = "dv";
/// How long a list notice waits on the relays for a friend who is offline.
const NOTICE_TTL_SEC: u32 = 7 * 24 * 3600;

impl Core {
    fn device_keys_key(&self) -> Option<String> {
        self.uid.as_ref().map(|u| format!("{KV_DEVICE_KEYS}.{u}"))
    }

    fn load_device_keys(&self, key: &str) -> Result<Option<Identity>, ErrorCode> {
        let Some(bytes) = self.ports.storage.kv(key)? else {
            return Ok(None);
        };
        let bytes = Zeroizing::new(bytes);
        let Ok(both) = <&[u8; 64]>::try_from(bytes.as_slice()) else {
            return Ok(None);
        };
        let (sign, dh) = both.split_at(32);
        let (Ok(sign), Ok(dh)) = (<[u8; 32]>::try_from(sign), <[u8; 32]>::try_from(dh)) else {
            return Ok(None);
        };
        Ok(Identity::from_stored(&StoredSecrets { sign, dh }).ok())
    }

    /// The account a linked device belongs to, from storage.
    pub(super) fn linked_account(&self) -> Option<(PublicJwk, PublicJwk)> {
        let v: serde_json::Value =
            serde_json::from_slice(&self.ports.storage.kv(KV_ACCOUNT_PUBLIC).ok()??).ok()?;
        let sign: PublicJwk = serde_json::from_value(v.get("sign")?.clone()).ok()?;
        let dh: PublicJwk = serde_json::from_value(v.get("dh")?.clone()).ok()?;
        Some((sign, dh))
    }

    /// Makes sure this install has device keys and a device list it is in. Runs whenever the account is loaded.
    ///
    /// A device holding the account key makes its keys and a first list itself. A device linked by QR code got both
    /// when it was linked; it cannot sign a list, so without a valid one it has no device.
    pub(super) fn ensure_devices(&mut self) -> Result<(), ErrorCode> {
        let (Some((sign, dh)), Some(key)) = (self.account.clone(), self.device_keys_key()) else {
            self.device_id = None;
            self.device_list = None;
            self.sync = None;
            return Ok(());
        };
        let root = self.identity.is_some();
        let mut ops = Vec::new();
        let device = match self.load_device_keys(&key)? {
            Some(d) => d,
            None if !root => {
                self.device_id = None;
                self.device_list = None;
                self.sync = None;
                return Ok(());
            }
            None => {
                let d = Identity::generate(&mut *self.ports.entropy)?;
                let s = d.export_for_storage();
                let mut value = Zeroizing::new(Vec::with_capacity(64));
                value.extend_from_slice(&s.sign);
                value.extend_from_slice(&s.dh);
                ops.push(StorageOp::SetKv {
                    key: key.clone(),
                    value: value.to_vec(),
                });
                d
            }
        };
        let id = privcord_crypto::devices::device_id(&device.sign_public(), &device.dh_public())?;
        let stored = self
            .ports
            .storage
            .kv(KV_DEVICE_LIST)?
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|t| DeviceList::from_json(&t).ok())
            .filter(|l| l.verify(&sign, &dh).is_ok());
        if stored
            .as_ref()
            .is_some_and(|l| l.get(&id).is_some_and(|d| !d.active()))
        {
            // The account removed this device.
            self.removed_from_account = true;
        }
        let stored = stored.filter(|l| l.get(&id).is_some_and(|d| d.active()));
        let list = match (stored, self.identity.as_ref()) {
            (Some(l), _) => l,
            (None, None) => {
                self.device_id = None;
                self.device_list = None;
                self.sync = None;
                return Ok(());
            }
            (None, Some(account)) => {
                // Another device of this account (a phrase-restored install, or one linked earlier) may have a list
                // already: the two are merged by a holder of the account key when they meet (core/sync.rs).
                let l = DeviceList::first(account, &device, FIRST_NAME, self.now())
                    .map_err(|_| ErrorCode::Crypto)?;
                ops.push(StorageOp::SetKv {
                    key: KV_DEVICE_LIST.into(),
                    value: l.to_json().into_bytes(),
                });
                l
            }
        };
        if !ops.is_empty() {
            self.ports.storage.apply(&ops)?;
        }
        let same_device = self.device_id.as_deref() == Some(id.as_str()) && self.sync.is_some();
        self.device_id = Some(id);
        self.device_list = Some(list);
        if !same_device {
            self.ensure_sync(device)?;
        }
        Ok(())
    }

    /// The person an address belongs to: `uid` for `uid` and for `uid.<device>`.
    pub(super) fn person_of(addr: &UserId) -> UserId {
        match addr.as_str().split_once(ADDR_SEP) {
            Some((uid, _)) => UserId::new(uid).unwrap_or_else(|_| addr.clone()),
            None => addr.clone(),
        }
    }

    /// The device an address names, if it names one.
    pub(super) fn device_of(addr: &UserId) -> Option<&str> {
        addr.as_str().split_once(ADDR_SEP).map(|(_, d)| d)
    }

    pub(super) fn device_address(uid: &UserId, device: &str) -> Option<UserId> {
        UserId::new(format!("{uid}{ADDR_SEP}{device}")).ok()
    }

    /// Where a friend is reached: each of their devices once their list has had more than one (also after removals
    /// leave one: the account address may belong to a removed device), else their account.
    pub(super) fn addresses_of(&self, uid: &UserId) -> Vec<UserId> {
        let many = self
            .contact_keys(uid)
            .and_then(|k| k.list().cloned())
            .filter(|l| l.devices.len() > 1);
        match many {
            Some(l) => l
                .active()
                .filter_map(|d| Self::device_address(uid, &d.id))
                .collect(),
            None => vec![uid.clone()],
        }
    }

    /// Whether `addr` is a way to reach a contact of ours: their account, or a device on their list (as we know it).
    pub(super) fn known_address(&self, addr: &UserId) -> bool {
        let person = Self::person_of(addr);
        if Some(&person) == self.uid.as_ref() {
            // Another device of ours, never this one and never the bare account.
            return Self::device_of(addr).is_some_and(|d| {
                Some(d) != self.device_id.as_deref()
                    && self
                        .device_list
                        .as_ref()
                        .and_then(|l| l.get(d))
                        .is_some_and(|e| e.active())
            });
        }
        if !self.contacts.contains_key(&person) {
            return false;
        }
        match Self::device_of(addr) {
            None => true,
            Some(d) => self
                .contact_keys(&person)
                .and_then(|k| k.list().map(|l| l.get(d).is_some_and(|e| e.active())))
                .unwrap_or(false),
        }
    }

    /// The keys a link or blob from `addr` must be signed with: their account's, or that device's (from their list).
    pub(super) fn keys_of_address(&self, addr: &UserId) -> Option<(PublicJwk, PublicJwk)> {
        let person = Self::person_of(addr);
        let keys = self.contact_keys(&person)?;
        match Self::device_of(addr) {
            None => Some((keys.sign.clone(), keys.dh.clone())),
            Some(d) => keys
                .list()?
                .get(d)
                .filter(|e| e.active())
                .map(|e| (e.sign.clone(), e.dh.clone())),
        }
    }

    /// Puts the mark that says our app understands device lists into something we send.
    pub(super) fn mark_devices(&self, value: &mut Value) {
        if let (false, Some(o)) = (self.old_version, value.as_object_mut()) {
            o.insert(AWARE_FIELD.into(), json!(1));
        }
    }

    /// Something from `from` carried the mark: their app understands device lists, and from now on we may speak to
    /// them as a device.
    pub(super) fn note_devices(&mut self, from: &UserId, value: &Value) {
        if value.get(AWARE_FIELD).is_none() {
            return;
        }
        let person = Self::person_of(from);
        let mut aware = self.aware_list();
        if aware.insert(person.to_string()) {
            let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                key: KV_AWARE.into(),
                value: serde_json::to_vec(&aware).unwrap_or_default(),
            }]);
        }
    }

    fn aware_list(&self) -> std::collections::BTreeSet<String> {
        self.ports
            .storage
            .kv(KV_AWARE)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// A person was removed: forget that their app understood device lists (it is learned again from what they send).
    pub(super) fn forget_aware(&mut self, person: &UserId) {
        let mut aware = self.aware_list();
        if aware.remove(person.as_str()) {
            let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                key: KV_AWARE.into(),
                value: serde_json::to_vec(&aware).unwrap_or_default(),
            }]);
        }
    }

    /// Whether a friend's app understands device lists (we have seen it say so).
    pub(super) fn understands_devices(&self, person: &UserId) -> bool {
        self.aware_list().contains(person.as_str())
    }

    /// Tests only: behave like an app from before device lists.
    #[doc(hidden)]
    pub fn act_as_old_version(&mut self) {
        self.old_version = true;
    }

    /// How we present ourselves by default: as this device once the account has had more than one, else as the
    /// account.
    pub(super) fn own_address(&self) -> Option<UserId> {
        let uid = self.uid.as_ref()?;
        let many = self
            .device_list
            .as_ref()
            .is_some_and(|l| l.devices.len() > 1);
        match (&self.device_id, many) {
            (Some(d), true) => Self::device_address(uid, d),
            _ => Some(uid.clone()),
        }
    }

    /// Our addresses: the account (where an install holding its key answers) and this device.
    pub(super) fn is_own_address(&self, addr: &str) -> bool {
        let Some(uid) = &self.uid else {
            return false;
        };
        (addr == uid.as_str() && self.identity.is_some())
            || self
                .own_address()
                .is_some_and(|a| a.as_str() == addr && Self::device_of(&a).is_some())
    }

    /// The key we sign with as `as_me` (one of our addresses): the account key, or this device's key.
    pub(super) fn key_for_address(&self, as_me: &UserId) -> Option<&Identity> {
        match Self::device_of(as_me) {
            None => self.identity.as_ref(),
            Some(_) => self.sync.as_ref().map(super::sync::SyncState::device_key),
        }
    }

    /// Whether an address is one of our own other devices.
    pub(super) fn is_own_person(&self, addr: &UserId) -> bool {
        self.uid.as_ref() == Some(&Self::person_of(addr)) && Self::device_of(addr).is_some()
    }

    /// Our other devices, by address (only once the account has several and this install has its sync key).
    pub(super) fn own_other_devices(&self) -> Vec<UserId> {
        let (Some(uid), Some(list), Some(me)) = (&self.uid, &self.device_list, &self.device_id)
        else {
            return Vec::new();
        };
        list.active()
            .filter(|d| &d.id != me)
            .filter_map(|d| Self::device_address(uid, &d.id))
            .collect()
    }

    /// Every address of a person with a live link up now.
    pub(super) fn ready_addresses(&self, person: &UserId) -> Vec<UserId> {
        self.peers
            .iter()
            .filter(|(a, p)| &Self::person_of(a) == person && p.ready())
            .map(|(a, _)| a.clone())
            .collect()
    }

    /// Whether any device of a person (or their account) has a live link up.
    pub(super) fn person_ready(&self, person: &UserId) -> bool {
        self.peers
            .iter()
            .any(|(a, p)| &Self::person_of(a) == person && p.ready())
    }

    /// How connected a person is: their best link (a person with one link up is connected).
    pub(super) fn person_state(&self, person: &UserId) -> privcord_domain::state::ConnectionState {
        use privcord_domain::state::ConnectionState as S;
        let rank = |s: &S| match s {
            S::Connected { .. } => 3,
            S::Connecting => 2,
            S::Failed { .. } => 1,
            _ => 0,
        };
        self.peers
            .iter()
            .filter(|(a, _)| &Self::person_of(a) == person)
            .map(|(_, p)| p.state)
            .max_by_key(rank)
            .unwrap_or(S::Offline)
    }

    /// Send what waits for a person over every live link to them.
    pub(super) fn flush_person(&mut self, person: &UserId) {
        for addr in self.ready_addresses(person) {
            self.flush(&addr);
        }
    }

    /// The address we use with `peer`: the one they reached us at, else our default.
    pub(super) fn as_me(&self, peer: &UserId) -> Option<UserId> {
        if let Some(a) = self.peers.get(peer).and_then(|p| p.as_me.clone()) {
            return Some(a);
        }
        if self.is_own_person(peer) {
            return self.own_address();
        }
        // A friend whose app predates device lists checks only the account key: the install holding it speaks to them
        // as the account (a linked device cannot; what it writes to them goes out from the install that can, see
        // `deliver_for_devices`).
        if self.identity.is_some() && !self.understands_devices(&Self::person_of(peer)) {
            return self.uid.clone();
        }
        self.own_address()
    }

    /// The key this install signs with on its link to `peer`.
    pub(super) fn link_key(&self, peer: &UserId) -> Option<Identity> {
        self.key_for_address(&self.as_me(peer)?).and_then(copy_key)
    }

    /// Who signs what this install sends to friends: the account key where this install holds it, else this device's
    /// own key with the account's list. (A free function over the fields, so the caller can still lend out entropy.)
    pub(super) fn author_of<'a>(
        uid: Option<&'a UserId>,
        identity: Option<&'a Identity>,
        sync: Option<&'a super::sync::SyncState>,
        list: Option<&'a DeviceList>,
    ) -> Result<Author<'a>, ErrorCode> {
        let uid = uid.ok_or(ErrorCode::NoIdentity)?;
        if let Some(id) = identity {
            return Ok(Author::account(uid, id));
        }
        match (sync, list) {
            (Some(s), Some(l)) => Ok(Author::device(uid, s.device_key(), l)),
            _ => Err(ErrorCode::NoIdentity),
        }
    }

    /// Who signs a change to a group: the account key where this install holds it (members on older apps check only that),
    /// else this device. Once the account has several devices the change carries the account's list, so every member
    /// learns of a device the account removed.
    pub(super) fn group_author<'a>(
        uid: Option<&'a UserId>,
        identity: Option<&'a Identity>,
        sync: Option<&'a super::sync::SyncState>,
        list: Option<&'a DeviceList>,
    ) -> Result<Author<'a>, ErrorCode> {
        let author = Self::author_of(uid, identity, sync, list)?;
        Ok(Author {
            list: list.filter(|l| l.devices.len() > 1),
            ..author
        })
    }

    /// The account's card with `name` on it (the name is not part of what the account key signs, so a linked device
    /// can put the current one on the card it was given).
    pub(super) fn own_contact_card(&self, name: Option<&str>) -> Result<ContactCard, ErrorCode> {
        if let Some(id) = &self.identity {
            return Ok(ContactCard::of(id, name)?);
        }
        let (_, card) = self
            .stored_account_card()
            .ok_or_else(|| self.identity().err().unwrap_or(ErrorCode::NoIdentity))?;
        let named = ContactCard {
            name: name.map(str::to_owned),
            ..card
        };
        // Through the text form, so the name is cut the way every card's is.
        ContactCard::decode(&named.encode()?)
            .map(|c| c.card)
            .ok_or(ErrorCode::Crypto)
    }

    fn contact_list_key(uid: &UserId) -> String {
        format!("{KV_CONTACT_LIST}.{uid}")
    }

    /// What we know of a friend's account: the keys on their card and the newest device list of theirs we have seen.
    pub(super) fn contact_keys(&self, uid: &UserId) -> Option<AccountKeys> {
        if Some(uid) == self.uid.as_ref() {
            // Ourselves (another device of ours): the account's keys and our own list.
            let (sign, dh) = self.account.clone()?;
            return Some(AccountKeys::new(
                uid.clone(),
                sign,
                dh,
                self.device_list.clone(),
            ));
        }
        let card = &self.contacts.get(uid)?.contact.card;
        let list = self
            .ports
            .storage
            .kv(&Self::contact_list_key(uid))
            .ok()
            .flatten()
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|t| DeviceList::from_json(&t).ok());
        Some(AccountKeys::new(
            uid.clone(),
            card.sign.clone(),
            card.dh.clone(),
            list,
        ))
    }

    /// A friend's device list arrived (with something one of their devices sent, or as a notice): kept if it is the
    /// first we have or extends the one we have.
    pub(super) fn learn_contact_list(&mut self, uid: &UserId, list: &DeviceList) {
        if self.old_version || Some(uid) == self.uid.as_ref() {
            return;
        }
        let Some(mut keys) = self.contact_keys(uid) else {
            return;
        };
        if keys.learn(list) {
            let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                key: Self::contact_list_key(uid),
                value: list.to_json().into_bytes(),
            }]);
            // Devices it removed: their keys go (their links close on the next tick).
            self.drop_device_keys(uid, list);
        }
        self.file_keys_under_device(uid);
    }

    /// A friend's device list as a notice (a frame on a link, or sealed to our mailbox).
    ///
    /// `keysOf` names the device of theirs that sent it, when it is the install their account address was: the keys we
    /// exchanged with that address are that device's, and move to its own address (so nothing falls back to the old
    /// protocol when they start using device addresses). Only that device itself can say so.
    pub(super) fn on_device_notice(&mut self, from: &UserId, notice: &Value) {
        let person = Self::person_of(from);
        if let Some(list) = notice
            .get("list")
            .and_then(|v| serde_json::from_value::<DeviceList>(v.clone()).ok())
        {
            self.learn_contact_list(&person, &list);
        }
        let keys_of = notice.get("keysOf").and_then(Value::as_str);
        if keys_of.is_some() && keys_of == Self::device_of(from) && self.known_address(from) {
            self.move_keys(&person, from);
        }
    }

    /// Forget what we knew of a friend's devices (they were removed).
    pub(super) fn forget_contact_devices(&mut self, uid: &UserId) {
        let mut told = self.told();
        told.remove(uid.as_str());
        let _ = self.ports.storage.apply(&[
            StorageOp::SetKv {
                key: Self::contact_list_key(uid),
                value: Vec::new(),
            },
            Self::told_op(&told),
        ]);
    }

    fn told(&self) -> BTreeMap<String, u64> {
        self.ports
            .storage
            .kv(KV_TOLD)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn told_op(told: &BTreeMap<String, u64>) -> StorageOp {
        StorageOp::SetKv {
            key: KV_TOLD.into(),
            value: serde_json::to_vec(told).unwrap_or_default(),
        }
    }

    /// Our list, when friends need to hear of it: once the account has more than one device. (Only an install holding
    /// the account key tells them for now; a linked device reaches friends from per-device delivery on.)
    fn list_to_tell(&self) -> Option<DeviceList> {
        self.device_list.clone().filter(|l| l.devices.len() > 1)
    }

    /// What a list notice says: the list, and which device sends it once this install speaks as a device (see
    /// `on_device_notice`).
    fn device_notice(&self, list: &DeviceList) -> Value {
        let mut v = json!({ "list": list });
        if let (Some(o), Some(d)) = (v.as_object_mut(), &self.device_id) {
            if self
                .own_address()
                .as_ref()
                .and_then(Self::device_of)
                .is_some()
            {
                o.insert("keysOf".into(), json!(d));
            }
        }
        v
    }

    /// A link to a friend is up: hand them our list if they have not had this version.
    pub(super) fn tell_devices_on_link(&mut self, peer: &UserId) {
        let Some(list) = self.list_to_tell() else {
            return;
        };
        let mut told = self.told();
        if told.get(peer.as_str()).is_some_and(|v| *v >= list.version) {
            return;
        }
        let mut frame = self.device_notice(&list);
        if let Some(o) = frame.as_object_mut() {
            o.insert("v".into(), json!(privcord_domain::limits::PROTOCOL_VERSION));
            o.insert("kind".into(), json!("devices"));
        }
        if self.send_secure(peer, &frame) {
            told.insert(peer.to_string(), list.version);
            let _ = self.ports.storage.apply(&[Self::told_op(&told)]);
        }
    }

    /// Friends who have not had our newest list get it sealed to their mailbox (so they hear of a new device even
    /// before it ever writes to them, and of a removed one as soon as they are online).
    pub(super) fn tick_device_notices(&mut self) {
        let Some(list) = self.list_to_tell() else {
            return;
        };
        if !self.ports.transport.signal_available() {
            return;
        }
        let mut told = self.told();
        let due: Vec<UserId> = self
            .contacts
            .keys()
            .flat_map(|u| self.addresses_of(u))
            .filter(|a| told.get(a.as_str()).is_none_or(|v| *v < list.version))
            .collect();
        if due.is_empty() {
            return;
        }
        let mut notice = self.device_notice(&list);
        if let Some(o) = notice.as_object_mut() {
            o.insert("t".into(), json!("devices"));
        }
        for uid in due {
            if self
                .put_sealed(&uid, &notice, PutKind::Msg, NOTICE_TTL_SEC)
                .is_ok()
            {
                told.insert(uid.to_string(), list.version);
            }
        }
        let _ = self.ports.storage.apply(&[Self::told_op(&told)]);
    }

    /// A friend's device list, the newest we have seen (`None` until one of their devices was linked).
    pub fn contact_device_list(&self, uid: &UserId) -> Option<DeviceList> {
        self.contact_keys(uid)?.list().cloned()
    }

    /// This install's device id, once it has an identity.
    pub fn this_device_id(&self) -> Option<&str> {
        self.device_id.as_deref()
    }

    /// The account's device list, as this install knows it.
    pub fn device_list(&self) -> Option<&DeviceList> {
        self.device_list.as_ref()
    }
}

/// A second handle on a key (keys are not `Clone`, so a copy is always deliberate): for a handshake that needs it while
/// the engine is borrowed mutably.
pub(super) fn copy_key(key: &Identity) -> Option<Identity> {
    Identity::from_stored(&key.export_for_storage()).ok()
}
