//! The sync log (docs/design/syncengine.md §3): how the devices of one account agree on everything that happened.
//!
//! Every durable change a device makes is an [`Entry`] in that device's own log, numbered 1, 2, 3 … with no gaps,
//! chained by hash to the one before, ordered by a hybrid logical clock, and signed with the device's key. A
//! [`Replica`] takes entries from any device of the account in any order, any number of times, and ends in the same
//! state as every other replica that has the same entries:
//!
//! * **Gaps** are held back (bounded) until the missing entries arrive, so a device's entries apply in order.
//! * **Merging never depends on order.** A reaction, an edit or a delete is kept against its message's id whether the
//!   message has arrived or not, and shows once it has; the latest edit / reaction / setting wins by clock (device id
//!   breaks a tie); a delete beats any edit; read marks only move forward.
//! * **Clones are caught.** Two different entries with the same number from one device (a copied install, a restored
//!   VM) freeze that device: nothing more is accepted from it.
//! * **Unknown kinds** (from a newer app) are kept and passed on byte for byte, never dropped.
//! * **A bad entry never stops the log.** A signed entry of a known kind that does not make sense is quarantined and
//!   reported, and the entries after it still apply.
//! * **A fingerprint** of the merged state lets two devices check they agree.
//!
//! Live chat is not slowed by any of this: it is the bookkeeping that moves alongside.

use crate::{
    canonical::canonical_json,
    devices::DeviceList,
    error::CryptoError,
    identity::{verify_signature, Identity},
    kdf::sha256,
    sealed::Sealed,
};
use privcord_domain::codec;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

/// A clock reading this far ahead of our own is not adopted (risk 14): a device whose clock is wrong cannot drag
/// every other device's clock into the future.
pub const MAX_FUTURE_MS: i64 = 3600 * 1000;
/// Entries held back per device stream, waiting for a gap to fill (risk 58).
pub const MAX_PENDING_PER_STREAM: usize = 1024;
/// Entries held back in all.
pub const MAX_PENDING_TOTAL: usize = 8192;
/// Largest entry body, in bytes of JSON (risk 77).
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// Longest id, conversation or setting key.
const MAX_ID_CHARS: usize = 256;

const ENTRY_TAG: &str = "duocord-entry-v1";

/// A hybrid logical clock reading: wall-clock milliseconds, and a counter for events in the same millisecond. It
/// never goes backwards and is always ahead of every entry seen, so causes sort before their effects.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Hlc {
    pub wall: i64,
    pub count: u32,
}

impl Hlc {
    /// The reading for something happening now.
    pub fn tick(self, now: i64) -> Hlc {
        if now > self.wall {
            Hlc {
                wall: now,
                count: 0,
            }
        } else {
            Hlc {
                wall: self.wall,
                count: self.count.saturating_add(1),
            }
        }
    }

    /// After seeing another device's reading.
    pub fn observe(self, remote: Hlc, now: i64) -> Hlc {
        let cap = now.saturating_add(MAX_FUTURE_MS);
        let remote = if remote.wall > cap {
            Hlc {
                wall: cap,
                count: 0,
            }
        } else {
            remote
        };
        self.max(remote)
    }
}

/// One change, written by one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub device: String,
    /// A device starts a new series after a restore, so a number is never used twice (risk 15).
    pub series: u32,
    /// 1, 2, 3 … within the series.
    pub seq: u64,
    /// Hash of the previous entry in the series ("" for the first).
    pub prev: String,
    pub hlc: Hlc,
    pub kind: String,
    pub body: Value,
    /// The device's signature over everything above, base64url.
    pub sig: String,
}

impl Entry {
    fn payload(
        device: &str,
        series: u32,
        seq: u64,
        prev: &str,
        hlc: Hlc,
        kind: &str,
        body: &Value,
    ) -> Result<Vec<u8>, CryptoError> {
        let v = json!({
            "t": ENTRY_TAG,
            "device": device,
            "series": series,
            "seq": seq,
            "prev": prev,
            "hlc": [hlc.wall, hlc.count],
            "kind": kind,
            "body": body,
        });
        Ok(canonical_json(&v)?.into_bytes())
    }

    fn signed_bytes(&self) -> Result<Vec<u8>, CryptoError> {
        Self::payload(
            &self.device,
            self.series,
            self.seq,
            &self.prev,
            self.hlc,
            &self.kind,
            &self.body,
        )
    }

    /// The entry's identity: a hash over what it says and its signature.
    pub fn hash(&self) -> String {
        let mut bytes = self.signed_bytes().unwrap_or_default();
        bytes.push(b'\n');
        bytes.extend_from_slice(self.sig.as_bytes());
        codec::hex(&sha256(&bytes))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parses an entry from the network. It still has to be checked by a [`Replica`].
    pub fn from_json(text: &str) -> Option<Self> {
        if text.len() > MAX_BODY_BYTES * 2 {
            return None;
        }
        serde_json::from_str(text).ok()
    }
}

/// What a device needs to keep (and save before anything it writes leaves it) to go on writing its log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Writer {
    pub device: String,
    pub series: u32,
    pub next_seq: u64,
    pub last_hash: String,
    pub clock: Hlc,
}

impl Writer {
    pub fn new(device: &str, series: u32) -> Self {
        Self {
            device: device.to_owned(),
            series,
            next_seq: 1,
            last_hash: String::new(),
            clock: Hlc::default(),
        }
    }

    /// Writes the next entry. The caller saves `self` before the entry is sent anywhere.
    pub fn append(
        &mut self,
        key: &Identity,
        kind: &str,
        body: Value,
        now: i64,
    ) -> Result<Entry, CryptoError> {
        if serde_json::to_vec(&body)
            .map_err(|_| CryptoError::Canonical)?
            .len()
            > MAX_BODY_BYTES
        {
            return Err(CryptoError::TooLarge);
        }
        let hlc = self.clock.tick(now);
        let payload = Entry::payload(
            &self.device,
            self.series,
            self.next_seq,
            &self.last_hash,
            hlc,
            kind,
            &body,
        )?;
        let entry = Entry {
            device: self.device.clone(),
            series: self.series,
            seq: self.next_seq,
            prev: self.last_hash.clone(),
            hlc,
            kind: kind.to_owned(),
            body,
            sig: codec::encode(&key.sign_message(&payload)),
        };
        self.next_seq += 1;
        self.last_hash = entry.hash();
        self.clock = hlc;
        Ok(entry)
    }

    /// Keeps this device's clock ahead of what other devices wrote.
    pub fn observe(&mut self, remote: Hlc, now: i64) {
        self.clock = self.clock.observe(remote, now);
    }
}

/* ------------------------------------------------------------------ merge rules */

/// Where an entry sits in the shared order: clock, then device, then number. The same on every device.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Order {
    pub hlc: Hlc,
    pub device: String,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct MessageRec {
    order: Order,
    hash: String,
    /// Sealed in memory (it carries the text); written out as the plain value.
    body: Sealed<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Lww<T> {
    hlc: Hlc,
    device: String,
    value: T,
}

impl<T> Lww<T> {
    fn newer_than(&self, other: &Lww<T>) -> bool {
        (self.hlc, &self.device) > (other.hlc, &other.device)
    }
}

fn lww<T>(e: &Entry, value: T) -> Lww<T> {
    Lww {
        hlc: e.hlc,
        device: e.device.clone(),
        value,
    }
}

fn put_lww<K: Ord, T>(map: &mut BTreeMap<K, Lww<T>>, key: K, next: Lww<T>) {
    match map.get(&key) {
        Some(cur) if !next.newer_than(cur) => {}
        _ => {
            map.insert(key, next);
        }
    }
}

/// Who reacted how, last writer wins; the emoji is sealed in memory (`None`: taken back).
type Reaction = Lww<Sealed<Option<String>>>;

/// One group's signed events as canonical text, each sealed in memory, kept apart by a hash of the text. Written out as
/// the sorted list of texts, so the stored and hashed form is the same as for a plain set.
#[derive(Debug, Clone, Default)]
struct GroupEvents(BTreeMap<String, Sealed<String>>);

impl GroupEvents {
    fn insert(&mut self, text: String) {
        let key = codec::hex(&sha256(text.as_bytes()));
        self.0.entry(key).or_insert_with(|| Sealed::new(&text));
    }

    fn texts(&self) -> Vec<String> {
        let mut t: Vec<String> = self.0.values().filter_map(Sealed::open).collect();
        t.sort();
        t
    }
}

impl PartialEq for GroupEvents {
    fn eq(&self, other: &Self) -> bool {
        self.texts() == other.texts()
    }
}

impl Serialize for GroupEvents {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.texts().serialize(s)
    }
}

impl<'de> Deserialize<'de> for GroupEvents {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut g = GroupEvents::default();
        for t in BTreeSet::<String>::deserialize(d)? {
            g.insert(t);
        }
        Ok(g)
    }
}

