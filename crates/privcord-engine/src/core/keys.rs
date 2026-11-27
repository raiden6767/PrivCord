//! Protocol v2 keys for sealed blobs (see `privcord_crypto::prekeys`): our own (with private halves) and each
//! contact's (public), kept in the encrypted database and refreshed over every post-quantum link.
//!
//! * One-time keys: each link hands the contact 10 new ones. A text message for someone who is offline is sealed to one
//!   of theirs, which the sender then forgets; the recipient erases its private half once the message is open. So an
//!   offline message has forward secrecy and is post-quantum.
//! * A standing key, replaced every 7 days (the previous ones are kept 30 days for late blobs): for signals, receipts,
//!   file pieces and when the one-time keys have run out.
//! * Sealing classically (protocol v1) is only for someone we have never had a link with (first contact). Once keys
//!   have been exchanged, a v1 blob from that person is refused, so nobody can force the old protocol.

use super::{Core, StorageOp};
use crate::error::ErrorCode;
use privcord_crypto::{
    devices::{AccountKeys, Signer},
    prekeys::{open_anon, open_v2, seal_anon, seal_v2, OwnKey, PeerKey},
    seal::Opened,
};
use privcord_domain::signaling::PutKind;
use privcord_domain::{
    blob::{PublicJwk, SealedBlob},
    UserId,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

const KV_KEYS: &str = "pq.keys";
const ONETIME_PER_LINK: usize = 10;
/// One-time keys we keep for one contact at most (the oldest go first).
const MAX_ISSUED_PER_PEER: usize = 60;
/// Their one-time keys we keep at most.
const MAX_THEIRS: usize = 50;
const STANDING_ROTATE_MS: i64 = 7 * 24 * 3600 * 1000;
/// Any of our keys older than this is erased (relays keep a stored blob for 7 days at most).
const KEY_LIFETIME_MS: i64 = 30 * 24 * 3600 * 1000;
/// How long keys handed over through a relay wait there for a friend who is offline.
const KEYS_TTL_SEC: u32 = 7 * 24 * 3600;
/// A contact who has this many of our one-time keys left or fewer is sent a fresh batch (each offline message uses one;
/// with none left it is sealed to our standing key, which is weaker for forward secrecy).
const LOW_WATER: usize = 3;
/// At most one top-up per contact address in this long: a contact who never opens what they hold must not make us post a
/// batch (about 35 KB) over and over.
const TOPUP_COOLDOWN_MS: i64 = 6 * 3600 * 1000;

/// A mailbox name as we make them: 64 lowercase hex characters.
fn is_mailbox_name(m: &str) -> bool {
    m.len() == 64
        && m.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) struct TheirKeys {
    standing: PeerKey,
    onetime: Vec<PeerKey>,
    /// Their app opens the anonymous envelope (protocol v3), so what we post to a relay for them can be wrapped in one.
    anon: bool,
    /// The private inbox they listen on (see `Core::own_inbox_mailbox`), if they told us.
    inbox: Option<String>,
    /// The install that sent them, when it said (see `fresh_keys_for`): filed under its own address once we know it.
    dev: Option<String>,
    /// The inbox generation they told us with these keys: an older one from the same install is a replay and is ignored.
    ig: u32,
}

#[derive(Default)]
pub(super) struct KeyState {
    /// Our standing keys, newest first.
    standing: Vec<OwnKey>,
    /// One-time keys we gave each contact.
    issued: BTreeMap<UserId, Vec<OwnKey>>,
    /// Each contact's keys, as they last gave them to us.
    theirs: BTreeMap<UserId, TheirKeys>,
    /// Addresses that hold our keys (a link exchanged them, or a v2 blob came from them): a v1 blob from these is
    /// refused, so nobody can force the old protocol on them. Keys that came through a relay say nothing about this.
    confirmed: std::collections::BTreeSet<UserId>,
    /// Which private inbox we are on (it changes when a contact is removed, so they can no longer watch it), and the
    /// one each contact address was last told. An address with no entry was told generation 0.
    inbox_gen: u32,
    told_gen: BTreeMap<UserId, u32>,
    /// When each address was last sent a top-up (kept in memory only: after a restart the wait starts again).
    topup_at: BTreeMap<UserId, i64>,
}

impl KeyState {
    fn to_value(&self) -> Value {
        let issued: Map<String, Value> = self
            .issued
            .iter()
            .map(|(u, ks)| {
                (
                    u.to_string(),
                    Value::Array(ks.iter().map(OwnKey::to_private_value).collect()),
                )
            })
            .collect();
        let theirs: Map<String, Value> = self
            .theirs
            .iter()
            .map(|(u, t)| {
                (
                    u.to_string(),
                    json!({ "standing": t.standing.to_value(), "onetime": t.onetime.iter().map(PeerKey::to_value).collect::<Vec<_>>(), "anon": t.anon, "inbox": t.inbox, "dev": t.dev, "ig": t.ig }),
                )
            })
            .collect();
        json!({
            "standing": self.standing.iter().map(OwnKey::to_private_value).collect::<Vec<_>>(),
            "issued": issued,
            "theirs": theirs,
            "confirmed": self.confirmed.iter().map(UserId::as_str).collect::<Vec<_>>(),
            "inboxGen": self.inbox_gen,
            "toldGen": self.told_gen.iter().map(|(u, g)| (u.to_string(), json!(g))).collect::<Map<String, Value>>(),
        })
    }

    fn from_value(v: &Value) -> Self {
        let own = |a: Option<&Value>| -> Vec<OwnKey> {
            a.and_then(Value::as_array)
                .map(|a| a.iter().filter_map(OwnKey::from_private_value).collect())
                .unwrap_or_default()
        };
        let mut state = Self {
            standing: own(v.get("standing")),
            inbox_gen: v
                .get("inboxGen")
                .and_then(Value::as_u64)
                .and_then(|g| u32::try_from(g).ok())
                .unwrap_or(0),
            ..Self::default()
        };
        if let Some(m) = v.get("toldGen").and_then(Value::as_object) {
            for (u, g) in m {
                if let (Ok(uid), Some(g)) = (
                    UserId::new(u),
                    g.as_u64().and_then(|g| u32::try_from(g).ok()),
                ) {
                    state.told_gen.insert(uid, g);
                }
            }
        }
        if let Some(m) = v.get("issued").and_then(Value::as_object) {
            for (u, ks) in m {
                if let Ok(uid) = UserId::new(u) {
                    state.issued.insert(uid, own(Some(ks)));
                }
            }
        }
        if let Some(m) = v.get("theirs").and_then(Value::as_object) {
            for (u, t) in m {
                let (Ok(uid), Some(standing)) = (
                    UserId::new(u),
                    t.get("standing").and_then(PeerKey::from_value),
                ) else {
                    continue;
                };
                let onetime = t
                    .get("onetime")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(PeerKey::from_value).collect())
                    .unwrap_or_default();
                let anon = t.get("anon").and_then(Value::as_bool) == Some(true);
                let inbox = t
                    .get("inbox")
                    .and_then(Value::as_str)
                    .filter(|m| is_mailbox_name(m))
                    .map(str::to_owned);
                let dev = t.get("dev").and_then(Value::as_str).map(str::to_owned);
                let ig = t
                    .get("ig")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(0);
                state.theirs.insert(
                    uid,
                    TheirKeys {
                        standing,
                        onetime,
                        anon,
                        inbox,
                        dev,
                        ig,
                    },
                );
            }
        }
        state.confirmed = match v.get("confirmed").and_then(Value::as_array) {
            Some(a) => a
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|u| UserId::new(u).ok())
                .collect(),
            // Saved before this was kept: every key we held then came over a link, which exchanged ours too.
            None => state.theirs.keys().cloned().collect(),
        };
        state
    }
}

