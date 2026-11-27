//! Snapshots and pruning (docs/design/syncengine.md §7, step 8).
//!
//! The log grows forever unless older entries go. One device (the first on the account's list) writes a snapshot of
//! the merged state now and then; it is far larger than a relay event, so it travels like a file (`core/media.rs`),
//! announced by a `snapshot` entry that carries its id (a hash of all of it) and its file reference. Every device that
//! holds it and every entry up to its cut says so (`have`). Once every active device has, each prunes what the snapshot
//! covers, from memory and from storage, and keeps the snapshot as where its log starts. A device that joins later
//! starts from the snapshot, then takes the entries after it.

use super::{sync::entry_key, Core, StorageOp};
use privcord_crypto::{
    media::{file_key, seal_media_with, MediaRef},
    synclog::{kind, Entry, Snapshot},
};
use serde_json::{json, Value};

/// Where the log starts once older entries were pruned.
const KV_BASE: &str = "sync.base";
/// Entries held in the log before the next snapshot is made.
const SNAPSHOT_EVERY: usize = 2_000;
/// A deleted message's text stays in the log until a snapshot lets it go, so a deletion asks for one: at most this often.
const DELETION_SNAPSHOT_MS: i64 = 6 * 3_600_000;
/// Gaps entries wait on, and since when: `{ "device/series/from-to": first seen }`.
const KV_GAPS: &str = "sync.gaps";
/// A gap still open after this long is given up (a relay keeps a post for 7 days; after that it cannot come).
const LOST_AFTER_MS: i64 = 8 * 24 * 3600 * 1000;
/// What a snapshot is called as a piece of media (`media.out`, `media.in`): a short form of its id.
pub(super) const SNAP_PREFIX: &str = "snap:";

fn short(id: &str) -> String {
    id.chars().take(32).collect()
}

fn snap_key(short: &str) -> String {
    format!("sync.snap.{short}")
}

fn ref_key(short: &str) -> String {
    format!("sync.snapref.{short}")
}

impl Core {
    pub(super) fn stored_base(&self) -> Option<Snapshot> {
        let b = self.ports.storage.kv(KV_BASE).ok()??;
        Snapshot::from_json(std::str::from_utf8(&b).ok()?)
    }

    /// The bytes of a snapshot this install holds, and the key its pieces are cut with (the one its announcement names).
    pub(super) fn snapshot_piece_source(&self, short: &str) -> Option<(Vec<u8>, [u8; 32])> {
        let bytes = self.ports.storage.kv(&snap_key(short)).ok()??;
        let r: MediaRef = self.kv_json(&ref_key(short))?;
        let key = <[u8; 32]>::try_from(privcord_domain::codec::decode(&r.key)?.as_slice()).ok()?;
        Some((bytes, key))
    }