/// The merged result of every applied entry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    messages: BTreeMap<String, MessageRec>,
    edits: BTreeMap<String, Lww<Sealed<String>>>,
    deleted: BTreeSet<String>,
    hidden: BTreeSet<String>,
    /// Messages read on some device: read everywhere (risk 21).
    seen: BTreeSet<String>,
    /// message → who → their reaction (`None`: taken back).
    reactions: BTreeMap<String, BTreeMap<String, Reaction>>,
    read: BTreeMap<String, Hlc>,
    settings: BTreeMap<String, Lww<Sealed<Value>>>,
    /// How far our own messages got with the person they went to (1 delivered, 2 read): the furthest any device saw.
    ticks: BTreeMap<String, u8>,
    /// Contacts as last written (by uid).
    contacts: BTreeMap<String, Lww<Sealed<Value>>>,
    /// Each group's signed events (canonical text), all of them: groups merge event by event.
    groups: BTreeMap<String, GroupEvents>,
    /// Snapshots announced (by id), and which devices hold each.
    snapshots: BTreeMap<String, Sealed<Value>>,
    haves: BTreeMap<String, BTreeSet<String>>,
    /// Removals asked for (device → when each request was made), and the requests taken back (`device@at`).
    #[serde(default)]
    removals: BTreeMap<String, BTreeSet<i64>>,
    #[serde(default)]
    cancelled: BTreeSet<String>,
}

/// The kinds this version understands. Anything else is kept in the log as it is.
pub mod kind {
    pub const MESSAGE: &str = "message";
    pub const EDIT: &str = "edit";
    pub const DELETE: &str = "delete";
    pub const HIDE: &str = "hide";
    pub const SEEN: &str = "seen";
    pub const REACT: &str = "react";
    pub const READ: &str = "read";
    pub const SETTING: &str = "setting";
    /// One of our messages reached the other side (`"delivered"`) or was read there (`"read"`).
    pub const TICK: &str = "tick";
    pub const CONTACT: &str = "contact";
    pub const GROUP: &str = "group";
    /// A snapshot of the merged state up to a cut (§7): `{ id, upto }`; the snapshot itself travels apart.
    pub const SNAPSHOT: &str = "snapshot";
    /// The writing device holds a snapshot and everything up to its cut: `{ snapshot }`.
    pub const HAVE: &str = "have";
    /// Signed by the account: a stretch of one device's log that can never be recovered (risk 49).
    pub const LOST: &str = "lost";
    /// Signed by the account: an entry a buggy release wrote is replaced (or voided); entries never change (risk 123).
    pub const CORRECTION: &str = "correction";
    /// A device of the account is to be removed: `{ device, at }`. It takes effect a day later unless taken back.
    pub const REMOVE: &str = "remove";
    /// A removal taken back: `{ device, at }` (the `at` of the request it cancels).
    pub const REMOVE_CANCEL: &str = "remove-cancel";

    /// Every kind this version understands. A snapshot is made only when nothing else is in the log (risk 47).
    pub const ALL: &[&str] = &[
        MESSAGE,
        EDIT,
        DELETE,
        HIDE,
        SEEN,
        REACT,
        READ,
        SETTING,
        TICK,
        CONTACT,
        GROUP,
        SNAPSHOT,
        HAVE,
        LOST,
        CORRECTION,
        REMOVE,
        REMOVE_CANCEL,
    ];
}

fn id_field<'a>(body: &'a Value, name: &str) -> Result<&'a str, &'static str> {
    let s = body
        .get(name)
        .and_then(Value::as_str)
        .ok_or("missing field")?;
    if s.is_empty() || s.chars().count() > MAX_ID_CHARS {
        return Err("bad id");
    }
    Ok(s)
}

/// A message body without what it says or carries: the text, a file's description and the reference to its pieces.
fn scrubbed(body: &Value) -> Value {
    let mut v = body.clone();
    if let Some(o) = v.as_object_mut() {
        if o.contains_key("text") {
            o.insert("text".into(), Value::String(String::new()));
        }
        o.remove("file");
        o.remove("media");
    }
    v
}

impl State {
    /// A deleted or hidden message keeps its place (so everything that refers to it still lines up) but not its content,
    /// and nothing it was edited to.
    fn scrub(&mut self, id: &str) {
        if let Some(rec) = self.messages.get_mut(id) {
            if let Some(b) = rec.body.open() {
                rec.body = Sealed::new(&scrubbed(&b));
            }
        }
        self.edits.remove(id);
    }