impl Core {
    pub(super) fn load_keys(&mut self) {
        self.keys = self
            .ports
            .storage
            .kv(KV_KEYS)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .map(|v| KeyState::from_value(&v))
            .unwrap_or_default();
    }

    fn save_keys(&mut self) {
        let value = serde_json::to_vec(&self.keys.to_value()).unwrap_or_default();
        let _ = self.ports.storage.apply(&[StorageOp::SetKv {
            key: KV_KEYS.into(),
            value,
        }]);
    }

    /// Forget every key (a new identity, or everything erased).
    pub(super) fn reset_keys(&mut self) {
        self.keys = KeyState::default();
        self.save_keys();
    }

    /// Fresh keys for `peer`: refresh our standing key if due, drop expired keys, and issue new one-time keys to them.
    /// Returns `{ standing, onetime }` to hand over.
    fn fresh_keys_for(&mut self, peer: &UserId) -> Option<Value> {
        let now = self.now();
        let fresh = self
            .keys
            .standing
            .first()
            .is_some_and(|k| now - k.created_at < STANDING_ROTATE_MS);
        if !fresh {
            match OwnKey::generate(&mut *self.ports.entropy, now) {
                Ok(k) => self.keys.standing.insert(0, k),
                Err(_) => return None,
            }
        }
        self.keys
            .standing
            .retain(|k| now - k.created_at < KEY_LIFETIME_MS);
        for list in self.keys.issued.values_mut() {
            list.retain(|k| now - k.created_at < KEY_LIFETIME_MS);
        }
        let mut new = Vec::with_capacity(ONETIME_PER_LINK);
        for _ in 0..ONETIME_PER_LINK {
            match OwnKey::generate(&mut *self.ports.entropy, now) {
                Ok(k) => new.push(k),
                Err(_) => return None,
            }
        }
        let publics: Vec<Value> = new.iter().map(|k| k.public().to_value()).collect();
        let list = self.keys.issued.entry(peer.clone()).or_default();
        list.extend(new);
        let over = list.len().saturating_sub(MAX_ISSUED_PER_PEER);
        list.drain(..over);
        let standing = self.keys.standing.first().map(|k| k.public().to_value())?;
        self.keys.told_gen.insert(peer.clone(), self.keys.inbox_gen);
        self.save_keys();
        // "anon": this app opens the anonymous envelope, so friends may wrap what they post to a relay for us. "inbox":
        // where to post for us instead of the mailbox anyone can work out from our public id.
        let inbox = self.as_me(peer).and_then(|me| self.own_inbox_mailbox(&me));
        // "dev": the install these keys belong to, so a friend who still has us under the account address can file
        // them under the device's own (the account key's holder speaks as the account until they know our list).
        Some(
            json!({ "standing": standing, "onetime": publics, "anon": true, "inbox": inbox, "dev": self.device_id, "ig": self.keys.inbox_gen }),
        )
    }

