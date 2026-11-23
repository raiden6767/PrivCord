//! Keeping the devices of one account in step (docs/design/syncengine.md §5, step 4).
//!
//! * **Recording.** Every change this install stores that matters to the person (a message sent or received, a delete,
//!   a reaction, a contact, who is muted) becomes an entry in this device's sync log, automatically: the storage port is
//!   wrapped by [`Recorder`], so no code path can store a change and forget to sync it.
//! * **The self-mailbox.** Entries go to the account's self-mailbox on the relays (`privcord_crypto::selfbox`): a
//!   weekly label and key only the account's devices can compute. Relays keep them for 7 days, so a device that comes
//!   online later still finds them.
//! * **Catching up.** On start and every few minutes, a device posts a short *hello* with how far it has each device's
//!   log; any other device that has more answers with exactly the missing entries. That also covers anything older
//!   than the relays keep.
//! * **Applying.** Entries from another device go through the sync log's merge rules, then into storage the way a
//!   restored backup does (nothing that exists is replaced), with the recorder quiet so they are not recorded again.
//!
//! Which devices count: this install's own device list, and the lists of other installs of the same account that
//! verify against the account's keys (an install restored from the recovery phrase). Linking a device by QR code adds
//! to the same list once its screens are in.
//!
//! "Sent means synced": entries are posted on the first tick after they are written, before the store-and-forward
//! pass of the same tick. ponytail: a message delivered over a direct link in the second before that tick is ahead of
//! its sync copy; holding delivery for the copy comes with the delivery rework (step 5).