    /// Applies one entry. `Err` means the entry is quarantined (it changed nothing).
    fn apply(&mut self, e: &Entry, hash: &str) -> Result<(), &'static str> {
        let order = Order {
            hlc: e.hlc,
            device: e.device.clone(),
            seq: e.seq,
        };
        let b = &e.body;
        match e.kind.as_str() {
            kind::MESSAGE => {
                let id = id_field(b, "id")?.to_owned();
                id_field(b, "conv")?;
                // Once deleted or hidden, what it said and carried is not kept (the entry itself stays in the log until a
                // snapshot lets it go).
                let gone = self.deleted.contains(&id) || self.hidden.contains(&id);
                let rec = MessageRec {
                    order,
                    hash: hash.to_owned(),
                    body: Sealed::new(&if gone { scrubbed(b) } else { b.clone() }),
                };
                // The same id written twice (only a broken or hostile device does that): keep one, the same one
                // everywhere.
                match self.messages.get(&id) {
                    Some(cur) if cur.hash <= rec.hash => {}
                    _ => {
                        self.messages.insert(id, rec);
                    }
                }
            }
            kind::EDIT => {
                let target = id_field(b, "target")?.to_owned();
                if self.deleted.contains(&target) || self.hidden.contains(&target) {
                    return Ok(());
                }
                let text = b
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or("missing text")?
                    .to_owned();
                put_lww(&mut self.edits, target, lww(e, Sealed::new(&text)));
            }
            kind::DELETE => {
                let target = id_field(b, "target")?.to_owned();
                self.scrub(&target);
                self.deleted.insert(target);
            }
            kind::SEEN => {
                self.seen.insert(id_field(b, "target")?.to_owned());
            }
            kind::HIDE => {
                let target = id_field(b, "target")?.to_owned();
                self.scrub(&target);
                self.hidden.insert(target);
            }
            kind::REACT => {
                let target = id_field(b, "target")?.to_owned();
                let by = id_field(b, "by")?.to_owned();
                let emoji = match b.get("emoji") {
                    Some(Value::String(s)) if !s.is_empty() && s.chars().count() <= 16 => {
                        Some(s.clone())
                    }
                    Some(Value::Null) | None => None,
                    _ => return Err("bad emoji"),
                };
                put_lww(
                    self.reactions.entry(target).or_default(),
                    by,
                    lww(e, Sealed::new(&emoji)),
                );
            }
            kind::READ => {
                let conv = id_field(b, "conv")?.to_owned();
                let upto: Hlc =
                    serde_json::from_value(b.get("upto").cloned().ok_or("missing upto")?)
                        .map_err(|_| "bad upto")?;
                let cur = self.read.entry(conv).or_default();
                *cur = (*cur).max(upto);
            }
            kind::TICK => {
                let target = id_field(b, "target")?.to_owned();
                let level = match b.get("state").and_then(Value::as_str) {
                    Some("delivered") => 1,
                    Some("read") => 2,
                    _ => return Err("bad tick"),
                };
                let cur = self.ticks.entry(target).or_default();
                *cur = (*cur).max(level);
            }
            kind::CONTACT => {
                let uid = id_field(b, "uid")?.to_owned();
                put_lww(&mut self.contacts, uid, lww(e, Sealed::new(b)));
            }
            kind::GROUP => {
                let id = id_field(b, "id")?.to_owned();
                let events = b
                    .get("events")
                    .and_then(Value::as_array)
                    .ok_or("missing events")?;
                let set = self.groups.entry(id).or_default();
                for ev in events {
                    set.insert(canonical_json(ev).map_err(|_| "bad event")?);
                }
            }
            kind::SNAPSHOT => {
                let id = id_field(b, "id")?.to_owned();
                self.snapshots.insert(id, Sealed::new(b));
            }
            kind::HAVE => {
                let id = id_field(b, "snapshot")?.to_owned();
                self.haves.entry(id).or_default().insert(e.device.clone());
            }
            // Handled by the replica (they need the account's key); nothing to merge.
            kind::LOST | kind::CORRECTION => {}
            kind::REMOVE => {
                let device = id_field(b, "device")?.to_owned();
                let at = b.get("at").and_then(Value::as_i64).ok_or("missing at")?;
                self.removals.entry(device).or_default().insert(at);
            }
            kind::REMOVE_CANCEL => {
                let device = id_field(b, "device")?;
                let at = b.get("at").and_then(Value::as_i64).ok_or("missing at")?;
                self.cancelled.insert(format!("{device}@{at}"));
            }
            kind::SETTING => {
                let key = id_field(b, "key")?.to_owned();
                let value = b.get("value").cloned().ok_or("missing value")?;
                put_lww(&mut self.settings, key, lww(e, Sealed::new(&value)));
            }
            _ => {}
        }
        Ok(())
    }

    /// The messages of a conversation as they should be shown: in the shared order, deleted and hidden ones left out,
    /// each with its latest text.
    pub fn conversation(&self, conv: &str) -> Vec<(String, String)> {
        let mut rows: Vec<(&Order, &String, &MessageRec)> = self
            .messages
            .iter()
            .filter(|(id, m)| {
                m.body
                    .open()
                    .is_some_and(|b| b.get("conv").and_then(Value::as_str) == Some(conv))
                    && !self.deleted.contains(*id)
                    && !self.hidden.contains(*id)
            })
            .map(|(id, m)| (&m.order, id, m))
            .collect();
        rows.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        rows.into_iter()
            .map(|(_, id, m)| {
                let text = self
                    .edits
                    .get(id)
                    .and_then(|e| e.value.open())
                    .or_else(|| {
                        m.body
                            .open()
                            .and_then(|b| b.get("text").and_then(Value::as_str).map(str::to_owned))
                    })
                    .unwrap_or_default();
                (id.clone(), text)
            })
            .collect()
    }

    /// Who reacted with what to a message (only once the message is here and not deleted).
    pub fn reactions(&self, message: &str) -> BTreeMap<String, String> {
        if !self.messages.contains_key(message) || self.deleted.contains(message) {
            return BTreeMap::new();
        }
        self.reactions
            .get(message)
            .into_iter()
            .flatten()
            .filter_map(|(by, r)| r.value.open().flatten().map(|e| (by.clone(), e)))
            .collect()
    }

    pub fn is_deleted(&self, message: &str) -> bool {
        self.deleted.contains(message)
    }

    pub fn is_seen(&self, message: &str) -> bool {
        self.seen.contains(message)
    }

    pub fn is_hidden(&self, message: &str) -> bool {
        self.hidden.contains(message)
    }

    /// Every reaction recorded for a message, removed ones (`None`) included, whether the message has arrived or not.
    pub fn reactions_raw(&self, message: &str) -> Vec<(String, Option<String>)> {
        self.reactions
            .get(message)
            .into_iter()
            .flatten()
            .map(|(by, r)| (by.clone(), r.value.open().flatten()))
            .collect()
    }

    /// How far one of our messages got: 1 delivered, 2 read.
    pub fn tick(&self, message: &str) -> Option<u8> {
        self.ticks.get(message).copied()
    }

    /// Removals asked for and not taken back: device → when asked (the earliest request still standing, so a later
    /// one cannot push a removal back).
    pub fn pending_removals(&self) -> BTreeMap<String, i64> {
        self.removals
            .iter()
            .filter_map(|(d, ats)| {
                ats.iter()
                    .find(|at| !self.cancelled.contains(&format!("{d}@{at}")))
                    .map(|at| (d.clone(), *at))
            })
            .collect()
    }

    /// Every message, as written (its body), by id.
    pub fn message_bodies(&self) -> impl Iterator<Item = (&String, Value)> {
        self.messages
            .iter()
            .filter_map(|(id, m)| m.body.open().map(|b| (id, b)))
    }

    pub fn deleted(&self) -> &BTreeSet<String> {
        &self.deleted
    }

    pub fn hidden(&self) -> &BTreeSet<String> {
        &self.hidden
    }

    pub fn contacts(&self) -> impl Iterator<Item = Value> + '_ {
        self.contacts.values().filter_map(|c| c.value.open())
    }

    /// Each group's events, parsed back.
    pub fn groups(&self) -> impl Iterator<Item = (&String, Vec<Value>)> {
        self.groups.iter().map(|(id, evs)| {
            (
                id,
                evs.texts()
                    .iter()
                    .filter_map(|t| serde_json::from_str(t).ok())
                    .collect(),
            )
        })
    }

    /// Every message that has reactions recorded, for applying a snapshot.
    pub fn reacted(&self) -> impl Iterator<Item = &String> {
        self.reactions.keys()
    }

    /// Messages with a tick, or read, for applying a snapshot.
    pub fn ticked(&self) -> impl Iterator<Item = &String> {
        self.ticks.keys()
    }

    pub fn seen_all(&self) -> &BTreeSet<String> {
        &self.seen
    }

    /// A snapshot announced in the log, and the devices that hold it.
    pub fn snapshot_note(&self, id: &str) -> Option<Value> {
        self.snapshots.get(id).and_then(Sealed::open)
    }

    pub fn holders(&self, id: &str) -> BTreeSet<String> {
        self.haves.get(id).cloned().unwrap_or_default()
    }

    /// The newest snapshot announced (by its cut's size), if any.
    pub fn latest_snapshot(&self) -> Option<(&String, Value)> {
        self.snapshots
            .iter()
            .filter_map(|(id, v)| v.open().map(|v| (id, v)))
            .max_by_key(|(id, v)| {
                let n: u64 = v
                    .get("upto")
                    .and_then(Value::as_object)
                    .map(|o| o.values().filter_map(Value::as_u64).sum())
                    .unwrap_or(0);
                (n, (*id).clone())
            })
    }

    pub fn read_upto(&self, conv: &str) -> Option<Hlc> {
        self.read.get(conv).copied()
    }

    pub fn setting(&self, key: &str) -> Option<Value> {
        self.settings.get(key).and_then(|s| s.value.open())
    }

    /// A hash of the merged state (risk 10). Text is normalised first, so the same words typed on two platforms
    /// count as the same (risks 57, 116).
    pub fn fingerprint(&self) -> String {
        // ponytail: recomputed over the whole state each time; an incremental hash tree when histories get large
        // (risk 115).
        // Every key of the state is a string, so this cannot fail; if it ever did, the fingerprint says so rather than
        // making every state look the same.
        let Ok(value) = serde_json::to_value(self).map(normalise) else {
            return "unserialisable".into();
        };
        codec::hex(&sha256(
            canonical_json(&value).unwrap_or_default().as_bytes(),
        ))
    }
}

fn normalise(v: Value) -> Value {
    match v {
        Value::String(s) => Value::String(s.nfc().collect()),
        Value::Array(a) => Value::Array(a.into_iter().map(normalise).collect()),
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, v)| (k.nfc().collect(), normalise(v)))
                .collect(),
        ),
        // Map keys that are tuples serialise as arrays inside strings only for JSON objects; everything else is kept.
        other => other,
    }
}

/* ------------------------------------------------------------------ replica */

/// Why an entry was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Signed by a device that is not in the account's list.
    UnknownDevice,
    /// The device was caught writing two different entries under one number; it must be linked again.
    Frozen,
    BadSignature,
    Malformed,
    TooLarge,
    /// Proof of a cloned or forked device: this entry conflicts with one already held.
    Clone,
    /// Too much is already waiting for gaps to fill; it will be asked for again later.
    PendingFull,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Applied (with any held-back entries it unblocked).
    Applied,
    /// Already held.
    Duplicate,
    /// Held back until the entries before it arrive.
    Pending,
    /// Taken into the log, but it made no sense and changed nothing.
    Quarantined(&'static str),
    Refused(Refused),
}

#[derive(Debug, Clone, Default)]
struct Stream {
    have: u64,
    /// The newest wall-clock time of an entry taken from this stream.
    last_wall: i64,
    last_hash: String,
    pending: BTreeMap<u64, HeldEntry>,
}

/// An entry as the replica keeps it in memory: the body (the text, the contact, the group events) sealed, the rest as it
/// is. It reads back as the same `Entry`, so what is sent, signed and hashed does not change.
#[derive(Debug, Clone, PartialEq)]
struct HeldEntry {
    device: String,
    series: u32,
    seq: u64,
    prev: String,
    hlc: Hlc,
    kind: String,
    body: Sealed<Value>,
    sig: String,
}

impl HeldEntry {
    fn new(e: &Entry) -> Self {
        Self {
            device: e.device.clone(),
            series: e.series,
            seq: e.seq,
            prev: e.prev.clone(),
            hlc: e.hlc,
            kind: e.kind.clone(),
            body: Sealed::new(&e.body),
            sig: e.sig.clone(),
        }
    }

    /// `None` only if sealing was impossible when it was made (then the entry could not be kept safely).
    fn open(&self) -> Option<Entry> {
        Some(Entry {
            device: self.device.clone(),
            series: self.series,
            seq: self.seq,
            prev: self.prev.clone(),
            hlc: self.hlc,
            kind: self.kind.clone(),
            body: self.body.open()?,
            sig: self.sig.clone(),
        })
    }
}

/// One device's view of the account's logs and their merged state.
#[derive(Debug, Clone)]
pub struct Replica {
    devices: DeviceList,
    /// Devices vouched for by another list of the same account (an install restored from the phrase keeps its own
    /// list until device lists are merged).
    vouched: BTreeMap<String, crate::devices::DeviceEntry>,
    streams: BTreeMap<(String, u32), Stream>,
    frozen: BTreeSet<String>,
    // ponytail: the log is kept in memory here; the engine keeps it in its database.
    log: BTreeMap<(String, u32, u64), HeldEntry>,
    hashes: BTreeMap<(String, u32, u64), String>,
    quarantined: Vec<(String, &'static str)>,
    pending_total: usize,
    state: State,
    /// The latest clock reading seen, for this device's own writer.
    pub clock: Hlc,
    /// Entries of kinds this version does not understand (while there are any, no snapshot is made: risk 47).
    unknown: usize,
    /// The snapshot the log starts from once older entries were pruned.
    base: Option<Snapshot>,
    /// Snapshots held whole (checked against an announcement in the log), by id.
    held: BTreeMap<String, Snapshot>,
    /// The account's signing key, for lost markers and corrections.
    account_sign: Option<crate::devices::PublicJwkAlias>,
    /// Corrections in force: entry hash → its replacement body (`Null`: voided).
    corrections: BTreeMap<String, Sealed<Value>>,
    /// Entries given up on by lost markers.
    lost: u64,
}

/// After a lost marker, the next entry of that stream is taken whatever it names as the one before it.
const LOST_LINK: &str = "*lost*";
/// What the account signs on a lost marker or a correction.
const RULING_TAG: &str = "duocord-account-ruling-v1";

/// The merged state at a cut of the log (§7). Announced in the log by a `snapshot` entry carrying its id, which is a hash
/// of all of it, so it can travel apart (it is large) and still be checked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The device that made it.
    pub by: String,
    /// The cut: for each stream, the last entry it covers.
    pub upto: Summary,
    /// For each stream, the hash of that last entry (so the log goes on from the cut).
    pub heads: BTreeMap<String, String>,
    pub state: State,
}