    /// A post-quantum link to `peer` is up: hand them our standing key and fresh one-time keys over it.
    pub(super) fn keys_on_link(&mut self, peer: &UserId) {
        let Some(mut frame) = self.fresh_keys_for(peer) else {
            return;
        };
        if let Some(o) = frame.as_object_mut() {
            o.insert("v".into(), json!(privcord_domain::limits::PROTOCOL_VERSION));
            o.insert("kind".into(), json!("keys"));
        }
        self.send_secure(peer, &frame);
    }

    /// Addresses of friends we have never given keys to (a friend just added, or a device of theirs just linked) get
    /// them sealed through the relay, so even their first message to us while we are offline is sealed post-quantum.
    pub(super) fn tick_key_handover(&mut self) {
        // Kept on the relays for days: only when the person lets the relays keep things for them.
        if !self.ports.transport.signal_available() || !self.store_forward() {
            return;
        }
        let now = self.now();
        let due: Vec<UserId> = self
            .contacts
            .keys()
            .flat_map(|u| self.addresses_of(u))
            .filter(|a| {
                !self.keys.issued.contains_key(a)
                    || self.keys.told_gen.get(a).copied().unwrap_or(0) != self.keys.inbox_gen
                    || (self
                        .keys
                        .issued
                        .get(a)
                        .is_some_and(|l| l.len() <= LOW_WATER)
                        && self
                            .keys
                            .topup_at
                            .get(a)
                            .is_none_or(|t| now - t >= TOPUP_COOLDOWN_MS))
            })
            .collect();
        for addr in due {
            // The first batch for an address is not a top-up; only later ones wait out the cooldown.
            if self.keys.issued.contains_key(&addr) {
                self.keys.topup_at.insert(addr.clone(), now);
            }
            let Some(mut payload) = self.fresh_keys_for(&addr) else {
                return;
            };
            if let Some(o) = payload.as_object_mut() {
                o.insert("t".into(), json!("keys"));
            }
            if self
                .put_sealed(&addr, &payload, PutKind::Msg, KEYS_TTL_SEC)
                .is_err()
            {
                // Not handed over: try again on a later tick.
                self.keys.issued.remove(&addr);
                self.keys.told_gen.remove(&addr);
                self.save_keys();
            }
        }
    }