    /// Runs every sync tick: fetch announced snapshots, confirm the newest, prune once everyone has, make the next.
    pub(super) fn tick_snapshots(&mut self) {
        let Some(me) = self.device_id.clone() else {
            return;
        };
        let Some(s) = self.sync.as_ref() else { return };
        let replica = s.replica();
        // Announced snapshots this install does not hold yet are fetched like files.
        let notes = replica.snapshot_notes();
        for note in notes {
            let (Some(id), Some(r)) = (
                note.get("id").and_then(Value::as_str),
                note.get("ref")
                    .and_then(|r| serde_json::from_value::<MediaRef>(r.clone()).ok()),
            ) else {
                continue;
            };
            let sh = short(id);
            if self
                .ports
                .storage
                .kv(&snap_key(&sh))
                .ok()
                .flatten()
                .is_some()
            {
                continue;
            }
            if self.kv_json::<MediaRef>(&ref_key(&sh)).is_none() {
                if let Ok(v) = serde_json::to_vec(&r) {
                    let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                        key: ref_key(&sh),
                        value: v,
                    }]);
                }
            }
            self.want_snapshot(&sh, r);
        }
        let Some(s) = self.sync.as_ref() else { return };
        let replica = s.replica();
        let latest = replica.state().latest_snapshot().map(|(id, _)| id.clone());
        if let Some(id) = &latest {
            let held = replica.held(id).cloned();
            let mine = replica.state().holders(id).contains(&me);
            if let Some(snap) = held {
                if replica.covers(&snap) && !mine {
                    self.append_items(vec![(kind::HAVE, json!({ "snapshot": id }))]);
                }
            }
            let base_id = self
                .sync
                .as_ref()
                .and_then(|s| s.replica().base().map(Snapshot::id));
            if base_id.as_deref() != Some(id.as_str()) {
                self.prune_to(id, base_id);
            }
        }
        // The first device on the list makes the next one, when the log has grown and the last one has been taken up.
        let Some(s) = self.sync.as_ref() else { return };
        let replica = s.replica();
        let maker = self
            .device_list
            .as_ref()
            .and_then(|l| l.active().map(|d| d.id.clone()).min());
        let settled = latest.is_none() || replica.base().map(Snapshot::id) == latest;
        let now = self.now();
        let deletion_due = replica.has_deletions()
            && now - s.last_snapshot >= DELETION_SNAPSHOT_MS
            && replica.entry_count() > 0;
        if maker.as_deref() == Some(me.as_str())
            && settled
            && (replica.entry_count() >= SNAPSHOT_EVERY || deletion_due)
            && self.make_snapshot()
        {
            if let Some(s) = self.sync.as_mut() {
                s.last_snapshot = now;
            }
        }
        self.tick_gaps();
    }

    /// A gap in another device's log that never fills (the device died before its entries went anywhere) is given up
    /// by the install holding the account key, with a signed lost marker (risk 49); every device then goes on past it,
    /// and says how many items could not be recovered.
    fn tick_gaps(&mut self) {
        let Some(s) = self.sync.as_ref() else { return };
        let gaps = s.replica().gaps();
        let now = self.now();
        let mut seen: std::collections::BTreeMap<String, i64> =
            self.kv_json(KV_GAPS).unwrap_or_default();
        let before = seen.clone();
        let key = |g: &(String, u32, u64, u64)| format!("{}/{}/{}-{}", g.0, g.1, g.2, g.3);
        seen.retain(|k, _| gaps.iter().any(|g| &key(g) == k));
        let mut rulings = Vec::new();
        for g in &gaps {
            let since = *seen.entry(key(g)).or_insert(now);
            if now - since < LOST_AFTER_MS {
                continue;
            }
            let Some(account) = self.identity.as_ref() else {
                continue;
            };
            let body = json!({ "device": g.0, "series": g.1, "from": g.2, "to": g.3 });
            if let Ok(r) = privcord_crypto::synclog::account_ruling(account, &body) {
                rulings.push((kind::LOST, r));
                seen.remove(&key(g));
            }
        }
        if seen != before {
            if let Ok(v) = serde_json::to_vec(&seen) {
                let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                    key: KV_GAPS.into(),
                    value: v,
                }]);
            }
        }
        if !rulings.is_empty() {
            self.append_items(rulings);
        }
    }

    /// How many items from other devices of the account could not be recovered (lost markers).
    pub fn sync_lost(&self) -> u64 {
        self.sync.as_ref().map_or(0, |s| s.replica().lost_count())
    }

    /// How many entries the log holds in memory (tests).
    #[doc(hidden)]
    pub fn sync_log_len(&self) -> usize {
        self.sync.as_ref().map_or(0, |s| s.replica().entry_count())
    }

    /// Makes a snapshot now (tests; normally made when the log has grown).
    #[doc(hidden)]
    pub fn make_snapshot(&mut self) -> bool {
        let Some(me) = self.device_id.clone() else {
            return false;
        };
        let Some(snap) = self
            .sync
            .as_ref()
            .and_then(|s| s.replica().make_snapshot(&me))
        else {
            return false;
        };
        let id = snap.id();
        let sh = short(&id);
        let bytes = snap.to_json().into_bytes();
        let Some(secret) = self.media_secret_pub() else {
            return false;
        };
        let Ok(key) = file_key(&secret, &format!("{SNAP_PREFIX}{sh}")) else {
            return false;
        };
        let Ok(sealed) = seal_media_with(&bytes, &key) else {
            return false;
        };
        let r = sealed.reference;
        let ops = vec![
            StorageOp::SetKv {
                key: snap_key(&sh),
                value: bytes,
            },
            StorageOp::SetKv {
                key: ref_key(&sh),
                value: serde_json::to_vec(&r).unwrap_or_default(),
            },
        ];
        if self.ports.storage.apply(&ops).is_err() {
            return false;
        }
        self.queue_upload(&format!("{SNAP_PREFIX}{sh}"));
        let upto = snap.upto.clone();
        self.append_items(vec![(
            kind::SNAPSHOT,
            json!({ "id": id, "upto": upto, "ref": r }),
        )]);
        if let Some(s) = self.sync.as_mut() {
            s.replica_mut().hold(snap);
        }
        true
    }

    /// Prunes to snapshot `id` if every active device holds it, in memory and in storage.
    fn prune_to(&mut self, id: &str, old_base: Option<String>) {
        let old_cut = old_base
            .and_then(|_| self.sync.as_ref()?.replica().base().map(|b| b.upto.clone()))
            .unwrap_or_default();
        let Some(s) = self.sync.as_mut() else { return };
        if s.replica_mut().prune(id) == 0 {
            return;
        }
        let Some(snap) = s.replica().base().cloned() else {
            return;
        };
        let mut ops: Vec<StorageOp> = Vec::new();
        for (stream, upto) in &snap.upto {
            let Some((dev, series)) = stream.rsplit_once('/') else {
                continue;
            };
            let Ok(series) = series.parse::<u32>() else {
                continue;
            };
            let from = old_cut.get(stream).copied().unwrap_or(0) + 1;
            for seq in from..=*upto {
                ops.push(StorageOp::SetKv {
                    key: entry_key(dev, series, seq),
                    value: Vec::new(),
                });
            }
        }
        ops.push(StorageOp::SetKv {
            key: KV_BASE.into(),
            value: snap.to_json().into_bytes(),
        });
        let _ = self.ports.storage.apply(&ops);
    }

    /// A device behind our base asked: make sure the snapshot's pieces are on the relay again.
    pub(super) fn offer_base(&mut self, id: &str) {
        let sh = short(id);
        if self
            .ports
            .storage
            .kv(&snap_key(&sh))
            .ok()
            .flatten()
            .is_none()
        {
            // Kept as the base: save it under its short name, with a reference cut from our own secret if we never
            // had the announced one.
            let Some(base) = self.sync.as_ref().and_then(|s| s.replica().base().cloned()) else {
                return;
            };
            let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                key: snap_key(&sh),
                value: base.to_json().into_bytes(),
            }]);
        }
        if self.kv_json::<MediaRef>(&ref_key(&sh)).is_some() {
            self.queue_upload(&format!("{SNAP_PREFIX}{sh}"));
        }
    }

    /// A snapshot arrived whole (checked against its root): a device behind it starts from it; any other keeps it.
    pub(super) fn snapshot_arrived(&mut self, sh: &str, bytes: Vec<u8>) {
        let Some(snap) = std::str::from_utf8(&bytes)
            .ok()
            .and_then(Snapshot::from_json)
        else {
            return;
        };
        if short(&snap.id()) != sh {
            return;
        }
        let _ = self.ports.storage.apply(&[StorageOp::SetKv {
            key: snap_key(sh),
            value: bytes,
        }]);
        let now = self.now();
        let Some(s) = self.sync.as_mut() else { return };
        if s.replica_mut().install(snap.clone(), now) {
            let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                key: KV_BASE.into(),
                value: snap.to_json().into_bytes(),
            }]);
            self.apply_snapshot_state();
            self.take_entries(Vec::new(), now);
        } else {
            s.replica_mut().hold(snap);
        }
    }

    /// Writes a snapshot's state into storage, the way entries are applied (nothing that exists is replaced).
    fn apply_snapshot_state(&mut self) {
        let Some(s) = self.sync.as_ref() else { return };
        let state = s.replica().state();
        let entry = |k: &str, body: Value| -> Option<Entry> {
            serde_json::from_value(json!({
                "device": "", "series": 0, "seq": 0, "prev": "", "hlc": { "wall": 0, "count": 0 },
                "kind": k, "body": body, "sig": ""
            }))
            .ok()
        };
        let mut entries: Vec<Entry> = Vec::new();
        entries.extend(state.contacts().filter_map(|c| entry("contact", c.clone())));
        entries.extend(
            state
                .message_bodies()
                .filter_map(|(_, b)| entry(kind::MESSAGE, b.clone())),
        );
        entries.extend(
            state
                .groups()
                .filter_map(|(id, evs)| entry("group", json!({ "id": id, "events": evs }))),
        );
        let targets: Vec<String> = state
            .deleted()
            .iter()
            .chain(state.hidden())
            .chain(state.seen_all())
            .chain(state.reacted())
            .chain(state.ticked())
            .cloned()
            .collect();
        entries.extend(
            targets
                .into_iter()
                .filter_map(|t| entry(kind::SEEN, json!({ "target": t }))),
        );
        entries.extend(entry(kind::SETTING, json!({ "key": "muted" })));
        for key in [
            super::prefs::KV_PINS,
            super::prefs::KV_ARCHIVED,
            super::prefs::KV_MUTE_UNTIL,
            super::prefs::KV_QUIET,
            super::prefs::KV_SECURE,
            super::prefs::KV_SILENT,
        ] {
            entries.extend(entry(kind::SETTING, json!({ "key": key })));
        }
        self.apply_entries(&entries);
    }
}