impl Snapshot {
    pub fn id(&self) -> String {
        let v = serde_json::to_value(self).unwrap_or(Value::Null);
        codec::hex(&sha256(canonical_json(&v).unwrap_or_default().as_bytes()))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }
}

/// A body the account signs (a lost marker or a correction): the body with `accountSig` added.
pub fn account_ruling(account: &Identity, body: &Value) -> Result<Value, CryptoError> {
    let text = format!("{RULING_TAG}\n{}", canonical_json(body)?);
    let mut out = body.clone();
    if let Some(o) = out.as_object_mut() {
        o.insert(
            "accountSig".into(),
            json!(codec::encode(&account.sign_message(text.as_bytes()))),
        );
    }
    Ok(out)
}

/// Whether `body` carries a valid `accountSig` (see [`account_ruling`]).
pub fn ruling_valid(body: &Value, account: &crate::devices::PublicJwkAlias) -> bool {
    let Some(sig) = body
        .get("accountSig")
        .and_then(Value::as_str)
        .and_then(codec::decode)
    else {
        return false;
    };
    let mut rest = body.clone();
    if let Some(o) = rest.as_object_mut() {
        o.remove("accountSig");
    }
    canonical_json(&rest)
        .is_ok_and(|t| verify_signature(account, &sig, format!("{RULING_TAG}\n{t}").as_bytes()))
}

/// How far each device's log a replica holds: "device/series" → highest number held with no gap before it.
pub type Summary = BTreeMap<String, u64>;

impl Replica {
    /// `devices` must already be verified against the account.
    pub fn new(devices: DeviceList) -> Self {
        Self {
            devices,
            vouched: BTreeMap::new(),
            streams: BTreeMap::new(),
            frozen: BTreeSet::new(),
            log: BTreeMap::new(),
            hashes: BTreeMap::new(),
            quarantined: Vec::new(),
            pending_total: 0,
            state: State::default(),
            clock: Hlc::default(),
            unknown: 0,
            base: None,
            held: BTreeMap::new(),
            account_sign: None,
            corrections: BTreeMap::new(),
            lost: 0,
        }
    }

    /// The account's public signing key: lost markers and corrections are believed only when it signed them.
    pub fn set_account(&mut self, sign: crate::devices::PublicJwkAlias) {
        self.account_sign = Some(sign);
    }

    /* ---- snapshots (§7) */

    /// A snapshot of the merged state as it is now, by device `by`. `None` while the log holds a kind this version does
    /// not understand (it would be lost from the snapshot: risk 47).
    pub fn make_snapshot(&self, by: &str) -> Option<Snapshot> {
        if self.unknown > 0 {
            return None;
        }
        let mut upto = Summary::new();
        let mut heads = BTreeMap::new();
        for ((d, series), s) in &self.streams {
            if s.have > 0 {
                upto.insert(format!("{d}/{series}"), s.have);
                heads.insert(format!("{d}/{series}"), s.last_hash.clone());
            }
        }
        Some(Snapshot {
            by: by.to_owned(),
            upto,
            heads,
            state: self.state.clone(),
        })
    }

    /// Whether the log (applied or still waiting) announces a snapshot with this id, in an entry signed by its author.
    fn announced(&self, id: &str) -> bool {
        self.state.snapshots.contains_key(id)
            || self.streams.values().any(|s| {
                s.pending.values().any(|e| {
                    e.kind == kind::SNAPSHOT
                        && e.body
                            .open()
                            .is_some_and(|b| b.get("id").and_then(Value::as_str) == Some(id))
                })
            })
    }

    /// Keeps a snapshot this replica now holds whole. Only one the log announces is kept.
    pub fn hold(&mut self, snap: Snapshot) -> bool {
        let id = snap.id();
        if !self.announced(&id) {
            return false;
        }
        self.held.insert(id, snap);
        true
    }

    pub fn held(&self, id: &str) -> Option<&Snapshot> {
        self.held.get(id)
    }

    /// Whether this replica has every entry a snapshot covers.
    pub fn covers(&self, snap: &Snapshot) -> bool {
        let ours = self.summary();
        snap.upto
            .iter()
            .all(|(k, v)| ours.get(k).copied().unwrap_or(0) >= *v)
    }

    /// Drops the entries a snapshot covers, once every active device of the account holds it and everything up to its
    /// cut (risks 12, 13): nobody will ever need them again, and a new device starts from the snapshot. Returns how
    /// many entries went.
    pub fn prune(&mut self, id: &str) -> usize {
        let Some(snap) = self.held.get(id).cloned() else {
            return 0;
        };
        let holders = self.state.holders(id);
        if !self.covers(&snap) || !self.devices.active().all(|d| holders.contains(&d.id)) {
            return 0;
        }
        let before = self.log.len();
        let cut = |d: &str, series: u32| {
            snap.upto
                .get(&format!("{d}/{series}"))
                .copied()
                .unwrap_or(0)
        };
        self.log
            .retain(|(d, series, seq), _| *seq > cut(d, *series));
        self.hashes
            .retain(|(d, series, seq), _| *seq > cut(d, *series));
        self.base = Some(snap);
        before - self.log.len()
    }

    pub fn base(&self) -> Option<&Snapshot> {
        self.base.as_ref()
    }

    /// The snapshot a replica with summary `theirs` needs first, when entries it lacks were pruned here.
    pub fn needs_base(&self, theirs: &Summary) -> Option<&Snapshot> {
        let base = self.base.as_ref()?;
        base.upto
            .iter()
            .any(|(k, v)| theirs.get(k).copied().unwrap_or(0) < *v)
            .then_some(base)
    }

    /// Starts from a snapshot (a new device, joining after older entries were pruned). Only one the log announces, and
    /// only when this replica holds nothing past its cut.
    pub fn install(&mut self, snap: Snapshot, now: i64) -> bool {
        let id = snap.id();
        if !self.announced(&id) {
            return false;
        }
        let ours = self.summary();
        let behind = ours
            .iter()
            .all(|(k, v)| *v <= snap.upto.get(k).copied().unwrap_or(0));
        if !behind {
            return false;
        }
        self.set_base(snap, now);
        true
    }

    /// Starts a fresh replica from this device's own stored base snapshot (at startup; it was checked when kept).
    pub fn restore(&mut self, snap: Snapshot, now: i64) {
        self.set_base(snap, now);
    }

    fn set_base(&mut self, snap: Snapshot, now: i64) {
        let id = snap.id();
        for (k, have) in &snap.upto {
            let Some((d, series)) = k.rsplit_once('/') else {
                continue;
            };
            let Ok(series) = series.parse::<u32>() else {
                continue;
            };
            let s = self.streams.entry((d.to_owned(), series)).or_default();
            s.have = *have;
            s.last_hash = snap.heads.get(k).cloned().unwrap_or_default();
            let dropped = s.pending.len();
            s.pending.retain(|seq, _| *seq > *have);
            self.pending_total -= dropped - s.pending.len();
        }
        let cut = snap.upto.clone();
        self.log.retain(|(d, series, seq), _| {
            *seq > cut.get(&format!("{d}/{series}")).copied().unwrap_or(0)
        });
        self.hashes.retain(|(d, series, seq), _| {
            *seq > cut.get(&format!("{d}/{series}")).copied().unwrap_or(0)
        });
        self.state = snap.state.clone();
        self.held.insert(id, snap.clone());
        self.base = Some(snap);
        let keys: Vec<(String, u32)> = self.streams.keys().cloned().collect();
        for key in keys {
            self.drain(&key, now);
        }
    }

    /// When a device last wrote anything this replica took (by the entries' clocks), if ever.
    pub fn last_heard(&self, device: &str) -> Option<i64> {
        self.streams
            .iter()
            .filter(|((d, _), s)| d == device && s.last_wall > 0)
            .map(|(_, s)| s.last_wall)
            .max()
    }

    /// Gaps entries are waiting on: `(device, series, first missing, last missing)` per stream.
    pub fn gaps(&self) -> Vec<(String, u32, u64, u64)> {
        self.streams
            .iter()
            .filter_map(|((d, series), s)| {
                let next = *s.pending.keys().next()?;
                (next > s.have + 1).then(|| (d.clone(), *series, s.have + 1, next - 1))
            })
            .collect()
    }