    /// Their keys arrived over the link: they replace what we had, and the link gave them ours too.
    pub(super) fn on_keys_frame(&mut self, peer: &UserId, frame: &Value) {
        self.keys.confirmed.insert(peer.clone());
        self.take_keys(peer, frame);
    }

    /// Their keys (over a link, or sealed through a relay): they replace what we had (unused old ones of theirs may
    /// already be gone).
    pub(super) fn take_keys(&mut self, peer: &UserId, frame: &Value) {
        let Some(standing) = frame.get("standing").and_then(PeerKey::from_value) else {
            return;
        };
        let onetime: Vec<PeerKey> = frame
            .get("onetime")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .take(MAX_THEIRS)
                    .filter_map(PeerKey::from_value)
                    .collect()
            })
            .unwrap_or_default();
        let anon = frame.get("anon").and_then(Value::as_bool) == Some(true);
        let inbox = frame
            .get("inbox")
            .and_then(Value::as_str)
            .filter(|m| is_mailbox_name(m))
            .map(str::to_owned);
        let dev = frame
            .get("dev")
            .and_then(Value::as_str)
            .filter(|d| d.len() <= 64 && d.bytes().all(|b| b.is_ascii_alphanumeric()))
            .map(str::to_owned);
        let device_address = dev
            .as_deref()
            .filter(|_| Self::device_of(peer).is_none())
            .and_then(|d| Self::device_address(peer, d))
            .filter(|a| self.known_address(a));
        let ig = frame
            .get("ig")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);
        let address = device_address.unwrap_or_else(|| peer.clone());
        // An old copy of their keys, replayed by a relay, would set their inbox back to one a removed contact still
        // watches: a frame from the same install with an older inbox generation than we hold is ignored.
        if let Some(have) = self.keys.theirs.get(&address) {
            if have.dev.is_some() && have.dev == dev && ig < have.ig {
                return;
            }
        }
        self.keys.theirs.insert(
            address,
            TheirKeys {
                standing,
                onetime,
                anon,
                inbox,
                dev,
                ig,
            },
        );
        self.save_keys();
    }

    /// Move to a new private inbox: someone who was removed still knows the old one and could watch it. Friends are
    /// told the new one with fresh keys on the next ticks; the last two older ones are still listened to, so a friend
    /// who was offline meanwhile still gets through.
    pub(super) fn rotate_inbox(&mut self) {
        self.keys.inbox_gen = self.keys.inbox_gen.saturating_add(1);
        self.save_keys();
    }

    pub(super) fn inbox_generation(&self) -> u32 {
        self.keys.inbox_gen
    }

    #[doc(hidden)]
    pub fn test_onetime_left(&self, peer: &UserId) -> usize {
        self.keys.theirs.get(peer).map_or(0, |t| t.onetime.len())
    }

    #[doc(hidden)]
    pub fn test_inbox_of(&self, peer: &UserId) -> Option<String> {
        self.keys_inbox_of(peer)
    }

    /// The private inbox `peer` told us to post to, if any.
    pub(super) fn keys_inbox_of(&self, peer: &UserId) -> Option<String> {
        self.keys.theirs.get(peer).and_then(|t| t.inbox.clone())
    }

    /// The text to post to a relay for `peer`: the sealed message itself, or, when their app opens it, that message inside
    /// an anonymous envelope, so the relay sees neither who sent it nor who it is for (only the mailbox it is posted to).
    pub(super) fn wire_for_relay(&mut self, peer: &UserId, blob: &SealedBlob) -> String {
        let inner = blob.encode();
        let Some(standing) = self
            .keys
            .theirs
            .get(peer)
            .filter(|t| t.anon)
            .map(|t| t.standing.clone())
        else {
            return inner;
        };
        match seal_anon(&standing, &inner, &mut *self.ports.entropy) {
            Ok(envelope) => envelope.encode(),
            // Too large to wrap: the plain sealed message still goes.
            Err(_) => inner,
        }
    }

    /// The sealed message inside an anonymous envelope made for one of our standing keys. The message inside is checked
    /// like any other (its sender and signature), so nobody gains anything by sending envelopes.
    pub(super) fn open_anon_blob(&self, blob: &SealedBlob) -> Option<SealedBlob> {
        let id = blob.key.as_deref()?;
        let key = self.keys.standing.iter().find(|k| k.id == id)?;
        let inner = SealedBlob::decode(&open_anon(blob, key)?)?;
        // One layer only.
        (inner.v != 3).then_some(inner)
    }

    /// For tests: what we hold for each address: (address, opens envelopes, has a private inbox).
    #[doc(hidden)]
    pub fn test_known_keys(&self) -> Vec<(String, bool, bool)> {
        self.keys
            .theirs
            .iter()
            .map(|(a, t)| (a.to_string(), t.anon, t.inbox.is_some()))
            .collect()
    }

    /// For tests: act as if this person's keys had come from an app that does not open anonymous envelopes.
    #[doc(hidden)]
    pub fn test_forget_anon_support(&mut self, person: &UserId) {
        for (addr, t) in &mut self.keys.theirs {
            if &Self::person_of(addr) == person {
                t.anon = false;
                t.inbox = None;
            }
        }
        self.save_keys();
    }

    /// `from` holds our keys (it just used one): v1 from it is refused from now on.
    fn confirm(&mut self, from: &UserId) {
        if self.keys.confirmed.insert(from.clone()) {
            self.save_keys();
        }
    }

    /// Forget every key exchanged with these addresses (devices their account removed).
    pub(super) fn forget_keys_of(&mut self, addrs: &[UserId]) {
        let mut changed = false;
        for a in addrs {
            changed |= self.keys.theirs.remove(a).is_some();
            changed |= self.keys.issued.remove(a).is_some();
            changed |= self.keys.confirmed.remove(a);
        }
        if changed {
            self.save_keys();
        }
    }

    /// Keys we hold under a friend's account address were sent by one of their devices (it said which): once their list
    /// has that device, they are filed under its address, so nothing for that device falls back to the public mailbox.
    pub(super) fn file_keys_under_device(&mut self, uid: &UserId) {
        let Some(addr) = self
            .keys
            .theirs
            .get(uid)
            .and_then(|t| t.dev.as_deref())
            .and_then(|d| Self::device_address(uid, d))
            .filter(|a| self.known_address(a))
        else {
            return;
        };
        self.move_keys(uid, &addr);
    }

    /// The keys exchanged with one address of a friend belong to another (their account address turned out to be one
    /// of their devices). Moved only where the new address has none yet.
    pub(super) fn move_keys(&mut self, from: &UserId, to: &UserId) {
        let mut moved = false;
        if !self.keys.theirs.contains_key(to) {
            if let Some(t) = self.keys.theirs.remove(from) {
                self.keys.theirs.insert(to.clone(), t);
                moved = true;
            }
        }
        if !self.keys.issued.contains_key(to) {
            if let Some(i) = self.keys.issued.remove(from) {
                self.keys.issued.insert(to.clone(), i);
                moved = true;
            }
        }
        if self.keys.confirmed.remove(from) {
            self.keys.confirmed.insert(to.clone());
            moved = true;
        }
        if moved {
            self.save_keys();
        }
    }

    /// Seal for a contact: v2 to one of their keys when we have them (a one-time key for a text message), else v1
    /// (only before the first link with them).
    pub(super) fn seal_for(
        &mut self,
        peer: &UserId,
        payload: &Value,
        dh: &PublicJwk,
    ) -> Result<SealedBlob, ErrorCode> {
        let one_time = payload.get("t").and_then(Value::as_str) == Some("msg");
        let key = match self.keys.theirs.get_mut(peer) {
            Some(t) if one_time && !t.onetime.is_empty() => Some(t.onetime.remove(0)),
            Some(t) => Some(t.standing.clone()),
            None => None,
        };
        // Signed as the address they know us by on this route.
        let as_me = self.as_me(peer).ok_or(ErrorCode::NoIdentity)?;
        let uid = self.uid.clone().ok_or(ErrorCode::NoIdentity)?;
        let author = match Self::device_of(&as_me) {
            None => privcord_crypto::devices::Author::account(
                &uid,
                self.identity.as_ref().ok_or(ErrorCode::NoIdentity)?,
            ),
            Some(_) => privcord_crypto::devices::Author::device(
                &uid,
                self.sync
                    .as_ref()
                    .map(super::sync::SyncState::device_key)
                    .ok_or(ErrorCode::NoIdentity)?,
                self.device_list.as_ref().ok_or(ErrorCode::NoIdentity)?,
            ),
        };
        let blob = match &key {
            Some(k) => seal_v2(&author, peer, k, payload, &mut *self.ports.entropy)?,
            None => privcord_crypto::seal::seal_as(
                &author,
                peer,
                dh,
                payload,
                &mut *self.ports.entropy,
            )?,
        };
        if one_time && key.is_some() {
            // The one-time key is used: never again.
            self.save_keys();
        }
        Ok(blob)
    }

    /// Open a blob from a contact: v2 with the key it names (a one-time key is erased once used); v1 only from someone
    /// we have never exchanged keys with.
    ///
    /// Returns what it said and the address it came from (their account, or the device that signed it). A device list
    /// the blob carries is learned first, so a device that list adds is known by the time anything answers it.
    pub(super) fn open_from_contact(
        &mut self,
        blob: &SealedBlob,
        keys: &AccountKeys,
    ) -> Option<(Value, UserId)> {
        let (opened, from) = self.open_sealed(blob, keys)?;
        if self.old_version && opened.signer != Signer::Account {
            return None;
        }
        if let Some(list) = &opened.list {
            self.learn_contact_list(&keys.uid, list);
        }
        Some((opened.payload, from))
    }

    fn sender_address(person: &UserId, signer: &Signer) -> Option<UserId> {
        match signer {
            Signer::Account => Some(person.clone()),
            Signer::Device(d) => UserId::new(format!("{person}.{d}")).ok(),
        }
    }

    fn open_sealed(&mut self, blob: &SealedBlob, keys: &AccountKeys) -> Option<(Opened, UserId)> {
        // Sealed to one of our addresses: the account (only with its key) or this device.
        if !self.is_own_address(&blob.to) {
            return None;
        }
        let own = UserId::new(blob.to.as_str()).ok()?;
        let person = &keys.uid;
        if blob.v == 2 {
            let id = blob.key.as_deref()?;
            if let Some(k) = self.keys.standing.iter().find(|k| k.id == id) {
                let opened = open_v2(blob, &own, k, keys)?;
                let from = Self::sender_address(person, &opened.signer)?;
                self.confirm(&from);
                return Some((opened, from));
            }
            // A one-time key goes to one address of theirs; only that address may use it.
            let (addr, at) = self.keys.issued.iter().find_map(|(addr, list)| {
                (&Self::person_of(addr) == person)
                    .then(|| {
                        list.iter()
                            .position(|k| k.id == id)
                            .map(|at| (addr.clone(), at))
                    })
                    .flatten()
            })?;
            let opened = open_v2(blob, &own, self.keys.issued.get(&addr)?.get(at)?, keys)?;
            let from = Self::sender_address(person, &opened.signer)?;
            // A key given to their account address and used by one of their devices: that device is the install the
            // key went to (nobody else has it), so the keys of that address are its keys from now on.
            let was_account = &addr == person && Self::device_of(&from).is_some();
            // The other way round: a key given to a device's address, used by the account key's holder speaking as the
            // account (it does, to a friend whose app does not know device lists yet). Only that install has both.
            let account_for_device = &from == person && Self::device_of(&addr).is_some();
            if from != addr && !was_account && !account_for_device {
                return None;
            }
            if let Some(list) = self.keys.issued.get_mut(&addr) {
                list.remove(at);
            }
            self.save_keys();
            if was_account {
                self.move_keys(&addr, &from);
            }
            self.confirm(&from);
            return Some((opened, from));
        }
        let identity = self.key_for_address(&own)?;
        let opened = privcord_crypto::seal::open(blob, &own, identity, keys)?;
        let from = Self::sender_address(person, &opened.signer)?;
        // Once that address has shown it holds our keys, the old protocol is refused.
        if self.keys.confirmed.contains(&from) {
            return None;
        }
        Some((opened, from))
    }
}