use super::{Core, StorageOp, StoredContactState};
use crate::{api::EngineEvent, error::ErrorCode};
use privcord_crypto::{
    card::ContactCard,
    devices::DeviceList,
    identity::Identity,
    selfbox::{device_inbox, open_from_device, seal_to_device, signed_by, SelfBox, SyncKey},
    synclog::{kind, Entry, Replica, Snapshot, Summary, Writer},
};
use privcord_domain::{
    blob::SealedBlob, message::DeliveryState, signaling::PutKind, ConversationId, MessageId, UserId,
};
use privcord_import::{backup::read_bundle, import, FileSource, Source};
use privcord_ports::{
    OutboxEntry, Storage, StorageError, StoredContact, StoredFile, StoredMessage, StoredReaction,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

const KV_WRITER: &str = "sync.writer";
pub(super) const KV_INDEX: &str = "sync.index";
const KV_LISTS: &str = "sync.lists";
const KV_PUSHED: &str = "sync.pushed";
/// The account's sync key (not the account key): `{ "v": version, "key": base64 }`.
const KV_SYNC_KEY: &str = "sync.key";
/// A device without the sync key asks the others for it this often.
const ASK_KEY_EVERY_MS: i64 = 10 * 60 * 1000;
/// Set on an install that restored an existing account from its recovery phrase.
const KV_JOINED: &str = "sync.joined";
/// The first sync key (version 0), kept after it is replaced: see `SyncState::first_key`.
const KV_FIRST_KEY: &str = "sync.key0";

/// What the account signs for a sync key after the first: its version and a hash of it.
pub(super) fn key_body(key: &SyncKey) -> Value {
    json!({
        "t": "sync-key",
        "v": key.version,
        "hash": privcord_crypto::kdf::sha256_hex(&key.to_bytes()),
    })
}
/// Contacts and groups are not merge-rule kinds of the log; the engine applies them (unknown kinds pass through).
const KIND_CONTACT: &str = "contact";
/// One group's signed events (membership, name): merged event by event, each one checked again.
const KIND_GROUP: &str = "group";
/// Set once this install has put the history it had before sync into its log.
const KV_BACKFILLED: &str = "sync.backfilled";
const SETTING_MUTED: &str = "muted";
const HELLO_EVERY_MS: i64 = 10 * 60 * 1000;
/// An answer to a hello is sent at most this often (several devices may ask at once).
const ANSWER_GAP_MS: i64 = 20 * 1000;
const RELAY_TTL_SEC: u32 = 7 * 24 * 3600;
/// Plaintext per blob, below what relays accept.
const BATCH_BYTES: usize = 24 * 1024;
/// Entries one answer carries at most; the rest follow on the next hello.
const ANSWER_MAX: usize = 400;
/// Entries per frame over a direct link between our devices (a frame may be up to 128 KiB).
const LINK_BATCH_BYTES: usize = 96 * 1024;

pub(super) fn entry_key(device: &str, series: u32, seq: u64) -> String {
    format!("sync.e.{device}.{series}.{seq}")
}

/* ------------------------------------------------------------------ recorder */

/// The storage port, wrapped: changes worth syncing are queued for the sync log as they are stored.
pub(crate) struct Recorder {
    inner: Arc<dyn Storage>,
    queue: Mutex<Vec<StorageOp>>,
    quiet: AtomicBool,
}

impl Recorder {
    pub(crate) fn new(inner: Arc<dyn Storage>) -> Self {
        Self {
            inner,
            queue: Mutex::new(Vec::new()),
            quiet: AtomicBool::new(false),
        }
    }

    fn take(&self) -> Vec<StorageOp> {
        self.queue
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }

    fn quiet(&self, on: bool) {
        self.quiet.store(on, Ordering::SeqCst);
    }

    /// Queues changes as if they had just been stored (the history that predates sync).
    fn push(&self, ops: Vec<StorageOp>) {
        if let Ok(mut q) = self.queue.lock() {
            q.extend(ops);
        }
    }
}

impl Storage for Recorder {
    fn apply(&self, batch: &[StorageOp]) -> Result<(), StorageError> {
        self.inner.apply(batch)?;
        if self.quiet.load(Ordering::SeqCst) {
            return Ok(());
        }
        let worth: Vec<StorageOp> = batch
            .iter()
            .filter(|op| match op {
                StorageOp::InsertMessage(m) => {
                    !matches!(m.conversation, ConversationId::Channel(_))
                }
                StorageOp::Tombstone(_)
                | StorageOp::DeleteMessage(_)
                | StorageOp::SetReaction { .. }
                | StorageOp::PutContact(_) => true,
                StorageOp::SetKv { key, .. } => {
                    key == super::social::KV_MUTED
                        || key == super::KV_GROUPS
                        || key.starts_with(super::prefs::PREFIX)
                }
                // A message read here, or one of ours delivered or read there (the engine decides below which).
                StorageOp::SetDelivery {
                    read_at: Some(_), ..
                }
                | StorageOp::SetDelivery {
                    state: DeliveryState::Delivered | DeliveryState::Read,
                    ..
                } => true,
                _ => false,
            })
            .cloned()
            .collect();
        if !worth.is_empty() {
            if let Ok(mut q) = self.queue.lock() {
                // ponytail: bounded so a long run without an identity cannot grow it forever.
                if q.len() < 100_000 {
                    q.extend(worth);
                }
            }
        }
        Ok(())
    }
    fn contacts(&self) -> Result<Vec<StoredContact>, StorageError> {
        self.inner.contacts()
    }
    fn messages(&self) -> Result<Vec<StoredMessage>, StorageError> {
        self.inner.messages()
    }
    fn outbox(&self) -> Result<Vec<OutboxEntry>, StorageError> {
        self.inner.outbox()
    }
    fn kv(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        self.inner.kv(key)
    }
    fn file(&self, id: &MessageId) -> Result<Option<StoredFile>, StorageError> {
        self.inner.file(id)
    }
    fn file_info(&self, id: &MessageId) -> Result<Option<privcord_ports::FileInfo>, StorageError> {
        self.inner.file_info(id)
    }
    fn reactions(&self) -> Result<Vec<StoredReaction>, StorageError> {
        self.inner.reactions()
    }
    fn deleted(&self) -> Result<Vec<MessageId>, StorageError> {
        self.inner.deleted()
    }
}

/* ------------------------------------------------------------------ state */

pub(super) struct SyncState {
    device: Identity,
    writer: Writer,
    replica: Replica,
    /// Other devices of this account seen so far.
    others: BTreeSet<String>,
    /// Device lists of other installs, as received (kept to vouch for them again after a restart).
    lists: Vec<String>,
    subscribed: Vec<String>,
    last_hello: i64,
    last_answer: i64,
    /// Our own entries up to this number have been posted.
    pushed: u64,
    /// How many events of each group were last put in the log.
    group_counts: BTreeMap<String, usize>,
    /// The account's sync key; `None` on a linked device until another device of the account hands it over.
    key: Option<SyncKey>,
    /// The account's signature over the key (every key after the first, which comes from the account key itself).
    key_sig: Option<String>,
    /// The key before the last replacement (a device still on it is found on the local network, and given the new one).
    prev_key: Option<SyncKey>,
    /// The first key (derived from the account key): an install set up from the recovery phrase starts on it, so every
    /// device keeps listening there for device lists only, and hands the current key to a device it did not know.
    first_key: Option<SyncKey>,
    /// This device's inbox (where the sync key is handed over) is being listened on.
    inbox: Option<String>,
    last_ask: i64,
    /// The summary last told to each of our devices over a direct link (said again only when it changes).
    link_told: BTreeMap<String, String>,
}

struct NoFiles;
impl FileSource for NoFiles {
    fn file(&self, _message_id: &str) -> Option<Vec<u8>> {
        None
    }
}

/// On a message entry: written by a device without the account key.
const FIELD_HELP: &str = "byDevice";

impl Core {
    /// Messages a device of ours without the account key wrote to a friend whose app predates device lists: that friend checks only the
    /// account key, so the install holding it sends them (they stay waiting in its outbox until the friend's receipt,
    /// which then reaches the other devices as a tick).
    fn deliver_for_devices(
        &self,
        own: &[privcord_ports::StoredMessage],
        by_device: &BTreeSet<String>,
    ) -> Vec<StorageOp> {
        if self.identity.is_none() {
            return Vec::new();
        }
        let now = self.now();
        own.iter()
            .filter(|m| by_device.contains(m.message.message_id.as_str()))
            .filter(|m| {
                matches!(
                    m.message.delivery_state,
                    DeliveryState::Queued | DeliveryState::Sent
                )
            })
            .filter_map(|m| match &m.conversation {
                ConversationId::Direct(p)
                    if !self.understands_devices(p)
                        && !self
                            .outbox
                            .iter()
                            .any(|o| o.message_id == m.message.message_id) =>
                {
                    Some(StorageOp::PutOutbox(privcord_ports::OutboxEntry {
                        message_id: m.message.message_id.clone(),
                        recipient: p.clone(),
                        attempts: 0,
                        next_try_ms: now,
                    }))
                }
                _ => None,
            })
            .collect()
    }
}

impl SyncState {
    /// This device's own keys.
    pub(super) fn device_key(&self) -> &Identity {
        &self.device
    }

    pub(super) fn replica(&self) -> &Replica {
        &self.replica
    }

    pub(super) fn replica_mut(&mut self) -> &mut Replica {
        &mut self.replica
    }

    /// The account's sync key, once this device has it.
    pub(super) fn sync_key(&self) -> Option<&SyncKey> {
        self.key.as_ref()
    }

    /// The account's device list changed (a device was linked or removed): its devices write entries this device
    /// accepts.
    pub(super) fn devices_changed(&mut self, list: &DeviceList) {
        self.replica.set_devices(list.clone());
        let own = self.writer.device.clone();
        self.others
            .extend(list.active().map(|d| d.id.clone()).filter(|d| d != &own));
        self.others.retain(|d| {
            list.get(d)
                .is_none_or(privcord_crypto::devices::DeviceEntry::active)
        });
    }

    /// The key before the last replacement, if any.
    pub(super) fn prev_key(&self) -> Option<&SyncKey> {
        self.prev_key.as_ref()
    }
}

impl Core {
    pub(super) fn kv_json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.ports
            .storage
            .kv(key)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice(&b).ok())
    }

    /// Loads (or starts) this device's sync log. Runs whenever the identity and device are known.
    pub(super) fn ensure_sync(&mut self, device: Identity) -> Result<(), ErrorCode> {
        let (Some(list), Some(id)) = (self.device_list.clone(), self.device_id.clone()) else {
            self.sync = None;
            return Ok(());
        };
        let now = self.now();
        let writer = self
            .kv_json::<Writer>(KV_WRITER)
            .filter(|w| w.device == id)
            .unwrap_or_else(|| Writer::new(&id, 1));
        let mut replica = Replica::new(list);
        if let Some((sign, _)) = &self.account {
            replica.set_account(sign.clone());
        }
        // Where the log starts, once older entries were pruned: the snapshot kept then.
        let base = self.stored_base();
        if let Some(b) = &base {
            replica.restore(b.clone(), now);
        }
        let mut others = BTreeSet::new();
        let lists: Vec<String> = self.kv_json(KV_LISTS).unwrap_or_default();
        if let Some(account) = &self.identity {
            let (sign, dh) = (account.sign_public(), account.dh_public());
            for text in &lists {
                if let Ok(l) = DeviceList::from_json(text) {
                    if l.verify(&sign, &dh).is_ok() {
                        replica.vouch(&l);
                        others.extend(l.devices.iter().map(|d| d.id.clone()).filter(|d| d != &id));
                    }
                }
            }
        }
        // Every entry after the base is read and checked again at start (the pruned ones are in the base).
        let index: BTreeMap<String, u64> = self.kv_json(KV_INDEX).unwrap_or_default();
        for (stream, have) in &index {
            let Some((dev, series)) = stream.rsplit_once('/') else {
                continue;
            };
            let Ok(series) = series.parse::<u32>() else {
                continue;
            };
            let from = base
                .as_ref()
                .and_then(|b| b.upto.get(stream))
                .copied()
                .unwrap_or(0)
                + 1;
            for seq in from..=*have {
                if let Some(e) = self.kv_json::<Entry>(&entry_key(dev, series, seq)) {
                    replica.receive(e, now);
                }
            }
        }
        let pushed = self.kv_json(KV_PUSHED).unwrap_or(0);
        // The sync key: stored, or (a holder of the account key) derived; a linked device waits to be given it.
        let stored = self.kv_json::<Value>(KV_SYNC_KEY);
        let key_sig = stored
            .as_ref()
            .and_then(|v| v.get("sig")?.as_str().map(str::to_owned));
        let mut key = stored.and_then(|v| {
            let version = u32::try_from(v.get("v")?.as_u64()?).ok()?;
            SyncKey::from_bytes(
                version,
                &privcord_domain::codec::decode(v.get("key")?.as_str()?)?,
            )
        });
        if key.is_none() {
            if let Some(account) = &self.identity {
                let derived = SyncKey::from_account(account)?;
                Self::store_key(&*self.ports.storage, &derived, None);
                key = Some(derived);
            }
        }
        let first_key = match &self.identity {
            Some(account) => Some(SyncKey::from_account(account)?),
            None => key
                .clone()
                .filter(|k| k.version == 0)
                .or_else(|| Self::load_key(&*self.ports.storage, KV_FIRST_KEY)),
        };
        // The other devices of this account's own list.
        if let Some(list) = &self.device_list {
            others.extend(list.active().map(|d| d.id.clone()).filter(|d| d != &id));
        }
        let group_counts = self
            .kv_json::<BTreeMap<String, Vec<Value>>>(super::KV_GROUPS)
            .unwrap_or_default()
            .into_iter()
            .map(|(id, events)| (id, events.len()))
            .collect();
        self.sync = Some(SyncState {
            device,
            writer,
            replica,
            others,
            lists,
            subscribed: Vec::new(),
            last_hello: i64::MIN / 2,
            last_answer: i64::MIN / 2,
            pushed,
            group_counts,
            key,
            key_sig,
            prev_key: None,
            first_key,
            inbox: None,
            last_ask: i64::MIN / 2,
            link_told: BTreeMap::new(),
        });
        Ok(())
    }

    fn store_key(storage: &dyn privcord_ports::Storage, key: &SyncKey, sig: Option<&str>) {
        let mut value =
            json!({ "v": key.version, "key": privcord_domain::codec::encode(&key.to_bytes()) });
        if let (Some(s), Some(o)) = (sig, value.as_object_mut()) {
            o.insert("sig".into(), json!(s));
        }
        let mut ops = vec![StorageOp::SetKv {
            key: KV_SYNC_KEY.into(),
            value: value.to_string().into_bytes(),
        }];
        if key.version == 0 {
            ops.push(StorageOp::SetKv {
                key: KV_FIRST_KEY.into(),
                value: value.to_string().into_bytes(),
            });
        }
        let _ = storage.apply(&ops);
    }

    fn load_key(storage: &dyn privcord_ports::Storage, at: &str) -> Option<SyncKey> {
        let v: Value = serde_json::from_slice(&storage.kv(at).ok()??).ok()?;
        let version = u32::try_from(v.get("v")?.as_u64()?).ok()?;
        SyncKey::from_bytes(
            version,
            &privcord_domain::codec::decode(v.get("key")?.as_str()?)?,
        )
    }

    /// Takes a sync key (a replacement, already checked): stored, used from now on, the old one kept for the local
    /// network.
    pub(super) fn take_sync_key(&mut self, key: SyncKey, sig: Option<String>) {
        Self::store_key(&*self.ports.storage, &key, sig.as_deref());
        if let Some(s) = self.sync.as_mut() {
            if key.version == 0 && s.first_key.is_none() {
                s.first_key = Some(key.clone());
            }
            s.prev_key = s.key.replace(key);
            s.key_sig = sig;
            // Listen on the new key's mailbox from the next tick.
            s.subscribed.clear();
        }
    }

    /// The sync key, for another device of the account: with the list, the account's signature, and the first key.
    pub(super) fn sync_key_message(&self, list: &DeviceList) -> Value {
        let Some(s) = self.sync.as_ref() else {
            return Value::Null;
        };
        let Some(key) = s.key.as_ref() else {
            return Value::Null;
        };
        let mut m = json!({
            "t": "sync-key",
            "v": key.version,
            "key": privcord_domain::codec::encode(&key.to_bytes()),
            "list": list.to_json(),
        });
        if let Some(o) = m.as_object_mut() {
            if let Some(sig) = &s.key_sig {
                o.insert("sig".into(), json!(sig));
            }
            if let Some(k0) = s.first_key.as_ref().filter(|_| key.version > 0) {
                o.insert(
                    "k0".into(),
                    json!(privcord_domain::codec::encode(&k0.to_bytes())),
                );
            }
        }
        m
    }

    /// The account's public keys, which every device has (device lists and posts are checked against them).
    pub(super) fn account_public(
        &self,
    ) -> Option<(
        privcord_domain::blob::PublicJwk,
        privcord_domain::blob::PublicJwk,
    )> {
        self.account.clone()
    }

    /// Other devices of this account seen so far (none until another install of it shows up).
    pub fn sync_devices(&self) -> Vec<String> {
        self.sync
            .as_ref()
            .map(|s| s.others.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The merged sync state's fingerprint, for checks.
    pub fn sync_fingerprint(&self) -> Option<String> {
        self.sync.as_ref().map(|s| s.replica.state().fingerprint())
    }

    pub(super) fn index_value(replica: &Replica) -> Vec<u8> {
        serde_json::to_vec(&replica.summary()).unwrap_or_default()
    }

    /// Turns what was stored since the last tick into entries of this device's log.
    fn record(&mut self) {
        let ops = self.recorder.take();
        if ops.is_empty() || self.sync.is_none() {
            return;
        }
        let mut items: Vec<(&'static str, Value)> = Vec::new();
        for op in ops {
            match op {
                StorageOp::InsertMessage(row) => {
                    let mut v = self.row_json(&row);
                    if let Some(o) = v.as_object_mut() {
                        // Who it still has to reach, its reactions and deletion travel separately.
                        for k in ["recipients", "reactions", "deleted"] {
                            o.remove(k);
                        }
                        o.insert("id".into(), json!(row.message.message_id));
                        o.insert("conv".into(), json!(row.conversation.storage_key()));
                        // A file's bytes do not go in the log: its reference does, and the pieces go up apart.
                        if o.contains_key("file") {
                            if let Some(r) = self.media_for_sync(&row.message.message_id) {
                                o.insert("media".into(), json!(r));
                            }
                        }
                        // Ours, written without the account key: friends whose app checks only that key get it
                        // from an install that holds it (`deliver_for_devices`).
                        if self.identity.is_none()
                            && Some(&row.message.sender_id) == self.uid.as_ref()
                        {
                            o.insert(FIELD_HELP.into(), json!(true));
                        }
                    }
                    items.push((kind::MESSAGE, v));
                }
                StorageOp::Tombstone(id) => items.push((kind::DELETE, json!({ "target": id }))),
                StorageOp::DeleteMessage(id) => items.push((kind::HIDE, json!({ "target": id }))),
                StorageOp::SetReaction {
                    message_id,
                    uid,
                    emoji,
                } => items.push((
                    kind::REACT,
                    json!({ "target": message_id, "by": uid, "emoji": emoji }),
                )),
                StorageOp::PutContact(stored) => {
                    if let Some(contact) =
                        ContactCard::decode(&stored.card).filter(|c| c.uid == stored.uid)
                    {
                        let v = Self::contact_json(&StoredContactState { stored, contact });
                        items.push((KIND_CONTACT, v));
                    }
                }
                StorageOp::SetKv { key, value } if key == super::KV_GROUPS => {
                    let groups: BTreeMap<String, Vec<Value>> =
                        serde_json::from_slice(&value).unwrap_or_default();
                    for (id, events) in groups {
                        let known = self
                            .sync
                            .as_ref()
                            .and_then(|s| s.group_counts.get(&id))
                            .copied();
                        if known == Some(events.len()) {
                            continue;
                        }
                        if let Some(s) = self.sync.as_mut() {
                            s.group_counts.insert(id.clone(), events.len());
                        }
                        items.push((KIND_GROUP, json!({ "id": id, "events": events })));
                    }
                }
                StorageOp::SetKv { key, value } if key.starts_with(super::prefs::PREFIX) => {
                    // "pref.pins.v1" travels as the setting "pref.pins".
                    let name = key.strip_suffix(".v1").unwrap_or(&key).to_owned();
                    let v: Value = serde_json::from_slice(&value).unwrap_or(Value::Null);
                    items.push((kind::SETTING, json!({ "key": name, "value": v })));
                }
                StorageOp::SetKv { value, .. } => {
                    let list: Value = serde_json::from_slice(&value).unwrap_or(json!([]));
                    items.push((
                        kind::SETTING,
                        json!({ "key": SETTING_MUTED, "value": list }),
                    ));
                }
                StorageOp::SetDelivery { id, state, read_at } => {
                    // A message from someone else: read here, read everywhere. One of ours: how far it got with them
                    // (the receipt came to the device that sent it; the others learn it here).
                    let theirs = self
                        .messages
                        .meta(&id)
                        .is_some_and(|m| Some(&m.sender_id) != self.uid.as_ref());
                    if theirs {
                        if read_at.is_some() {
                            items.push((kind::SEEN, json!({ "target": id })));
                        }
                    } else {
                        match state {
                            DeliveryState::Delivered => items
                                .push((kind::TICK, json!({ "target": id, "state": "delivered" }))),
                            DeliveryState::Read => {
                                items.push((kind::TICK, json!({ "target": id, "state": "read" })));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        self.append_items(items);
    }

    /// Writes entries of this device's log, saved with its counter and index in one batch.
    pub(super) fn append_items(&mut self, items: Vec<(&'static str, Value)>) {
        let now = self.now();
        let Some(sync) = self.sync.as_mut() else {
            return;
        };
        let mut batch = Vec::new();
        for (k, body) in items {
            sync.writer.observe(sync.replica.clock, now);
            let Ok(e) = sync.writer.append(&sync.device, k, body, now) else {
                continue;
            };
            batch.push(StorageOp::SetKv {
                key: entry_key(&e.device, e.series, e.seq),
                value: serde_json::to_vec(&e).unwrap_or_default(),
            });
            sync.replica.receive(e, now);
        }
        if batch.is_empty() {
            return;
        }
        // The counter is saved with the entries, before any of them leaves this device (risk 15).
        batch.push(StorageOp::SetKv {
            key: KV_WRITER.into(),
            value: serde_json::to_vec(&sync.writer).unwrap_or_default(),
        });
        let _ = self.ports.storage.apply(&batch);
        // Our entries may have let others' through (a lost marker closing a gap): store and apply those, and the index
        // with them.
        self.take_entries(Vec::new(), now);
    }

    /// Posts `payload` to this week's self-mailbox.
    fn post_self(&mut self, payload: &Value) -> bool {
        let now = self.now();
        let (Some(s), Some(own)) = (self.sync.as_ref(), self.device_id.clone()) else {
            return false;
        };
        let Some(key) = s.key.as_ref() else {
            return false;
        };
        let Ok([this_week, _]) = SelfBox::current(key, now) else {
            return false;
        };
        // Signed by this device; the payload says which device that is.
        let mut payload = payload.clone();
        if let Some(o) = payload.as_object_mut() {
            o.insert("device".into(), json!(own));
        }
        let Ok(blob) = this_week.seal(&s.device, &payload, &mut *self.ports.entropy) else {
            return false;
        };
        self.ports
            .transport
            .signal_put(
                &this_week.label,
                &blob.encode(),
                PutKind::Msg,
                RELAY_TTL_SEC,
            )
            .is_ok()
    }

    /// An install on the first sync key while the account's list shows a removed device: the key was replaced when that
    /// device was removed (it is replaced only then), and the removed device still has the first one. Nothing is posted
    /// until the current key arrives.
    fn waiting_for_key(&self) -> bool {
        let rotated = self
            .device_list
            .as_ref()
            .is_some_and(|l| l.devices.iter().any(|d| !d.active()));
        rotated
            && self
                .sync
                .as_ref()
                .is_none_or(|s| s.key.as_ref().is_none_or(|k| k.version == 0))
    }

    /// Posts entries in batches that fit a relay event.
    fn post_entries(&mut self, entries: Vec<Entry>) -> bool {
        let Some(list) = self.device_list.as_ref().map(DeviceList::to_json) else {
            return false;
        };
        if self.waiting_for_key() {
            return false;
        }
        let mut batch: Vec<Value> = Vec::new();
        let mut size = 0;
        let mut ok = true;
        for e in entries {
            let v = serde_json::to_value(&e).unwrap_or(Value::Null);
            let n = v.to_string().len();
            if n > BATCH_BYTES {
                // Too big for one relay event (a very long message with a large card): it waits for a direct sync
                // between the devices instead of holding up everything after it.
                continue;
            }
            if !batch.is_empty() && size + n > BATCH_BYTES {
                ok &= self.post_self(
                    &json!({ "t": "entries", "list": list, "entries": std::mem::take(&mut batch) }),
                );
                size = 0;
            }
            size += n;
            batch.push(v);
        }
        if !batch.is_empty() {
            ok &= self.post_self(&json!({ "t": "entries", "list": list, "entries": batch }));
        }
        ok
    }

    fn post_hello(&mut self) {
        let (Some(sync), Some(list)) = (self.sync.as_ref(), self.device_list.as_ref()) else {
            return;
        };
        let payload =
            json!({ "t": "hello", "list": list.to_json(), "summary": sync.replica.summary() });
        if self.post_self(&payload) {
            if let Some(s) = self.sync.as_mut() {
                s.last_hello = self.ports.clock.now_ms();
            }
        }
    }

    /// Runs every engine tick, before the store-and-forward pass.
    pub(super) fn tick_sync(&mut self) {
        self.record();
        if self.sync.is_none() {
            return;
        }
        self.tick_link_sync();
        self.tick_snapshots();
        if !self.ports.transport.signal_available() {
            if let Some(s) = self.sync.as_mut() {
                s.subscribed.clear();
                s.inbox = None;
            }
            return;
        }
        let now = self.now();
        self.tick_inbox(now);
        let mut labels: Vec<String> = match self
            .sync
            .as_ref()
            .and_then(|s| s.key.as_ref())
            .map(|k| SelfBox::current(k, now))
        {
            Some(Ok(boxes)) => boxes.iter().map(|b| b.label.clone()).collect(),
            _ => return,
        };
        labels.extend(self.rendezvous_labels(now));
        let mut fresh = false;
        for label in &labels {
            let known = self
                .sync
                .as_ref()
                .is_some_and(|s| s.subscribed.contains(label));
            if !known && self.ports.transport.signal_subscribe(label).is_ok() {
                if let Some(s) = self.sync.as_mut() {
                    s.subscribed.push(label.clone());
                    fresh = true;
                }
            }
        }
        if let Some(s) = self.sync.as_mut() {
            s.subscribed.retain(|l| labels.contains(l));
        }
        // A lone install posts nothing at all (risk 25): it only listens. It says hello once it knows another device of
        // the account, or when it was itself set up from the recovery phrase (it is joining an account that already
        // has a device).
        let joined = matches!(self.ports.storage.kv(KV_JOINED), Ok(Some(_)));
        let talk = joined || self.sync.as_ref().is_some_and(|s| !s.others.is_empty());
        let due = self
            .sync
            .as_ref()
            .is_some_and(|s| now - s.last_hello >= HELLO_EVERY_MS);
        if talk && (fresh || due) {
            self.post_hello();
        }
        // Our own new entries, once there is another device to read them.
        let Some(s) = self.sync.as_ref() else { return };
        if s.others.is_empty() {
            return;
        }
        let (device, series, pushed) = (s.writer.device.clone(), s.writer.series, s.pushed);
        let mut summary = Summary::new();
        summary.insert(format!("{device}/{series}"), pushed);
        let mine: Vec<Entry> = s
            .replica
            .missing_for(&summary, ANSWER_MAX)
            .into_iter()
            .filter(|e| e.device == device && e.series == series)
            .collect();
        let Some(top) = mine.last().map(|e| e.seq) else {
            return;
        };
        if self.post_entries(mine) {
            if let Some(s) = self.sync.as_mut() {
                s.pushed = top;
            }
            let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                key: KV_PUSHED.into(),
                value: serde_json::to_vec(&top).unwrap_or_default(),
            }]);
        }
    }

    /// A direct link to another device of ours is up (step 7): say where our log stands; it answers with what we lack,
    /// and with its own summary if we hold what it lacks. No relay, no relay size limits.
    pub(super) fn link_sync_hello(&mut self, peer: &UserId) {
        let Some(s) = self.sync.as_mut() else { return };
        let summary = s.replica.summary();
        let text = serde_json::to_string(&summary).unwrap_or_default();
        s.link_told.insert(peer.to_string(), text);
        let frame = json!({ "v": privcord_domain::limits::PROTOCOL_VERSION, "kind": "sync", "t": "hello", "summary": summary });
        self.send_secure(peer, &frame);
    }

    /// Our log changed: tell the devices we are linked to directly.
    fn tick_link_sync(&mut self) {
        let Some(s) = self.sync.as_ref() else { return };
        let now_text = serde_json::to_string(&s.replica.summary()).unwrap_or_default();
        let due: Vec<UserId> = self
            .own_other_devices()
            .into_iter()
            .filter(|a| self.peers.get(a).is_some_and(super::Peer::ready))
            .filter(|a| s.link_told.get(a.as_str()) != Some(&now_text))
            .collect();
        for peer in due {
            self.link_sync_hello(&peer);
        }
    }

    /// Sync over a direct link with another device of ours. The link is authenticated with that device's own keys from
    /// the account's list, so what it sends is taken as a post of it would be.
    pub(super) fn on_sync_frame(&mut self, peer: &UserId, frame: &Value) {
        let now = self.now();
        if let Some((_, device)) = peer.as_str().split_once('.') {
            self.note_device_seen(device);
        }
        match frame.get("t").and_then(Value::as_str) {
            Some("hello") => {
                let theirs: Summary = frame
                    .get("summary")
                    .and_then(|s| serde_json::from_value(s.clone()).ok())
                    .unwrap_or_default();
                let Some(s) = self.sync.as_ref() else { return };
                let missing = s.replica.missing_for(&theirs, usize::MAX);
                let ours = s.replica.summary();
                let behind = theirs
                    .iter()
                    .any(|(k, v)| ours.get(k).copied().unwrap_or(0) < *v);
                let mut batch: Vec<Value> = Vec::new();
                let mut size = 0;
                for e in missing {
                    let v = serde_json::to_value(&e).unwrap_or(Value::Null);
                    let n = v.to_string().len();
                    if !batch.is_empty() && size + n > LINK_BATCH_BYTES {
                        let entries = std::mem::take(&mut batch);
                        self.send_secure(
                            peer,
                            &json!({ "v": 1, "kind": "sync", "t": "entries", "entries": entries }),
                        );
                        size = 0;
                    }
                    size += n;
                    batch.push(v);
                }
                if !batch.is_empty() {
                    self.send_secure(
                        peer,
                        &json!({ "v": 1, "kind": "sync", "t": "entries", "entries": batch }),
                    );
                }
                if behind {
                    self.link_sync_hello(peer);
                }
            }
            Some("entries") => {
                let entries: Vec<Entry> = frame
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| serde_json::from_value(v.clone()).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                self.take_entries(entries, now);
            }
            Some("inbox") => {
                let sender = Self::device_of(peer).and_then(|d| {
                    self.device_list
                        .as_ref()
                        .and_then(|l| l.get(d))
                        .filter(|e| e.active())
                        .cloned()
                });
                if let (Some(sender), Some(payload)) = (sender, frame.get("payload")) {
                    self.device_message(&sender, &payload.clone());
                }
            }
            _ => {}
        }
    }

    /// Is this mailbox one of our self-mailboxes?
    pub(super) fn is_self_mailbox(&self, mailbox: &str) -> bool {
        self.sync.as_ref().is_some_and(|s| {
            s.subscribed.iter().any(|l| l == mailbox) || s.inbox.as_deref() == Some(mailbox)
        })
    }

    /// The first key's mailboxes, once the key was replaced (see `SyncState::first_key`).
    fn rendezvous_labels(&self, now: i64) -> Vec<String> {
        self.sync
            .as_ref()
            .filter(|s| s.key.as_ref().is_some_and(|k| k.version > 0))
            .and_then(|s| s.first_key.as_ref())
            .and_then(|k| SelfBox::current(k, now).ok())
            .map(|b| b.iter().map(|b| b.label.clone()).collect())
            .unwrap_or_default()
    }

    /// A post on the first key's mailbox after the key was replaced: only the device list it carries counts (that key
    /// is known to removed devices). A device we did not know (an install set up from the phrase) is given the key.
    fn rendezvous_blob(&mut self, mailbox: &str, blob: &SealedBlob, now: i64) {
        let Some(k0) = self.sync.as_ref().and_then(|s| s.first_key.clone()) else {
            return;
        };
        let Some(payload) = SelfBox::current(&k0, now)
            .ok()
            .and_then(|boxes| boxes.into_iter().find(|b| b.label == mailbox))
            .and_then(|b| b.open(blob))
        else {
            return;
        };
        let Some((sign, dh)) = self.account_public() else {
            return;
        };
        let Some(text) = payload.get("list").and_then(Value::as_str) else {
            return;
        };
        let Ok(list) = DeviceList::from_json(text) else {
            return;
        };
        if list.verify(&sign, &dh).is_err() {
            return;
        }
        let author = payload
            .get("device")
            .and_then(Value::as_str)
            .and_then(|d| list.get(d));
        if !author.is_some_and(|d| d.active() && signed_by(blob, d)) {
            return;
        }
        if let Some(device) = payload.get("device").and_then(Value::as_str) {
            self.note_device_seen(device);
        }
        if self.learn_list(&list, text) {
            let Some(ours) = self.device_list.clone() else {
                return;
            };
            let message = self.sync_key_message(&ours);
            let own = self.device_id.clone().unwrap_or_default();
            let others: Vec<_> = ours.active().filter(|d| d.id != own).cloned().collect();
            for d in others {
                self.send_to_device(&d, &message);
            }
        }
    }

    /// A blob arrived on our self-mailbox.
    pub(super) fn sync_blob(&mut self, mailbox: &str, blob: &SealedBlob) {
        let now = self.now();
        if self.sync.as_ref().and_then(|s| s.inbox.as_deref()) == Some(mailbox) {
            self.inbox_blob(blob);
            return;
        }
        if self.rendezvous_labels(now).iter().any(|l| l == mailbox) {
            self.rendezvous_blob(mailbox, blob, now);
            return;
        }
        let Some(key) = self.sync.as_ref().and_then(|s| s.key.clone()) else {
            return;
        };
        let Ok(boxes) = SelfBox::current(&key, now) else {
            return;
        };
        let Some(payload) = boxes
            .iter()
            .find(|b| b.label == mailbox)
            .and_then(|b| b.open(blob))
        else {
            return;
        };
        // The author's device list, checked against the account's keys, and the post signed by that device in it.
        let Some((sign, dh)) = self.account_public() else {
            return;
        };
        let Some(text) = payload.get("list").and_then(Value::as_str) else {
            return;
        };
        let Ok(list) = DeviceList::from_json(text) else {
            return;
        };
        if list.verify(&sign, &dh).is_err() {
            return;
        }
        let author = payload
            .get("device")
            .and_then(Value::as_str)
            .and_then(|d| list.get(d));
        if !author.is_some_and(|d| d.active() && signed_by(blob, d)) {
            return;
        }
        if let Some(device) = payload.get("device").and_then(Value::as_str) {
            self.note_device_seen(device);
        }
        if self.learn_list(&list, text) {
            // A device we did not know: tell it about us, so it starts posting what it writes, and give it our history.
            self.backfill();
            self.record();
            self.post_hello();
        }
        match payload.get("t").and_then(Value::as_str) {
            Some("hello") => {
                let theirs: Summary = payload
                    .get("summary")
                    .and_then(|s| serde_json::from_value(s.clone()).ok())
                    .unwrap_or_default();
                self.answer_hello(&theirs, now);
            }
            Some("entries") => {
                let entries: Vec<Entry> = payload
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| serde_json::from_value(v.clone()).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                self.take_entries(entries, now);
            }
            _ => {}
        }
    }

    /// Puts what this install held before sync into its log, once: contacts, groups, messages (newest first), their
    /// reactions, deletions and read marks. Runs when another device of the account first shows up, so that device
    /// gets the whole history, not only what changes from now on.
    fn backfill(&mut self) {
        if matches!(self.ports.storage.kv(KV_BACKFILLED), Ok(Some(_))) {
            return;
        }
        let mut ops: Vec<StorageOp> = self
            .contacts
            .values()
            .map(|c| StorageOp::PutContact(c.stored.clone()))
            .collect();
        if let Ok(Some(value)) = self.ports.storage.kv(super::KV_GROUPS) {
            if let Some(s) = self.sync.as_mut() {
                s.group_counts.clear();
            }
            ops.push(StorageOp::SetKv {
                key: super::KV_GROUPS.into(),
                value,
            });
        }
        let mut rows: Vec<StoredMessage> = self
            .all_rows()
            .into_iter()
            .filter(|m| !matches!(m.conversation, ConversationId::Channel(_)))
            .collect();
        rows.sort_by_key(|m| std::cmp::Reverse(m.message.client_created_at));
        for m in rows {
            let id = m.message.message_id.clone();
            ops.push(StorageOp::InsertMessage(m.clone()));
            if self.deleted.contains(&id) {
                ops.push(StorageOp::Tombstone(id.clone()));
            }
            if m.message.read_at.is_some() {
                ops.push(StorageOp::SetDelivery {
                    id: id.clone(),
                    state: m.message.delivery_state,
                    read_at: m.message.read_at,
                });
            }
            for (uid, emoji) in self.reactions.get(&id).into_iter().flatten() {
                ops.push(StorageOp::SetReaction {
                    message_id: id.clone(),
                    uid: uid.clone(),
                    emoji: Some(emoji.clone()),
                });
            }
        }
        if let Ok(list) =
            serde_json::to_vec(&self.muted.iter().map(UserId::as_str).collect::<Vec<_>>())
        {
            ops.push(StorageOp::SetKv {
                key: super::social::KV_MUTED.into(),
                value: list,
            });
        }
        let p = &self.prefs;
        for (key, value) in [
            (super::prefs::KV_PINS, serde_json::json!(p.pins)),
            (super::prefs::KV_ARCHIVED, serde_json::json!(p.archived)),
            (super::prefs::KV_MUTE_UNTIL, serde_json::json!(p.mute_until)),
            (super::prefs::KV_QUIET, serde_json::json!(p.quiet)),
            (super::prefs::KV_SECURE, serde_json::json!(p.secure_screen)),
            (super::prefs::KV_SILENT, serde_json::json!(p.silent_chats)),
        ] {
            ops.push(StorageOp::SetKv {
                key: format!("{key}.v1"),
                value: serde_json::to_vec(&value).unwrap_or_default(),
            });
        }
        self.recorder.push(ops);
        let _ = self.ports.storage.apply(&[StorageOp::SetKv {
            key: KV_BACKFILLED.into(),
            value: b"1".to_vec(),
        }]);
    }

    /// Marks this install as joining an account that already exists (restored from the recovery phrase).
    pub(super) fn mark_joined(&mut self) {
        let _ = self.ports.storage.apply(&[StorageOp::SetKv {
            key: KV_JOINED.into(),
            value: b"1".to_vec(),
        }]);
    }

    /// Another device's copy of the account's device list (already checked against the account's keys). A newer
    /// list that extends ours replaces it; two lists that fork (an install restored from the recovery phrase started
    /// its own) are merged and signed by a device holding the account key. Returns whether a device we did not know
    /// appeared.
    fn learn_list(&mut self, list: &DeviceList, text: &str) -> bool {
        let Some((sign, dh)) = self.account_public() else {
            return false;
        };
        let Some(current) = self.device_list.clone() else {
            return false;
        };
        // The same devices, signed separately (two holders of the account key merged at once): both sides keep the same
        // one, chosen without signing anything new, so merges never go back and forth.
        let same_devices = |a: &DeviceList, b: &DeviceList| {
            let key = |l: &DeviceList| {
                let mut v: Vec<(String, Option<i64>)> = l
                    .devices
                    .iter()
                    .map(|d| (d.id.clone(), d.removed_at))
                    .collect();
                v.sort();
                v
            };
            key(a) == key(b)
        };
        let next = if current.accepts(list, &sign, &dh).is_ok() {
            Some(list.clone())
        } else if list.accepts(&current, &sign, &dh).is_ok() || list == &current {
            None
        } else if same_devices(list, &current) {
            (list.version > current.version
                || (list.version == current.version && list.sig < current.sig))
                .then(|| list.clone())
        } else if let Some(merged) = self
            .identity
            .as_ref()
            .and_then(|a| current.merged(list, a).ok())
        {
            Some(merged)
        } else {
            // A fork this device cannot sign a merge for: trust its devices until a holder of the account key does.
            if let Some(s) = self.sync.as_mut() {
                s.replica.vouch(list);
                s.lists.push(text.to_owned());
                let value = serde_json::to_vec(&s.lists).unwrap_or_default();
                let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                    key: KV_LISTS.into(),
                    value,
                }]);
            }
            None
        };
        let own = self.device_id.clone().unwrap_or_default();
        let before: BTreeSet<String> = self
            .sync
            .as_ref()
            .map(|s| s.others.clone())
            .unwrap_or_default();
        if let Some(next) = next {
            let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                key: super::devices::KV_DEVICE_LIST.into(),
                value: next.to_json().into_bytes(),
            }]);
            if let Some(s) = self.sync.as_mut() {
                s.devices_changed(&next);
            }
            self.device_list = Some(next);
        }
        if let Some(s) = self.sync.as_mut() {
            s.others
                .extend(list.active().map(|d| d.id.clone()).filter(|d| d != &own));
            return s.others.len() > before.len();
        }
        false
    }

    /* ---------------------------------------------------------------- the device inbox (handing over the sync key) */

    /// Listens on this device's inbox, and (a device without the sync key) asks the other devices for it.
    fn tick_inbox(&mut self, now: i64) {
        let Some(own) = self.device_id.clone() else {
            return;
        };
        let label = device_inbox(&own);
        if self
            .sync
            .as_ref()
            .is_some_and(|s| s.inbox.as_deref() != Some(label.as_str()))
            && self.ports.transport.signal_subscribe(&label).is_ok()
        {
            if let Some(s) = self.sync.as_mut() {
                s.inbox = Some(label);
            }
        }
        let waiting = self.waiting_for_key();
        let asking = self
            .sync
            .as_ref()
            .is_some_and(|s| (s.key.is_none() || waiting) && now - s.last_ask >= ASK_KEY_EVERY_MS);
        if !asking {
            return;
        }
        let (Some(list), Some(s)) = (self.device_list.clone(), self.sync.as_mut()) else {
            return;
        };
        s.last_ask = now;
        let payload = json!({ "t": "need-key", "list": list.to_json() });
        for d in list.active().filter(|d| d.id != own) {
            self.put_to_device(d, &payload);
        }
    }

    /// Something for one other device of ours: over a direct link when there is one, and sealed to its inbox.
    pub(super) fn send_to_device(
        &mut self,
        to: &privcord_crypto::devices::DeviceEntry,
        payload: &Value,
    ) {
        if let Some(addr) = self
            .uid
            .as_ref()
            .and_then(|u| UserId::new(format!("{u}.{}", to.id)).ok())
        {
            if self.peers.get(&addr).is_some_and(super::Peer::ready) {
                self.send_secure(
                    &addr,
                    &json!({ "v": 1, "kind": "sync", "t": "inbox", "payload": payload }),
                );
            }
        }
        if self.ports.transport.signal_available() {
            self.put_to_device(to, payload);
        }
    }

    fn put_to_device(&mut self, to: &privcord_crypto::devices::DeviceEntry, payload: &Value) {
        let Some(s) = self.sync.as_ref() else { return };
        let Ok(blob) = seal_to_device(&s.device, to, payload, &mut *self.ports.entropy) else {
            return;
        };
        let _ = self.ports.transport.signal_put(
            &device_inbox(&to.id),
            &blob.encode(),
            PutKind::Msg,
            RELAY_TTL_SEC,
        );
    }

    /// Something another device of this account sealed to this device.
    fn inbox_blob(&mut self, blob: &SealedBlob) {
        let (Some(list), Some(own)) = (self.device_list.clone(), self.device_id.clone()) else {
            return;
        };
        let Some(own_entry) = list.get(&own).cloned() else {
            return;
        };
        // The sender is a device of the account's list (by the id its keys give it).
        let sender = list.active().find(|d| {
            privcord_crypto::identity::uid_from_keys(&d.sign, &d.dh)
                .is_ok_and(|u| u.as_str() == blob.from)
        });
        let Some(sender) = sender.cloned() else {
            return;
        };
        let Some(payload) = self
            .sync
            .as_ref()
            .and_then(|s| open_from_device(blob, &s.device, &own_entry, &sender))
        else {
            return;
        };
        self.device_message(&sender, &payload);
    }

    /// Something another device of ours sent this one (sealed to its inbox, or over a direct link with it).
    fn device_message(&mut self, sender: &privcord_crypto::devices::DeviceEntry, payload: &Value) {
        // A list it carries (signed by the account) is learned first: a removal, or a device added since.
        let carried = payload
            .get("list")
            .and_then(Value::as_str)
            .and_then(|t| DeviceList::from_json(t).ok().map(|l| (l, t.to_owned())));
        if let (Some((list, text)), Some((sign, dh))) = (&carried, self.account_public()) {
            if list.verify(&sign, &dh).is_ok() {
                self.learn_list(list, text);
            }
        }
        // Still on the list, by what we know now.
        if !self
            .device_list
            .as_ref()
            .and_then(|l| l.get(&sender.id))
            .is_some_and(privcord_crypto::devices::DeviceEntry::active)
        {
            return;
        }
        match payload.get("t").and_then(Value::as_str) {
            Some("need-key") => {
                let Some(list) = self.device_list.clone() else {
                    return;
                };
                let reply = self.sync_key_message(&list);
                if !reply.is_null() {
                    self.send_to_device(sender, &reply);
                }
            }
            Some("sync-key") => self.offered_key(payload),
            // The account removed this device: everything on it is erased (the tick does it once the list says so; the
            // list came with this message and was learned above).
            Some("wipe") => {}
            _ => {}
        }
    }

    /// A sync key from another device of ours. The first key (version 0) is the one derived from the account key and
    /// is taken while this device has none; every later key must carry the account's signature, so a device cannot
    /// hand out a key of its own. A newer version replaces ours; two different keys of the same version (two installs
    /// holding the account key replaced it at once) settle on the one with the lower hash.
    fn offered_key(&mut self, payload: &Value) {
        let version = payload
            .get("v")
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok());
        let bytes = payload
            .get("key")
            .and_then(Value::as_str)
            .and_then(privcord_domain::codec::decode);
        let Some(key) = version
            .zip(bytes)
            .and_then(|(v, b)| SyncKey::from_bytes(v, &b))
        else {
            return;
        };
        let k0 = payload
            .get("k0")
            .and_then(Value::as_str)
            .and_then(privcord_domain::codec::decode)
            .and_then(|b| SyncKey::from_bytes(0, &b));
        if let Some(k0) = k0 {
            if self.sync.as_ref().is_some_and(|s| s.first_key.is_none()) {
                let value =
                    json!({ "v": 0, "key": privcord_domain::codec::encode(&k0.to_bytes()) });
                let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                    key: KV_FIRST_KEY.into(),
                    value: value.to_string().into_bytes(),
                }]);
                if let Some(s) = self.sync.as_mut() {
                    s.first_key = Some(k0);
                }
            }
        }
        let sig = payload
            .get("sig")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if key.version > 0 {
            let Some((account, _)) = self.account_public() else {
                return;
            };
            let mut body = key_body(&key);
            if let (Some(s), Some(o)) = (&sig, body.as_object_mut()) {
                o.insert("accountSig".into(), json!(s));
            }
            if !privcord_crypto::synclog::ruling_valid(&body, &account) {
                return;
            }
        }
        let hash = |k: &SyncKey| privcord_crypto::kdf::sha256_hex(&k.to_bytes());
        let take = self.sync.as_ref().is_some_and(|s| match &s.key {
            None => true,
            Some(cur) => {
                cur.version < key.version || (cur.version == key.version && hash(&key) < hash(cur))
            }
        });
        if take {
            self.take_sync_key(key, sig);
        }
    }

    /// Another device said how far it has each log: send what it lacks, and ask for what we lack.
    fn answer_hello(&mut self, theirs: &Summary, now: i64) {
        let Some(s) = self.sync.as_ref() else { return };
        if now - s.last_answer < ANSWER_GAP_MS {
            return;
        }
        let needs = s.replica.needs_base(theirs).map(Snapshot::id);
        let missing = s.replica.missing_for(theirs, ANSWER_MAX);
        let ours = s.replica.summary();
        let behind = theirs
            .iter()
            .any(|(k, v)| ours.get(k).copied().unwrap_or(0) < *v);
        if !missing.is_empty() && self.post_entries(missing) {
            if let Some(s) = self.sync.as_mut() {
                s.last_answer = now;
            }
        }
        if behind {
            self.post_hello();
        }
        if let Some(id) = needs {
            self.offer_base(&id);
        }
    }

    /// Entries from another device (or echoes of ours): through the merge rules, then into storage.
    pub(super) fn take_entries(&mut self, entries: Vec<Entry>, now: i64) {
        let own = self.device_id.clone().unwrap_or_default();
        if let Some(s) = self.sync.as_mut() {
            for e in entries {
                // Held-back entries are applied when their gap fills; a refused one is simply dropped.
                let _ = s.replica.receive(e, now);
            }
        }
        // Everything the log now holds beyond what storage holds: keep it, and apply what other devices wrote.
        let stored: Summary = self.kv_json(KV_INDEX).unwrap_or_default();
        let Some(s) = self.sync.as_ref() else { return };
        let new_entries = s.replica.missing_for(&stored, usize::MAX);
        if new_entries.is_empty() {
            return;
        }
        let mut keep: Vec<StorageOp> = new_entries
            .iter()
            .map(|e| StorageOp::SetKv {
                key: entry_key(&e.device, e.series, e.seq),
                value: serde_json::to_vec(e).unwrap_or_default(),
            })
            .collect();
        keep.push(StorageOp::SetKv {
            key: KV_INDEX.into(),
            value: Self::index_value(&s.replica),
        });
        let _ = self.ports.storage.apply(&keep);
        let applied: Vec<Entry> = new_entries
            .into_iter()
            .filter(|e| e.device != own)
            .collect();
        self.apply_entries(&applied);
    }

    /// Writes other devices' entries into storage, without recording them again.
    pub(super) fn apply_entries(&mut self, entries: &[Entry]) {
        if entries.is_empty() {
            return;
        }
        let Some(own) = self.uid.clone() else { return };
        let mut messages = Vec::new();
        let mut contacts = Vec::new();
        let mut touched: BTreeSet<String> = BTreeSet::new();
        // Messages written by a device without the account key.
        let mut by_device: BTreeSet<String> = BTreeSet::new();
        // File messages whose bytes come separately.
        let mut with_media: Vec<Value> = Vec::new();
        let mut muted: Option<Value> = None;
        let mut pref_settings: BTreeSet<String> = BTreeSet::new();
        let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
        for e in entries {
            match e.kind.as_str() {
                kind::MESSAGE => {
                    if e.body.get("media").is_some() {
                        with_media.push(e.body.clone());
                    }
                    if let Some(id) = e.body.get("id").and_then(Value::as_str) {
                        touched.insert(id.to_owned());
                        if e.body.get(FIELD_HELP).and_then(Value::as_bool) == Some(true) {
                            by_device.insert(id.to_owned());
                        }
                    }
                    messages.push(e.body.clone());
                }
                kind::DELETE | kind::HIDE | kind::REACT => {
                    if let Some(id) = e.body.get("target").and_then(Value::as_str) {
                        touched.insert(id.to_owned());
                    }
                }
                KIND_CONTACT => contacts.push(e.body.clone()),
                KIND_GROUP => {
                    if let (Some(id), Some(events)) = (
                        e.body.get("id").and_then(Value::as_str),
                        e.body.get("events").and_then(Value::as_array),
                    ) {
                        groups.push((id.to_owned(), events.clone()));
                    }
                }
                kind::SEEN | kind::TICK => {
                    if let Some(id) = e.body.get("target").and_then(Value::as_str) {
                        touched.insert(id.to_owned());
                    }
                }
                kind::SETTING
                    if e.body
                        .get("key")
                        .and_then(Value::as_str)
                        .is_some_and(|k| k.starts_with(super::prefs::PREFIX)) =>
                {
                    if let Some(k) = e.body.get("key").and_then(Value::as_str) {
                        pref_settings.insert(k.to_owned());
                    }
                }
                kind::SETTING
                    if e.body.get("key").and_then(Value::as_str) == Some(SETTING_MUTED) =>
                {
                    muted = self
                        .sync
                        .as_ref()
                        .and_then(|s| s.replica.state().setting(SETTING_MUTED));
                }
                _ => {}
            }
        }
        self.recorder.quiet(true);
        let now = self.now();
        if !messages.is_empty() || !contacts.is_empty() {
            let data = json!({ "v": 1, "uid": own, "contacts": contacts, "messages": messages });
            if let Some((_, bundle)) = read_bundle(&data) {
                let _ = import(
                    &bundle,
                    Source::Backup,
                    &NoFiles,
                    &own,
                    &*self.ports.storage,
                    now,
                );
            }
        }
        for body in &with_media {
            if let Some(id) = body
                .get("id")
                .and_then(Value::as_str)
                .and_then(|i| MessageId::new(i).ok())
            {
                self.want_media(&id, body);
            }
        }
        // Groups: each event is checked again and added to what this install has (never replacing it).
        if !groups.is_empty() {
            let mut saved: BTreeMap<String, Vec<Value>> =
                self.kv_json(super::KV_GROUPS).unwrap_or_default();
            for (id, events) in groups {
                let mut log = privcord_crypto::group::GroupLog::new(&id);
                for e in saved.get(&id).into_iter().flatten().chain(events.iter()) {
                    let _ = log.add(e);
                }
                let all: Vec<Value> = log
                    .all()
                    .iter()
                    .map(privcord_crypto::group::GroupEvent::to_value)
                    .collect();
                if !all.is_empty() {
                    if let Some(s) = self.sync.as_mut() {
                        s.group_counts.insert(id.clone(), all.len());
                    }
                    saved.insert(id, all);
                }
            }
            if let Ok(value) = serde_json::to_vec(&saved) {
                let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                    key: super::KV_GROUPS.into(),
                    value,
                }]);
            }
        }
        // Deletes and reactions, as the merged state has them, for every message these entries touched (in any
        // order: a reaction that came before its message is applied now that the message is here).
        let mut ops: Vec<StorageOp> = Vec::new();
        // Our own messages as storage has them now: their delivery (for the ticks other devices saw) and who they
        // were for.
        let own_rows: Vec<privcord_ports::StoredMessage> = self
            .ports
            .storage
            .messages()
            .unwrap_or_default()
            .into_iter()
            .filter(|m| Some(&m.message.sender_id) == self.uid.as_ref())
            .collect();
        let own_state: std::collections::HashMap<MessageId, DeliveryState> = own_rows
            .iter()
            .map(|m| (m.message.message_id.clone(), m.message.delivery_state))
            .collect();
        // Unread messages as storage has them now (just imported ones included), read once.
        let unread: std::collections::HashMap<MessageId, privcord_domain::message::DeliveryState> =
            self.ports
                .storage
                .messages()
                .unwrap_or_default()
                .into_iter()
                .filter(|m| {
                    m.message.read_at.is_none() && touched.contains(m.message.message_id.as_str())
                })
                .map(|m| (m.message.message_id, m.message.delivery_state))
                .collect();
        if let Some(s) = self.sync.as_ref() {
            let state = s.replica.state();
            for id in &touched {
                let Ok(mid) = MessageId::new(id.as_str()) else {
                    continue;
                };
                if state.is_deleted(id) {
                    ops.push(StorageOp::Tombstone(mid));
                    continue;
                }
                if state.is_hidden(id) {
                    ops.push(StorageOp::DeleteMessage(mid));
                    continue;
                }
                if let (Some(level), Some(cur)) = (state.tick(id), own_state.get(&mid)) {
                    let seen = if level >= 2 {
                        DeliveryState::Read
                    } else {
                        DeliveryState::Delivered
                    };
                    if DeliveryState::highest(*cur, seen) != *cur {
                        ops.push(StorageOp::SetDelivery {
                            id: mid.clone(),
                            state: seen,
                            read_at: (seen == DeliveryState::Read).then_some(now),
                        });
                    }
                }
                if state.is_seen(id) {
                    if let Some(delivery) = unread.get(&mid) {
                        ops.push(StorageOp::SetDelivery {
                            id: mid.clone(),
                            state: *delivery,
                            read_at: Some(now),
                        });
                    }
                }
                for (by, emoji) in state.reactions_raw(id) {
                    if let Ok(uid) = UserId::new(by.as_str()) {
                        ops.push(StorageOp::SetReaction {
                            message_id: mid.clone(),
                            uid,
                            emoji,
                        });
                    }
                }
            }
        }
        ops.extend(self.deliver_for_devices(&own_rows, &by_device));
        if let Some(list) = muted {
            ops.push(StorageOp::SetKv {
                key: super::social::KV_MUTED.into(),
                value: serde_json::to_vec(&list).unwrap_or_default(),
            });
        }
        // Preferences: the merged value of each setting that arrived (last writer wins).
        if let Some(s) = self.sync.as_ref() {
            for key in &pref_settings {
                if let Some(v) = s.replica.state().setting(key) {
                    ops.push(StorageOp::SetKv {
                        key: format!("{key}.v1"),
                        value: serde_json::to_vec(&v).unwrap_or_default(),
                    });
                }
            }
        }
        // One at a time: a reaction to a message this device never got must not stop the rest.
        for op in ops {
            let _ = self.ports.storage.apply(std::slice::from_ref(&op));
        }
        self.recorder.quiet(false);
        if self.reload_from_storage().is_ok() {
            self.hub.publish(EngineEvent::Restored);
        }
    }
}