    /// How many entries lost markers have given up on.
    pub fn lost_count(&self) -> u64 {
        self.lost
    }

    /// Snapshot announcements in the log, applied or still waiting (a new device sees them before it can apply them).
    pub fn snapshot_notes(&self) -> Vec<Value> {
        let mut out: Vec<Value> = self
            .state
            .snapshots
            .values()
            .filter_map(Sealed::open)
            .collect();
        for s in self.streams.values() {
            for e in s.pending.values() {
                if e.kind == kind::SNAPSHOT {
                    out.extend(e.body.open());
                }
            }
        }
        out
    }

    /// Applies, in order, whatever was waiting on a stream and can go now.
    fn drain(&mut self, key: &(String, u32), now: i64) {
        loop {
            let stream = self.streams.entry(key.clone()).or_default();
            let next = stream.have + 1;
            let Some(held) = stream.pending.remove(&next) else {
                break;
            };
            self.pending_total -= 1;
            let Some(e) = held.open() else {
                break;
            };
            let h = e.hash();
            if let Outcome::Refused(_) = self.take(e, h, now) {
                break;
            }
        }
    }

    /// A lost marker: a stretch of one device's log is closed (the device is gone with it), and what comes after it
    /// is taken.
    fn apply_lost(&mut self, b: &Value, now: i64) {
        let (Some(device), Some(series), Some(from), Some(to)) = (
            b.get("device").and_then(Value::as_str),
            b.get("series")
                .and_then(Value::as_u64)
                .and_then(|s| u32::try_from(s).ok()),
            b.get("from").and_then(Value::as_u64),
            b.get("to").and_then(Value::as_u64),
        ) else {
            return;
        };
        let key = (device.to_owned(), series);
        let s = self.streams.entry(key.clone()).or_default();
        if from == 0 || to < from || s.have + 1 < from || s.have >= to {
            return;
        }
        self.lost += to - s.have;
        s.have = to;
        s.last_hash = LOST_LINK.into();
        let dropped = s.pending.len();
        s.pending.retain(|seq, _| *seq > to);
        self.pending_total -= dropped - s.pending.len();
        self.drain(&key, now);
    }

    /// The state again from the base and every entry held, with the corrections in force (a correction arrived).
    fn rebuild(&mut self) {
        let mut state = self
            .base
            .as_ref()
            .map(|b| b.state.clone())
            .unwrap_or_default();
        for (k, held) in &self.log {
            let Some(hash) = self.hashes.get(k) else {
                continue;
            };
            let Some(e) = held.open() else {
                continue;
            };
            match self.corrections.get(hash).map(Sealed::open) {
                Some(Some(Value::Null) | None) => {}
                Some(Some(body)) => {
                    let mut fixed = e.clone();
                    fixed.body = body;
                    let _ = state.apply(&fixed, hash);
                }
                None => {
                    let _ = state.apply(&e, hash);
                }
            }
        }
        self.state = state;
    }

    /// A newer device list (already checked with [`DeviceList::accepts`]).
    pub fn set_devices(&mut self, devices: DeviceList) {
        self.devices = devices;
    }

    /// Trusts the devices of another list of this account. The caller has verified it against the account's keys.
    pub fn vouch(&mut self, list: &DeviceList) {
        for d in &list.devices {
            if self.devices.get(&d.id).is_none() {
                self.vouched.insert(d.id.clone(), d.clone());
            }
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn quarantined(&self) -> &[(String, &'static str)] {
        &self.quarantined
    }

    pub fn is_frozen(&self, device: &str) -> bool {
        self.frozen.contains(device)
    }

    /// Takes an entry from anywhere: this device's own writer, another device, a relay.
    pub fn receive(&mut self, e: Entry, now: i64) -> Outcome {
        if let Err(r) = self.check(&e) {
            return Outcome::Refused(r);
        }
        let key = (e.device.clone(), e.series);
        let hash = e.hash();
        let stream = self.streams.entry(key.clone()).or_default();
        if e.seq <= stream.have {
            return if self.hashes.get(&(key.0.clone(), key.1, e.seq)) == Some(&hash) {
                Outcome::Duplicate
            } else {
                self.freeze(&e.device)
            };
        }
        if let Some(waiting) = stream.pending.get(&e.seq) {
            return if waiting.open().map(|w| w.hash()).as_deref() == Some(hash.as_str()) {
                Outcome::Duplicate
            } else {
                self.freeze(&e.device)
            };
        }
        if e.seq > stream.have + 1 {
            if stream.pending.len() >= MAX_PENDING_PER_STREAM
                || self.pending_total >= MAX_PENDING_TOTAL
            {
                return Outcome::Refused(Refused::PendingFull);
            }
            stream.pending.insert(e.seq, HeldEntry::new(&e));
            self.pending_total += 1;
            return Outcome::Pending;
        }
        let first = self.take(e, hash, now);
        // Entries that were waiting for this one (a held-back entry that does not chain on is a fork: `take` freezes
        // the device).
        self.drain(&key, now);
        first
    }

    fn check(&self, e: &Entry) -> Result<(), Refused> {
        if self.frozen.contains(&e.device) {
            return Err(Refused::Frozen);
        }
        if e.seq == 0 || e.kind.is_empty() || e.kind.len() > 64 || e.device.len() > 64 {
            return Err(Refused::Malformed);
        }
        if serde_json::to_vec(&e.body)
            .map(|b| b.len())
            .unwrap_or(usize::MAX)
            > MAX_BODY_BYTES
        {
            return Err(Refused::TooLarge);
        }
        let device = self
            .devices
            .get(&e.device)
            .or_else(|| self.vouched.get(&e.device))
            .ok_or(Refused::UnknownDevice)?;
        let sig = codec::decode(&e.sig).ok_or(Refused::BadSignature)?;
        let payload = e.signed_bytes().map_err(|_| Refused::Malformed)?;
        if !verify_signature(&device.sign, &sig, &payload) {
            return Err(Refused::BadSignature);
        }
        Ok(())
    }

    /// Applies the next entry of a stream.
    fn take(&mut self, e: Entry, hash: String, now: i64) -> Outcome {
        let key = (e.device.clone(), e.series);
        let stream = self.streams.entry(key.clone()).or_default();
        if e.prev != stream.last_hash && stream.last_hash != LOST_LINK {
            // Signed by the device, numbered right, but not the entry we hold before it: two histories.
            return self.freeze(&e.device);
        }
        stream.have = e.seq;
        stream.last_hash = hash.clone();
        stream.last_wall = stream.last_wall.max(e.hlc.wall);
        self.clock = self.clock.observe(e.hlc, now);
        if !kind::ALL.contains(&e.kind.as_str()) {
            self.unknown += 1;
        }
        let ruling = matches!(e.kind.as_str(), kind::LOST | kind::CORRECTION)
            && self
                .account_sign
                .as_ref()
                .is_some_and(|a| ruling_valid(&e.body, a));
        let result = match self.corrections.get(&hash).map(Sealed::open) {
            Some(Some(Value::Null) | None) => Ok(()),
            Some(Some(body)) => {
                let mut fixed = e.clone();
                fixed.body = body;
                self.state.apply(&fixed, &hash)
            }
            None => self.state.apply(&e, &hash),
        };
        let ruling_body = ruling.then(|| (e.kind.clone(), e.body.clone()));
        self.hashes
            .insert((key.0.clone(), key.1, e.seq), hash.clone());
        self.log.insert((key.0, key.1, e.seq), HeldEntry::new(&e));
        match ruling_body {
            Some((k, b)) if k == kind::LOST => self.apply_lost(&b, now),
            Some((_, b)) => {
                if let (Some(target), Some(replace)) =
                    (b.get("target").and_then(Value::as_str), b.get("replace"))
                {
                    self.corrections
                        .insert(target.to_owned(), Sealed::new(replace));
                    self.rebuild();
                }
            }
            None => {}
        }
        match result {
            Ok(()) => Outcome::Applied,
            Err(reason) => {
                self.quarantined.push((hash, reason));
                Outcome::Quarantined(reason)
            }
        }
    }

    fn freeze(&mut self, device: &str) -> Outcome {
        self.frozen.insert(device.to_owned());
        // Whatever it had waiting goes too.
        for ((d, _), s) in self.streams.iter_mut() {
            if d == device {
                self.pending_total -= s.pending.len();
                s.pending.clear();
            }
        }
        Outcome::Refused(Refused::Clone)
    }

    /// How far this replica has every device's log.
    pub fn summary(&self) -> Summary {
        self.streams
            .iter()
            .filter(|(_, s)| s.have > 0)
            .map(|((d, series), s)| (format!("{d}/{series}"), s.have))
            .collect()
    }

    /// Entries this replica holds that a replica with summary `theirs` is missing, oldest first per stream.
    pub fn missing_for(&self, theirs: &Summary, limit: usize) -> Vec<Entry> {
        let mut out = Vec::new();
        for ((d, series), s) in &self.streams {
            let from = theirs.get(&format!("{d}/{series}")).copied().unwrap_or(0) + 1;
            for seq in from..=s.have {
                if out.len() >= limit {
                    return out;
                }
                if let Some(e) = self
                    .log
                    .get(&(d.clone(), *series, seq))
                    .and_then(HeldEntry::open)
                {
                    out.push(e);
                }
            }
        }
        out
    }

    /// Every entry held, for a device catching up from nothing.
    pub fn entries(&self) -> impl Iterator<Item = Entry> + '_ {
        self.log.values().filter_map(HeldEntry::open)
    }

    /// How many entries are held (without opening any).
    pub fn entry_count(&self) -> usize {
        self.log.len()
    }

    /// Whether the log still holds a deletion. The entries it deletes carry their text until a snapshot lets them go.
    pub fn has_deletions(&self) -> bool {
        self.log
            .values()
            .any(|e| e.kind == kind::DELETE || e.kind == kind::HIDE)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::devices::DeviceEntry;
    use crate::rng::OsEntropy;

    struct Device {
        key: Identity,
        writer: Writer,
        replica: Replica,
    }

    /// An account with `n` devices, each with a writer and a replica.
    fn account(n: usize) -> (Identity, Vec<Device>, DeviceList) {
        let account = Identity::generate(&mut OsEntropy).unwrap();
        let keys: Vec<Identity> = (0..n)
            .map(|_| Identity::generate(&mut OsEntropy).unwrap())
            .collect();
        let mut list = DeviceList::first(&account, &keys[0], "D0", 1).unwrap();
        for (i, k) in keys.iter().enumerate().skip(1) {
            list = list
                .with_added(&account, DeviceEntry::new(k, &format!("D{i}"), 1).unwrap())
                .unwrap();
        }
        let devices = keys
            .into_iter()
            .map(|key| {
                let id = crate::devices::device_id(&key.sign_public(), &key.dh_public()).unwrap();
                Device {
                    writer: Writer::new(&id, 1),
                    key,
                    replica: Replica::new(list.clone()),
                }
            })
            .collect();
        (account, devices, list)
    }

    fn write(d: &mut Device, kind: &str, body: Value, now: i64) -> Entry {
        d.writer.observe(d.replica.clock, now);
        let e = d.writer.append(&d.key, kind, body, now).unwrap();
        assert_eq!(d.replica.receive(e.clone(), now), Outcome::Applied);
        e
    }

    /// Writes an entry without applying it anywhere.
    fn raw(d: &mut Device, kind: &str, body: Value, now: i64) -> Entry {
        d.writer.append(&d.key, kind, body, now).unwrap()
    }

    /// A tiny deterministic generator, so a failing run can be replayed from its seed.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    #[test]
    fn nothing_of_a_message_is_held_in_the_clear_in_memory() {
        let (_, mut ds, list) = account(2);
        let secret = "the key is under the blue stone";
        let first = write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "m1", "conv": "c-secret-chat", "text": secret}),
            10,
        );
        let waiting = write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "m2", "conv": "c-secret-chat", "text": secret}),
            11,
        );
        let mut r = Replica::new(list);
        // One entry waits for the one before it, the other is applied: both places are checked.
        assert_eq!(r.receive(waiting, 100), Outcome::Pending);
        assert_eq!(r.receive(first, 100), Outcome::Applied);
        let clear = |bytes: &[u8]| {
            [secret.as_bytes(), b"c-secret-chat".as_slice()]
                .iter()
                .any(|needle| bytes.windows(needle.len()).any(|w| w == *needle))
        };
        assert!(r.log.values().all(|e| !clear(e.body.raw())));
        assert!(r.state.messages.values().all(|m| !clear(m.body.raw())));
        // It still reads back, and what is sent to another device is the plain entry.
        assert_eq!(r.state().conversation("c-secret-chat").len(), 2);
        assert_eq!(r.missing_for(&Summary::new(), 10).len(), 2);
    }

    #[test]
    fn entries_apply_in_order_even_when_they_arrive_out_of_order() {
        let (_, mut ds, list) = account(2);
        let a: Vec<Entry> = (0..5)
            .map(|i| {
                write(
                    &mut ds[0],
                    kind::MESSAGE,
                    json!({"id": format!("m{i}"), "conv": "c", "text": format!("t{i}")}),
                    10 + i,
                )
            })
            .collect();
        let mut r = Replica::new(list);
        assert_eq!(r.receive(a[3].clone(), 100), Outcome::Pending);
        assert_eq!(r.receive(a[1].clone(), 100), Outcome::Pending);
        assert_eq!(r.state().conversation("c"), vec![]);
        assert_eq!(r.receive(a[0].clone(), 100), Outcome::Applied);
        assert_eq!(r.state().conversation("c").len(), 2);
        assert_eq!(r.receive(a[2].clone(), 100), Outcome::Applied);
        assert_eq!(r.receive(a[4].clone(), 100), Outcome::Applied);
        assert_eq!(r.receive(a[2].clone(), 100), Outcome::Duplicate);
        let texts: Vec<String> = r
            .state()
            .conversation("c")
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        assert_eq!(texts, ["t0", "t1", "t2", "t3", "t4"]);
        assert_eq!(r.state().fingerprint(), ds[0].replica.state().fingerprint());
    }

    #[test]
    fn strangers_forgeries_and_tampering_are_refused() {
        let (_, mut ds, list) = account(1);
        let e = write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "m", "conv": "c", "text": "hi"}),
            1,
        );
        let mut r = Replica::new(list.clone());

        let mut changed = e.clone();
        changed.body = json!({"id": "m", "conv": "c", "text": "send money"});
        assert_eq!(
            r.receive(changed, 2),
            Outcome::Refused(Refused::BadSignature)
        );

        let stranger = Identity::generate(&mut OsEntropy).unwrap();
        let sid =
            crate::devices::device_id(&stranger.sign_public(), &stranger.dh_public()).unwrap();
        let fake = Writer::new(&sid, 1)
            .append(&stranger, kind::MESSAGE, json!({"id": "x", "conv": "c"}), 1)
            .unwrap();
        assert_eq!(r.receive(fake, 2), Outcome::Refused(Refused::UnknownDevice));

        // Signed by a stranger but claiming to be our device.
        let mut claimed = Writer::new(&ds[0].writer.device, 1)
            .append(&stranger, kind::MESSAGE, json!({"id": "y", "conv": "c"}), 1)
            .unwrap();
        assert_eq!(
            r.receive(claimed.clone(), 2),
            Outcome::Refused(Refused::BadSignature)
        );
        claimed.seq = 0;
        assert!(matches!(r.receive(claimed, 2), Outcome::Refused(_)));

        assert_eq!(r.receive(e, 2), Outcome::Applied);
    }

    #[test]
    fn a_cloned_device_is_caught_and_frozen() {
        let (_, mut ds, list) = account(2);
        // A copy of device 0's install (same keys, same counter) writes its own entry 1.
        let mut clone = ds[0].writer.clone();
        let original = write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "a", "conv": "c", "text": "real"}),
            1,
        );
        let copy = clone
            .append(
                &ds[0].key,
                kind::MESSAGE,
                json!({"id": "b", "conv": "c", "text": "copy"}),
                1,
            )
            .unwrap();

        let mut r = Replica::new(list);
        assert_eq!(r.receive(original, 2), Outcome::Applied);
        assert_eq!(r.receive(copy, 2), Outcome::Refused(Refused::Clone));
        assert!(r.is_frozen(&ds[0].writer.device));
        let later = write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "z", "conv": "c"}),
            3,
        );
        assert_eq!(r.receive(later, 4), Outcome::Refused(Refused::Frozen));
        // Other devices are unaffected.
        let other = write(
            &mut ds[1],
            kind::MESSAGE,
            json!({"id": "o", "conv": "c"}),
            3,
        );
        assert_eq!(r.receive(other, 4), Outcome::Applied);
    }

    #[test]
    fn a_fork_found_among_held_back_entries_freezes_the_device() {
        let (_, mut ds, list) = account(1);
        let e1 = write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "1", "conv": "c"}),
            1,
        );
        let mut fork = ds[0].writer.clone();
        let e2 = write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "2", "conv": "c"}),
            2,
        );
        let _ = e2;
        // A different entry 2 (another history), and an entry 3 built on it.
        let f2 = fork
            .append(
                &ds[0].key,
                kind::MESSAGE,
                json!({"id": "2x", "conv": "c"}),
                2,
            )
            .unwrap();
        let f3 = fork
            .append(
                &ds[0].key,
                kind::MESSAGE,
                json!({"id": "3x", "conv": "c"}),
                3,
            )
            .unwrap();
        let mut r = Replica::new(list);
        assert_eq!(r.receive(f3, 5), Outcome::Pending);
        assert_eq!(r.receive(e1, 5), Outcome::Applied);
        assert_eq!(r.receive(f2, 5), Outcome::Applied);
        // Now the real entry 2 shows up: the device signed two different second entries.
        let real2 = ds[0]
            .replica
            .entries()
            .find(|e| e.seq == 2)
            .unwrap()
            .clone();
        assert_eq!(r.receive(real2, 5), Outcome::Refused(Refused::Clone));
        assert!(r.is_frozen(&ds[0].writer.device));
    }

    #[test]
    fn a_bad_entry_is_quarantined_and_the_log_goes_on() {
        let (_, mut ds, list) = account(1);
        let bad = raw(&mut ds[0], kind::REACT, json!({"target": "m"}), 1);
        let good = raw(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "m", "conv": "c", "text": "after"}),
            2,
        );
        let mut r = Replica::new(list);
        assert_eq!(r.receive(bad, 3), Outcome::Quarantined("missing field"));
        assert_eq!(r.receive(good, 3), Outcome::Applied);
        assert_eq!(r.state().conversation("c").len(), 1);
        assert_eq!(r.quarantined().len(), 1);
    }

    #[test]
    fn unknown_kinds_are_kept_and_passed_on_unchanged() {
        let (_, mut ds, list) = account(2);
        let future = raw(
            &mut ds[0],
            "poll",
            json!({"question": "lunch?", "options": ["a", "b"]}),
            1,
        );
        let mut r = Replica::new(list.clone());
        assert_eq!(r.receive(future.clone(), 2), Outcome::Applied);
        // Passed on to a third replica byte for byte, and it still verifies there.
        let forwarded: Vec<Entry> = r.missing_for(&Summary::new(), 10);
        assert_eq!(forwarded, vec![future.clone()]);
        let mut third = Replica::new(list);
        assert_eq!(
            third.receive(Entry::from_json(&forwarded[0].to_json()).unwrap(), 3),
            Outcome::Applied
        );
        assert_eq!(third.entries().next().unwrap().hash(), future.hash());
    }

    #[test]
    fn delete_beats_edit_reactions_wait_for_their_message_and_read_marks_only_move_forward() {
        let (_, mut ds, list) = account(2);
        let msg = write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "m", "conv": "c", "text": "hello"}),
            1,
        );
        let react = write(
            &mut ds[1],
            kind::REACT,
            json!({"target": "m", "by": "me", "emoji": "👍"}),
            2,
        );
        let edit = write(
            &mut ds[1],
            kind::EDIT,
            json!({"target": "m", "text": "hello!"}),
            3,
        );
        let read_late = write(
            &mut ds[0],
            kind::READ,
            json!({"conv": "c", "upto": {"wall": 50, "count": 0}}),
            4,
        );
        let read_early = write(
            &mut ds[1],
            kind::READ,
            json!({"conv": "c", "upto": {"wall": 20, "count": 0}}),
            5,
        );
        let delete = write(&mut ds[0], kind::DELETE, json!({"target": "m"}), 6);

        let mut r = Replica::new(list);
        // The reaction and the edit arrive before the message.
        for e in [react.clone(), edit.clone()] {
            assert_eq!(r.receive(e, 9), Outcome::Applied);
        }
        assert!(r.state().reactions("m").is_empty());
        assert_eq!(r.receive(msg, 9), Outcome::Applied);
        assert_eq!(
            r.state().reactions("m").get("me").map(String::as_str),
            Some("👍")
        );
        assert_eq!(
            r.state().conversation("c"),
            vec![("m".to_owned(), "hello!".to_owned())]
        );
        for e in [read_late, read_early] {
            r.receive(e, 9);
        }
        assert_eq!(r.state().read_upto("c"), Some(Hlc { wall: 50, count: 0 }));
        assert_eq!(r.receive(delete, 9), Outcome::Applied);
        assert!(r.state().conversation("c").is_empty());
        assert!(r.state().reactions("m").is_empty());
    }

    #[test]
    fn a_deleted_message_keeps_no_text_and_every_order_agrees() {
        let (_, mut ds, list) = account(2);
        let msg = raw(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "m", "conv": "c", "text": "secret", "file": {"name": "a.png"}, "media": {"k": 1}}),
            1,
        );
        let edit = raw(
            &mut ds[0],
            kind::EDIT,
            json!({"target": "m", "text": "edited secret"}),
            2,
        );
        let hide = raw(&mut ds[1], kind::HIDE, json!({"target": "m"}), 3);
        let mut texts = Vec::new();
        let mut prints = Vec::new();
        for order in [[0usize, 1, 2], [2, 0, 1], [1, 2, 0], [2, 1, 0]] {
            let all = [msg.clone(), edit.clone(), hide.clone()];
            let mut r = Replica::new(list.clone());
            for i in order {
                r.receive(all[i].clone(), 9);
            }
            let body = r.state().message_bodies().next().map(|(_, b)| b);
            texts.push(body.clone());
            prints.push(r.state().fingerprint());
            let shown = serde_json::to_string(r.state()).unwrap_or_default();
            assert!(!shown.contains("secret"), "text left in the state: {shown}");
            assert!(!shown.contains("a.png"), "file left in the state");
        }
        assert!(
            prints.windows(2).all(|w| w[0] == w[1]),
            "orders disagree: {prints:?}"
        );
        assert!(
            texts.windows(2).all(|w| w[0] == w[1]),
            "bodies disagree: {texts:?}"
        );
    }

    #[test]
    fn a_clock_far_in_the_future_does_not_drag_the_others_along() {
        let c = Hlc::default().observe(
            Hlc {
                wall: 10_000_000_000,
                count: 0,
            },
            1_000,
        );
        assert_eq!(c.wall, 1_000 + MAX_FUTURE_MS);
        let t = Hlc { wall: 5, count: 3 };
        assert_eq!(t.tick(5), Hlc { wall: 5, count: 4 });
        assert_eq!(t.tick(9), Hlc { wall: 9, count: 0 });
        assert_eq!(t.tick(1), Hlc { wall: 5, count: 4 }, "never backwards");
    }

    #[test]
    fn the_fingerprint_ignores_how_text_was_encoded() {
        let (_, mut ds, list) = account(2);
        // "é" as one code point on one device, as e + accent on the other.
        write(
            &mut ds[0],
            kind::SETTING,
            json!({"key": "nick:a", "value": "Ren\u{e9}"}),
            1,
        );
        let mut a = Replica::new(list.clone());
        let mut b = Replica::new(list);
        a.receive(ds[0].replica.entries().next().unwrap().clone(), 2);
        let mut state_b = a.state().clone();
        state_b.settings.get_mut("nick:a").unwrap().value = Sealed::new(&json!("Rene\u{301}"));
        assert_eq!(a.state().fingerprint(), state_b.fingerprint());
        b.receive(ds[0].replica.entries().next().unwrap().clone(), 2);
        assert_eq!(a.state().fingerprint(), b.state().fingerprint());
    }

    #[test]
    fn a_snapshot_lets_every_device_prune_and_a_new_device_start_from_it() {
        let (_, mut ds, list) = account(3);
        // Devices 0 and 1 write; device 2 has not joined yet.
        for i in 0..20 {
            write(
                &mut ds[i % 2],
                kind::MESSAGE,
                json!({"id": format!("m{i}"), "conv": "c", "text": format!("t{i}")}),
                10 + i as i64,
            );
        }
        write(
            &mut ds[0],
            kind::REACT,
            json!({"target": "m1", "by": "x", "emoji": "👍"}),
            40,
        );
        let entries: Vec<Entry> = ds
            .iter()
            .take(2)
            .flat_map(|d| d.replica.entries().collect::<Vec<_>>())
            .collect();
        for d in ds.iter_mut().take(2) {
            for e in &entries {
                d.replica.receive(e.clone(), 50);
            }
        }
        assert_eq!(
            ds[0].replica.state().fingerprint(),
            ds[1].replica.state().fingerprint()
        );

        // Device 0 makes a snapshot and announces it.
        let snap = ds[0]
            .replica
            .make_snapshot(&ds[0].writer.device.clone())
            .unwrap();
        let id = snap.id();
        let upto = snap.upto.clone();
        let note = write(
            &mut ds[0],
            kind::SNAPSHOT,
            json!({"id": id, "upto": upto}),
            60,
        );
        ds[1].replica.receive(note.clone(), 61);
        // Nobody prunes before everyone holds it.
        assert!(ds[0].replica.hold(snap.clone()));
        assert!(ds[1]
            .replica
            .hold(Snapshot::from_json(&snap.to_json()).unwrap()));
        assert_eq!(ds[0].replica.prune(&id), 0);
        // A snapshot the log does not announce is not taken.
        let mut forged = snap.clone();
        forged.by = "someone".into();
        assert!(!ds[1].replica.hold(forged));

        // Device 2 joins the log now, from nothing: it holds the snapshot too, then says so with the others.
        ds[2].replica.receive(note.clone(), 62);
        assert!(ds[2].replica.install(snap.clone(), 63));
        let haves: Vec<Entry> = (0..3)
            .map(|i| write(&mut ds[i], kind::HAVE, json!({"snapshot": id}), 70))
            .collect();
        for d in ds.iter_mut() {
            for h in &haves {
                d.replica.receive(h.clone(), 71);
            }
        }
        let before = ds[0].replica.entries().count();
        let pruned = ds[0].replica.prune(&id);
        assert!(pruned >= 21, "{pruned} of {before}");
        assert!(ds[1].replica.prune(&id) > 0);
        // The state is untouched by pruning; the three agree.
        assert_eq!(
            ds[0].replica.state().fingerprint(),
            ds[1].replica.state().fingerprint()
        );
        assert_eq!(
            ds[0].replica.state().fingerprint(),
            ds[2].replica.state().fingerprint()
        );
        assert_eq!(
            ds[2]
                .replica
                .state()
                .reactions("m1")
                .get("x")
                .map(String::as_str),
            Some("👍")
        );

        // The log goes on from the cut: a new entry chains onto the pruned ones everywhere.
        let after = write(
            &mut ds[1],
            kind::MESSAGE,
            json!({"id": "late", "conv": "c", "text": "after"}),
            80,
        );
        assert_eq!(ds[0].replica.receive(after.clone(), 81), Outcome::Applied);
        assert_eq!(ds[2].replica.receive(after, 81), Outcome::Applied);
        // A replica still at the start needs the snapshot first.
        assert!(ds[0].replica.needs_base(&Summary::new()).is_some());
        let _ = list;
    }

    #[test]
    fn no_snapshot_while_the_log_holds_a_kind_this_version_does_not_know() {
        let (_, mut ds, _) = account(1);
        write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "m", "conv": "c", "text": "t"}),
            1,
        );
        assert!(ds[0].replica.make_snapshot("d").is_some());
        write(&mut ds[0], "from-the-future", json!({"x": 1}), 2);
        assert!(
            ds[0].replica.make_snapshot("d").is_none(),
            "it would be lost from the snapshot"
        );
    }

    #[test]
    fn a_lost_marker_signed_by_the_account_closes_a_gap_and_nothing_else_does() {
        let (account, mut ds, list) = account(2);
        let e1 = write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "a", "conv": "c", "text": "1"}),
            1,
        );
        let _lost = write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "b", "conv": "c", "text": "2"}),
            2,
        );
        let e3 = write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "c", "conv": "c", "text": "3"}),
            3,
        );
        let mut r = Replica::new(list);
        r.set_account(account.sign_public());
        r.receive(e1, 4);
        assert_eq!(r.receive(e3, 4), Outcome::Pending);
        let body = json!({"device": ds[0].writer.device, "series": 1, "from": 2, "to": 2});
        // Not signed by the account: nothing happens.
        let fake = write(&mut ds[1], kind::LOST, body.clone(), 5);
        r.receive(fake, 6);
        assert!(r.state().conversation("c").len() == 1);
        let marker = write(
            &mut ds[1],
            kind::LOST,
            account_ruling(&account, &body).unwrap(),
            7,
        );
        r.receive(marker, 8);
        let ids: Vec<String> = r
            .state()
            .conversation("c")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            ids,
            ["a", "c"],
            "the gap is closed and what came after it is applied"
        );
    }

    #[test]
    fn a_correction_signed_by_the_account_replaces_or_voids_an_entry() {
        let (account, mut ds, list) = account(1);
        let bad = write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "m", "conv": "c", "text": "garbled"}),
            1,
        );
        let mut r = Replica::new(list);
        r.set_account(account.sign_public());
        r.receive(bad.clone(), 2);
        let fix = account_ruling(
            &account,
            &json!({"target": bad.hash(), "replace": {"id": "m", "conv": "c", "text": "fixed"}}),
        )
        .unwrap();
        r.receive(write(&mut ds[0], kind::CORRECTION, fix, 3), 4);
        assert_eq!(
            r.state().conversation("c"),
            vec![("m".to_owned(), "fixed".to_owned())]
        );
        let void =
            account_ruling(&account, &json!({"target": bad.hash(), "replace": null})).unwrap();
        r.receive(write(&mut ds[0], kind::CORRECTION, void, 5), 6);
        assert!(r.state().conversation("c").is_empty());
        // The entry itself never changed.
        assert!(r.entries().any(|e| e.hash() == bad.hash()));
    }

    #[test]
    fn the_fingerprint_sees_reactions() {
        let (_, mut ds, list) = account(1);
        write(
            &mut ds[0],
            kind::MESSAGE,
            json!({"id": "m", "conv": "c", "text": "hi"}),
            1,
        );
        let mut a = Replica::new(list.clone());
        let mut b = Replica::new(list);
        for e in ds[0].replica.entries().collect::<Vec<_>>() {
            a.receive(e.clone(), 2);
            b.receive(e, 2);
        }
        let thumbs = write(
            &mut ds[0],
            kind::REACT,
            json!({"target": "m", "by": "x", "emoji": "👍"}),
            3,
        );
        a.receive(thumbs, 4);
        let mut other = a.state().clone();
        let empty = b.state().fingerprint();
        // `assert!(a != b)`, not `assert_ne!`: a failing assert_ne would print both fingerprints.
        assert!(
            a.state().fingerprint() != empty,
            "a reaction changes the fingerprint"
        );
        // A different reaction gives a different fingerprint too.
        other
            .reactions
            .get_mut("m")
            .unwrap()
            .get_mut("x")
            .unwrap()
            .value = Sealed::new(&Some("😂".to_owned()));
        assert_ne!(a.state().fingerprint(), other.fingerprint());
    }

    #[test]
    fn summaries_tell_each_side_exactly_what_the_other_is_missing() {
        let (_, mut ds, _) = account(2);
        for i in 0..4 {
            write(
                &mut ds[0],
                kind::MESSAGE,
                json!({"id": format!("a{i}"), "conv": "c"}),
                i,
            );
        }
        for i in 0..3 {
            write(
                &mut ds[1],
                kind::MESSAGE,
                json!({"id": format!("b{i}"), "conv": "c"}),
                i,
            );
        }
        let (left, right) = ds.split_at_mut(1);
        let (a, b) = (&mut left[0], &mut right[0]);
        let for_b = a.replica.missing_for(&b.replica.summary(), 100);
        let for_a = b.replica.missing_for(&a.replica.summary(), 100);
        assert_eq!((for_b.len(), for_a.len()), (4, 3));
        for e in for_b {
            assert_eq!(b.replica.receive(e, 10), Outcome::Applied);
        }
        for e in for_a {
            assert_eq!(a.replica.receive(e, 10), Outcome::Applied);
        }
        assert_eq!(a.replica.summary(), b.replica.summary());
        assert_eq!(
            a.replica.state().fingerprint(),
            b.replica.state().fingerprint()
        );
        assert!(a.replica.missing_for(&b.replica.summary(), 100).is_empty());
    }

    /// Many devices doing many things at once, every entry delivered to every replica in a random order, some twice,
    /// with partial syncs between devices along the way: every replica ends in the same state (risk 10).
    #[test]
    fn replicas_converge_whatever_the_order() {
        for seed in 1..=40u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let (_, mut ds, list) = account(3);
            let mut all: Vec<Entry> = Vec::new();
            let mut now = 1_000i64;
            for step in 0..120 {
                now += rng.below(3) as i64;
                let who = rng.below(ds.len());
                let target = format!("m{}", rng.below(step + 1));
                let (k, body) = match rng.below(7) {
                    0 | 1 => (
                        kind::MESSAGE,
                        json!({"id": format!("m{step}"), "conv": format!("c{}", rng.below(2)), "text": format!("t{step}")}),
                    ),
                    2 => (
                        kind::EDIT,
                        json!({"target": target, "text": format!("e{step}")}),
                    ),
                    3 => (kind::DELETE, json!({"target": target})),
                    4 => (
                        kind::REACT,
                        json!({"target": target, "by": format!("u{}", rng.below(3)), "emoji": if rng.below(4) == 0 { Value::Null } else { json!("❤") }}),
                    ),
                    5 => (
                        kind::READ,
                        json!({"conv": "c0", "upto": {"wall": now - rng.below(50) as i64, "count": 0}}),
                    ),
                    _ => (
                        kind::SETTING,
                        json!({"key": format!("mute:{}", rng.below(3)), "value": rng.below(2) == 0}),
                    ),
                };
                all.push(write(&mut ds[who], k, body, now));
                // Now and then two devices sync with each other part of the way.
                if rng.below(5) == 0 {
                    let (x, y) = (rng.below(ds.len()), rng.below(ds.len()));
                    if x != y {
                        let summary = ds[y].replica.summary();
                        let give = ds[x].replica.missing_for(&summary, rng.below(10) + 1);
                        for e in give {
                            ds[y].replica.receive(e, now);
                        }
                    }
                }
            }
            // Finally every replica gets everything, shuffled, with duplicates.
            let mut expected: Option<String> = None;
            for d in ds.iter_mut() {
                let mut mixed = all.clone();
                mixed.extend(all.iter().take(rng.below(all.len())).cloned());
                for i in (1..mixed.len()).rev() {
                    mixed.swap(i, rng.below(i + 1));
                }
                for e in mixed {
                    assert!(
                        !matches!(
                            d.replica.receive(e, now),
                            Outcome::Refused(_) | Outcome::Quarantined(_)
                        ),
                        "seed {seed}"
                    );
                }
                assert_eq!(
                    d.replica.summary().values().sum::<u64>(),
                    all.len() as u64,
                    "seed {seed}"
                );
                let fp = d.replica.state().fingerprint();
                assert_eq!(expected.get_or_insert(fp.clone()), &fp, "seed {seed}");
            }
            let mut fresh = Replica::new(list);
            for e in all.iter().rev() {
                fresh.receive(e.clone(), now);
            }
            assert_eq!(Some(fresh.state().fingerprint()), expected, "seed {seed}");
        }
    }
}
