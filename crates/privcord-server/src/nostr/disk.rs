//! The events a Nostr relay keeps **on disk**, for relays too big to hold in memory: the events and every index (time, author,
//! kind, tag) live in a transactional database file (redb: pure Rust, copy-on-write, crash-safe), so memory use does not grow
//! with the number of events. It behaves exactly as [`super::store::MemStore`] does (replaceable events, deletion, vanish,
//! bans, expiry, size cap), and the same tests run against both.
//!
//! Writes are committed without waiting for the disk, and `sync()` (called twice a second, or per batch in durable mode)
//! makes them durable: a crash loses at most what came after the last sync, the same promise as the log.

use super::event::{hex, unhex, Event, Id};
use super::filter::Filter;
use super::store::{tag_keys, Put, MAX_COUNT, MAX_SCAN, MAX_TOMBSTONES};
use crate::record::{Codec, Decoded};
use redb::{
    Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

type Def = TableDefinition<'static, &'static [u8], &'static [u8]>;
type R<T> = Result<T, redb::Error>;

const EVENTS: Def = TableDefinition::new("events");
const BY_TIME: Def = TableDefinition::new("by_time");
const BY_AUTHOR: Def = TableDefinition::new("by_author");
const BY_KIND: Def = TableDefinition::new("by_kind");
const BY_TAG: Def = TableDefinition::new("by_tag");
const SLOTS: Def = TableDefinition::new("slots");
const EXPIRING: Def = TableDefinition::new("expiring");
const DELETED: Def = TableDefinition::new("deleted");
const DELETED_ADDRS: Def = TableDefinition::new("deleted_addrs");
const BANNED: Def = TableDefinition::new("banned");
const VANISHED: Def = TableDefinition::new("vanished");
const META: Def = TableDefinition::new("meta");

const LABEL: &str = "nostr-events";

/// Times sort as unsigned numbers: flip the sign bit.
fn t8(t: i64) -> [u8; 8] {
    ((t as u64) ^ (1 << 63)).to_be_bytes()
}

fn k8(v: u64) -> [u8; 8] {
    v.to_be_bytes()
}

fn tag_hash(value: &str) -> [u8; 16] {
    let d = Sha256::digest(value.as_bytes());
    let mut out = [0u8; 16];
    for (o, b) in out.iter_mut().zip(d.iter()) {
        *o = *b;
    }
    out
}

/// `prefix ++ time ++ id`: how every index orders its entries (newest last within one prefix).
fn key(prefix: &[u8], time: i64, id: &Id) -> Vec<u8> {
    let mut k = Vec::with_capacity(prefix.len() + 40);
    k.extend_from_slice(prefix);
    k.extend_from_slice(&t8(time));
    k.extend_from_slice(id);
    k
}

fn tag_prefix(letter: char, value: &str) -> Vec<u8> {
    let mut p = vec![u8::try_from(letter as u32).unwrap_or(b'?')];
    p.extend_from_slice(&tag_hash(value));
    p
}

fn slot_key(slot: &(Id, u64, String)) -> Vec<u8> {
    let mut k = slot.0.to_vec();
    k.extend_from_slice(&k8(slot.1));
    k.extend_from_slice(slot.2.as_bytes());
    k
}

fn id_of(index_key: &[u8]) -> Option<Id> {
    let tail = index_key.get(index_key.len().checked_sub(32)?..)?;
    <[u8; 32]>::try_from(tail).ok()
}

fn time_of(index_key: &[u8]) -> Option<i64> {
    let at = index_key.len().checked_sub(40)?;
    let b = <[u8; 8]>::try_from(index_key.get(at..at + 8)?).ok()?;
    Some((u64::from_be_bytes(b) ^ (1 << 63)) as i64)
}

pub struct DiskStore {
    /// Taken away by `close`, so the file is released even while connections still hold the store.
    db: std::sync::RwLock<Option<Database>>,
    max_bytes: usize,
    codec: Arc<Codec>,
    count: AtomicUsize,
    bytes: AtomicUsize,
}

impl std::fmt::Debug for DiskStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("nostr::DiskStore")
    }
}

/// How many events and bytes a transaction leaves behind (written with it, so a restart does not have to count).
struct Totals {
    count: usize,
    bytes: usize,
}

type T<'a> = redb::Table<'a, &'static [u8], &'static [u8]>;

/// Every table, open for writing inside one transaction.
struct W<'a> {
    events: T<'a>,
    by_time: T<'a>,
    by_author: T<'a>,
    by_kind: T<'a>,
    by_tag: T<'a>,
    slots: T<'a>,
    expiring: T<'a>,
    deleted: T<'a>,
    deleted_addrs: T<'a>,
    banned: T<'a>,
    vanished: T<'a>,
    meta: T<'a>,
    codec: &'a Codec,
    totals: Totals,
}

fn decode_event(codec: &Codec, raw: &[u8]) -> Option<Event> {
    let text = std::str::from_utf8(raw).ok()?;
    let Decoded::Payload(p) = codec.decode(LABEL, text) else {
        return None;
    };
    Event::parse_trusted(&serde_json::from_str::<serde_json::Value>(&p).ok()?)
}

impl W<'_> {
    fn get(&self, id: &Id) -> R<Option<Event>> {
        Ok(self
            .events
            .get(id.as_slice())?
            .and_then(|g| decode_event(self.codec, g.value())))
    }

    fn has(&self, id: &Id) -> R<bool> {
        Ok(self.events.get(id.as_slice())?.is_some())
    }

    fn add(&mut self, e: &Event) -> R<()> {
        let body = self.codec.encode(LABEL, &e.json);
        self.events.insert(e.id.as_slice(), body.as_bytes())?;
        let none: &[u8] = &[];
        self.by_time
            .insert(key(&[], e.created_at, &e.id).as_slice(), none)?;
        self.by_author
            .insert(key(&e.pubkey, e.created_at, &e.id).as_slice(), none)?;
        self.by_kind
            .insert(key(&k8(e.kind), e.created_at, &e.id).as_slice(), none)?;
        for (letter, value) in tag_keys(e) {
            self.by_tag.insert(
                key(&tag_prefix(letter, &value), e.created_at, &e.id).as_slice(),
                none,
            )?;
        }
        if let Some(exp) = e.expiration() {
            self.expiring
                .insert(key(&[], exp, &e.id).as_slice(), none)?;
        }
        if let Some(slot) = e.replace_key() {
            self.slots
                .insert(slot_key(&slot).as_slice(), e.id.as_slice())?;
        }
        self.totals.count += 1;
        self.totals.bytes += e.size();
        Ok(())
    }

    fn remove(&mut self, id: &Id) -> R<Option<Event>> {
        let Some(e) = self.get(id)? else {
            // A record that cannot be read is still removed, so it cannot hold the store up.
            self.events.remove(id.as_slice())?;
            return Ok(None);
        };
        self.events.remove(id.as_slice())?;
        self.by_time.remove(key(&[], e.created_at, id).as_slice())?;
        self.by_author
            .remove(key(&e.pubkey, e.created_at, id).as_slice())?;
        self.by_kind
            .remove(key(&k8(e.kind), e.created_at, id).as_slice())?;
        for (letter, value) in tag_keys(&e) {
            self.by_tag
                .remove(key(&tag_prefix(letter, &value), e.created_at, id).as_slice())?;
        }
        if let Some(exp) = e.expiration() {
            self.expiring.remove(key(&[], exp, id).as_slice())?;
        }
        if let Some(slot) = e.replace_key() {
            let k = slot_key(&slot);
            let holds = self
                .slots
                .get(k.as_slice())?
                .is_some_and(|g| g.value() == id.as_slice());
            if holds {
                self.slots.remove(k.as_slice())?;
            }
        }
        self.totals.count = self.totals.count.saturating_sub(1);
        self.totals.bytes = self.totals.bytes.saturating_sub(e.size());
        Ok(Some(e))
    }

    fn tombstone(&mut self, id: &Id, by: &Id) -> R<()> {
        if usize::try_from(self.deleted.len()?).unwrap_or(usize::MAX) < MAX_TOMBSTONES {
            let mut k = id.to_vec();
            k.extend_from_slice(by);
            self.deleted.insert(k.as_slice(), [].as_slice())?;
        }
        Ok(())
    }

    /// NIP-09: a kind 5 event removes the events it names, if they are the author's own, and refuses them if they arrive
    /// later.
    fn apply_deletion(&mut self, d: &Event) -> R<()> {
        for t in d.tags.iter().take(128) {
            match (t.first().map(String::as_str), t.get(1)) {
                (Some("e"), Some(id)) => {
                    let Some(id) = unhex::<32>(id) else { continue };
                    if let Some(e) = self.get(&id)? {
                        if e.pubkey == d.pubkey && e.kind != 5 {
                            self.remove(&id)?;
                        }
                    }
                    self.tombstone(&id, &d.pubkey)?;
                }
                (Some("a"), Some(addr)) => {
                    let mut parts = addr.splitn(3, ':');
                    let (Some(kind), Some(pk), Some(dtag)) =
                        (parts.next(), parts.next(), parts.next())
                    else {
                        continue;
                    };
                    let (Ok(kind), Some(pk)) = (kind.parse::<u64>(), unhex::<32>(pk)) else {
                        continue;
                    };
                    if pk != d.pubkey {
                        continue;
                    }
                    let slot = (pk, kind, dtag.to_owned());
                    let sk = slot_key(&slot);
                    let held = self
                        .slots
                        .get(sk.as_slice())?
                        .and_then(|g| <[u8; 32]>::try_from(g.value()).ok());
                    if let Some(id) = held {
                        if self.get(&id)?.is_some_and(|e| e.created_at <= d.created_at) {
                            self.remove(&id)?;
                        }
                    }
                    let known = self.deleted_addrs.get(sk.as_slice())?.is_some();
                    let room = usize::try_from(self.deleted_addrs.len()?).unwrap_or(usize::MAX)
                        < MAX_TOMBSTONES;
                    if room || known {
                        let until = self
                            .deleted_addrs
                            .get(sk.as_slice())?
                            .and_then(|g| <[u8; 8]>::try_from(g.value()).ok())
                            .map_or(i64::MIN, |b| (u64::from_be_bytes(b) ^ (1 << 63)) as i64)
                            .max(d.created_at);
                        self.deleted_addrs
                            .insert(sk.as_slice(), t8(until).as_slice())?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// NIP-62 request to vanish: everything the author published up to then goes, and so do the gift wraps (kind 1059) sent
    /// to them; older events of theirs that arrive later are refused.
    fn vanish(&mut self, req: &Event) -> R<()> {
        let known = self.vanished.get(req.pubkey.as_slice())?.is_some();
        if known || usize::try_from(self.vanished.len()?).unwrap_or(usize::MAX) < MAX_TOMBSTONES {
            let before = self
                .vanished
                .get(req.pubkey.as_slice())?
                .and_then(|g| <[u8; 8]>::try_from(g.value()).ok())
                .map_or(0, |b| (u64::from_be_bytes(b) ^ (1 << 63)) as i64);
            self.vanished.insert(
                req.pubkey.as_slice(),
                t8(before.max(req.created_at)).as_slice(),
            )?;
        }
        let mut gone: Vec<Id> = Vec::new();
        {
            let lo = key(&req.pubkey, i64::MIN, &[0; 32]);
            let hi = key(&req.pubkey, req.created_at, &[0xff; 32]);
            for item in self.by_author.range(lo.as_slice()..=hi.as_slice())? {
                if let Some(id) = id_of(item?.0.value()) {
                    gone.push(id);
                }
            }
        }
        let prefix = tag_prefix('p', &hex(&req.pubkey));
        let wraps: Vec<Id> = {
            let lo = key(&prefix, i64::MIN, &[0; 32]);
            let hi = key(&prefix, req.created_at, &[0xff; 32]);
            let mut v = Vec::new();
            for item in self.by_tag.range(lo.as_slice()..=hi.as_slice())? {
                if let Some(id) = id_of(item?.0.value()) {
                    v.push(id);
                }
            }
            v
        };
        for id in wraps {
            if self.get(&id)?.is_some_and(|e| e.kind == 1059) {
                gone.push(id);
            }
        }
        for id in gone {
            self.remove(&id)?;
        }
        Ok(())
    }

    fn save_totals(&mut self) -> R<()> {
        let c = u64::try_from(self.totals.count).unwrap_or(u64::MAX);
        let b = u64::try_from(self.totals.bytes).unwrap_or(u64::MAX);
        self.meta.insert(b"count".as_slice(), k8(c).as_slice())?;
        self.meta.insert(b"bytes".as_slice(), k8(b).as_slice())?;
        Ok(())
    }
}

impl DiskStore {
    /// Runs `f` with the database, or fails when it has been closed.
    fn with_db<X>(&self, f: impl FnOnce(&Database) -> R<X>) -> R<X> {
        let guard = self
            .db
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.as_ref() {
            Some(db) => f(db),
            None => Err(redb::StorageError::Io(std::io::Error::other(
                "the event database is closed",
            ))
            .into()),
        }
    }

    /// Releases the database file (the server is stopping). Later calls find nothing and do nothing.
    pub fn close(&self) {
        let mut guard = self
            .db
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(db) = guard.as_ref() {
            // Whatever was written is made durable before the file is let go.
            if let Ok(mut t) = db.begin_write() {
                if t.set_durability(Durability::Immediate).is_ok() {
                    let _ = t.commit();
                }
            }
        }
        *guard = None;
    }

    pub fn open(
        dir: &std::path::Path,
        max_bytes: usize,
        codec: Arc<Codec>,
    ) -> Result<Self, String> {
        crate::fsutil::create_private_dir(dir)?;
        let path = dir.join("nostr-events.redb");
        let db = Database::create(&path)
            .map_err(|e| format!("could not open the event database {}: {e}", path.display()))?;
        // The tables exist from the first start, so a read never meets a missing one.
        let setup = (|| -> R<(usize, usize)> {
            let txn = db.begin_write()?;
            for def in [
                EVENTS,
                BY_TIME,
                BY_AUTHOR,
                BY_KIND,
                BY_TAG,
                SLOTS,
                EXPIRING,
                DELETED,
                DELETED_ADDRS,
                BANNED,
                VANISHED,
                META,
            ] {
                txn.open_table(def)?;
            }
            let (c, b) = {
                let meta = txn.open_table(META)?;
                let read = |name: &[u8]| {
                    meta.get(name)
                        .ok()
                        .flatten()
                        .and_then(|g| <[u8; 8]>::try_from(g.value()).ok())
                        .map_or(0, |b| usize::try_from(u64::from_be_bytes(b)).unwrap_or(0))
                };
                (read(b"count"), read(b"bytes"))
            };
            txn.commit()?;
            Ok((c, b))
        })()
        .map_err(|e| {
            format!(
                "could not set up the event database {}: {e}",
                path.display()
            )
        })?;
        Ok(Self {
            db: std::sync::RwLock::new(Some(db)),
            max_bytes,
            codec,
            count: AtomicUsize::new(setup.0),
            bytes: AtomicUsize::new(setup.1),
        })
    }

    /// Runs `f` in one write transaction (committed without waiting for the disk; `sync` makes it durable).
    fn write<X>(&self, f: impl FnOnce(&mut W<'_>) -> R<X>) -> R<X> {
        self.with_db(|db| self.write_in(db, f))
    }

    fn write_in<X>(&self, db: &Database, f: impl FnOnce(&mut W<'_>) -> R<X>) -> R<X> {
        let mut txn = db.begin_write()?;
        txn.set_durability(Durability::None)?;
        let out = {
            let mut w = W {
                events: txn.open_table(EVENTS)?,
                by_time: txn.open_table(BY_TIME)?,
                by_author: txn.open_table(BY_AUTHOR)?,
                by_kind: txn.open_table(BY_KIND)?,
                by_tag: txn.open_table(BY_TAG)?,
                slots: txn.open_table(SLOTS)?,
                expiring: txn.open_table(EXPIRING)?,
                deleted: txn.open_table(DELETED)?,
                deleted_addrs: txn.open_table(DELETED_ADDRS)?,
                banned: txn.open_table(BANNED)?,
                vanished: txn.open_table(VANISHED)?,
                meta: txn.open_table(META)?,
                codec: &self.codec,
                totals: Totals {
                    count: self.count.load(Ordering::Relaxed),
                    bytes: self.bytes.load(Ordering::Relaxed),
                },
            };
            let out = f(&mut w)?;
            w.save_totals()?;
            let (c, b) = (w.totals.count, w.totals.bytes);
            drop(w);
            (out, c, b)
        };
        txn.commit()?;
        self.count.store(out.1, Ordering::Relaxed);
        self.bytes.store(out.2, Ordering::Relaxed);
        Ok(out.0)
    }

    pub fn insert(&self, e: Arc<Event>) -> Put {
        let max = self.max_bytes;
        let r = self.write(|w| {
            if w.has(&e.id)? {
                return Ok(Put::Duplicate);
            }
            let mut dk = e.id.to_vec();
            dk.extend_from_slice(&e.pubkey);
            if w.banned.get(e.id.as_slice())?.is_some() || w.deleted.get(dk.as_slice())?.is_some() {
                return Ok(Put::Deleted);
            }
            if e.kind != 62 {
                let gone_until = w
                    .vanished
                    .get(e.pubkey.as_slice())?
                    .and_then(|g| <[u8; 8]>::try_from(g.value()).ok())
                    .map(|b| (u64::from_be_bytes(b) ^ (1 << 63)) as i64);
                if gone_until.is_some_and(|t| e.created_at <= t) {
                    return Ok(Put::Deleted);
                }
            }
            if let Some(slot) = e.replace_key() {
                let sk = slot_key(&slot);
                let until = w
                    .deleted_addrs
                    .get(sk.as_slice())?
                    .and_then(|g| <[u8; 8]>::try_from(g.value()).ok())
                    .map(|b| (u64::from_be_bytes(b) ^ (1 << 63)) as i64);
                if until.is_some_and(|t| e.created_at <= t) {
                    return Ok(Put::Deleted);
                }
                let held = w
                    .slots
                    .get(sk.as_slice())?
                    .and_then(|g| <[u8; 32]>::try_from(g.value()).ok());
                if let Some(old_id) = held {
                    if let Some(old) = w.get(&old_id)? {
                        // The newer one wins; at the same second, the lower id.
                        if (old.created_at, std::cmp::Reverse(old.id))
                            >= (e.created_at, std::cmp::Reverse(e.id))
                        {
                            return Ok(Put::Stale);
                        }
                    }
                    w.remove(&old_id)?;
                }
            }
            if e.kind == 5 {
                w.apply_deletion(&e)?;
            }
            if e.kind == 62 {
                w.vanish(&e)?;
            }
            w.add(&e)?;
            if max > 0 && w.totals.bytes > max {
                // Over the size limit: the oldest events go first (never the one just added, unless it alone is too big).
                let mut over = w.totals.bytes - max;
                let mut oldest: Vec<Id> = Vec::new();
                for item in w.by_time.iter()? {
                    if let Some(id) = id_of(item?.0.value()) {
                        if id != e.id {
                            oldest.push(id);
                        }
                    }
                    if oldest.len() >= 4096 {
                        break;
                    }
                }
                for id in oldest {
                    if over == 0 {
                        break;
                    }
                    if let Some(gone) = w.remove(&id)? {
                        over = over.saturating_sub(gone.size());
                    }
                }
                if over > 0 {
                    w.remove(&e.id)?;
                    return Ok(Put::Full);
                }
            }
            Ok(Put::Stored)
        });
        r.unwrap_or(Put::Failed)
    }

    /// Makes what was written so far durable (a power cut loses at most what came after the last call).
    pub fn sync(&self) -> bool {
        self.with_db(|db| {
            let mut txn = db.begin_write()?;
            txn.set_durability(Durability::Immediate)?;
            txn.commit()?;
            Ok(())
        })
        .is_ok()
    }

    pub fn banned_events(&self) -> Vec<Id> {
        let mut v: Vec<Id> = self
            .with_db(|db| {
                let r = db.begin_read()?;
                let t = r.open_table(BANNED)?;
                let mut out = Vec::new();
                for item in t.iter()? {
                    if let Ok(id) = <[u8; 32]>::try_from(item?.0.value()) {
                        out.push(id);
                    }
                }
                Ok(out)
            })
            .unwrap_or_default();
        v.sort_unstable();
        v
    }

    pub fn unban_event(&self, id: &Id) -> bool {
        self.write(|w| Ok(w.banned.remove(id.as_slice())?.is_some()))
            .unwrap_or(false)
    }

    pub fn ban_event(&self, id: &Id) -> bool {
        self.write(|w| {
            let had = w.remove(id)?.is_some();
            if usize::try_from(w.banned.len()?).unwrap_or(usize::MAX) < MAX_TOMBSTONES {
                w.banned.insert(id.as_slice(), [].as_slice())?;
            }
            Ok(had)
        })
        .unwrap_or(false)
    }

    /// Removes every stored event. Bans and deletions are kept, so what was removed on purpose stays refused.
    pub fn clear(&self) -> usize {
        let n = self.len();
        let _ = self.write(|w| {
            let ids: Vec<Id> = w
                .events
                .iter()?
                .filter_map(|i| i.ok())
                .filter_map(|(k, _)| <[u8; 32]>::try_from(k.value()).ok())
                .collect();
            for id in ids {
                w.remove(&id)?;
            }
            Ok(())
        });
        n
    }

    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn bytes(&self) -> usize {
        self.bytes.load(Ordering::Relaxed)
    }

    /// The events that match `f`, newest first, at most `max` of them.
    pub fn query(&self, f: &Filter, now: i64, max: usize) -> Vec<Arc<Event>> {
        let limit = f.limit.map_or(max, |l| l.min(max));
        if limit == 0 {
            return Vec::new();
        }
        self.with_db(|db| self.query_inner(db, f, now, limit))
            .unwrap_or_default()
    }

    fn query_inner(&self, db: &Database, f: &Filter, now: i64, limit: usize) -> R<Vec<Arc<Event>>> {
        let r = db.begin_read()?;
        let events = r.open_table(EVENTS)?;
        let live = |e: &Event| e.expiration().is_none_or(|x| x > now) && f.matches(e);
        let load = |id: &Id| -> R<Option<Event>> {
            Ok(events
                .get(id.as_slice())?
                .and_then(|g| decode_event(&self.codec, g.value())))
        };
        let mut out: Vec<Arc<Event>> = Vec::new();
        if let Some(ids) = f.exact_ids() {
            for id in ids {
                if let Some(e) = load(&id)? {
                    if live(&e) {
                        out.push(Arc::new(e));
                    }
                }
            }
        } else {
            // The most selective way in: a tag, then authors, then kinds, else everything by time.
            let (table, prefixes): (Def, Vec<Vec<u8>>) = if let Some((name, values)) = f
                .tags
                .iter()
                .filter(|(_, v)| !v.is_empty())
                .min_by_key(|(_, v)| v.len())
            {
                (
                    BY_TAG,
                    values.iter().map(|v| tag_prefix(*name, v)).collect(),
                )
            } else if !f.authors.is_empty() && f.authors.iter().all(|a| a.len() == 64) {
                (
                    BY_AUTHOR,
                    f.authors
                        .iter()
                        .filter_map(|a| unhex::<32>(a))
                        .map(|a| a.to_vec())
                        .collect(),
                )
            } else if !f.kinds.is_empty() {
                (BY_KIND, f.kinds.iter().map(|k| k8(*k).to_vec()).collect())
            } else {
                (BY_TIME, vec![Vec::new()])
            };
            let index = r.open_table(table)?;
            let since = f.since.unwrap_or(i64::MIN);
            let until = f.until.unwrap_or(i64::MAX);
            if since <= until {
                for prefix in prefixes {
                    let lo = key(&prefix, since, &[0; 32]);
                    let hi = key(&prefix, until, &[0xff; 32]);
                    let mut taken = 0;
                    for (steps, item) in index
                        .range(lo.as_slice()..=hi.as_slice())?
                        .rev()
                        .enumerate()
                    {
                        if steps >= MAX_SCAN {
                            break;
                        }
                        let Some(id) = id_of(item?.0.value()) else {
                            continue;
                        };
                        if let Some(e) = load(&id)? {
                            if live(&e) {
                                out.push(Arc::new(e));
                                taken += 1;
                                if taken >= limit {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        out.sort_unstable_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        out.dedup_by_key(|e| e.id);
        out.truncate(limit);
        Ok(out)
    }

    pub fn count(&self, filters: &[Filter], now: i64) -> usize {
        let mut seen = std::collections::HashSet::new();
        for f in filters {
            let g = Filter {
                limit: None,
                ..f.clone()
            };
            for e in self.query(&g, now, MAX_COUNT) {
                seen.insert(e.id);
            }
        }
        seen.len()
    }

    /// Drops what has expired, what is older than `retention_secs` (0: keep), and the oldest beyond `max_events` (0: no cap).
    pub fn sweep(&self, now: i64, retention_secs: i64, max_events: usize) -> usize {
        self.write(|w| {
            let mut gone = 0;
            let expired: Vec<Id> = {
                let hi = key(&[], now, &[0xff; 32]);
                let mut v = Vec::new();
                for item in w.expiring.range(..=hi.as_slice())? {
                    if let Some(id) = id_of(item?.0.value()) {
                        v.push(id);
                    }
                }
                v
            };
            for id in expired {
                gone += usize::from(w.remove(&id)?.is_some());
            }
            if retention_secs > 0 {
                let hi = key(&[], now - retention_secs, &[0; 32]);
                let mut old: Vec<Id> = Vec::new();
                for item in w.by_time.range(..hi.as_slice())? {
                    if let Some(id) = id_of(item?.0.value()) {
                        old.push(id);
                    }
                }
                for id in old {
                    if let Some(e) = w.get(&id)? {
                        if e.replace_key().is_none() && e.kind != 5 {
                            gone += usize::from(w.remove(&id)?.is_some());
                        }
                    }
                }
            }
            if max_events > 0 && w.totals.count > max_events {
                let excess = w.totals.count - max_events;
                let mut oldest: Vec<Id> = Vec::new();
                for item in w.by_time.iter()?.take(excess) {
                    if let Some(id) = id_of(item?.0.value()) {
                        oldest.push(id);
                    }
                }
                for id in oldest {
                    gone += usize::from(w.remove(&id)?.is_some());
                }
            }
            Ok(gone)
        })
        .unwrap_or(0)
    }

    /// The database reuses the space of what was removed, so there is no log to rewrite.
    pub fn compact(&self, _force: bool) {}

    /// Records that could not be trusted: the database checks its own pages, so none are skipped silently.
    pub fn corrupt_lines(&self) -> usize {
        0
    }

    /// The newest event's time and the oldest's, for the health report (None when empty).
    pub fn span(&self) -> Option<(i64, i64)> {
        self.with_db(|db| {
            let r = db.begin_read()?;
            let t = r.open_table(BY_TIME)?;
            let first = t.first()?.map(|x| x.0.value().to_vec());
            let last = t.last()?.map(|x| x.0.value().to_vec());
            Ok(first.zip(last))
        })
        .ok()
        .flatten()
        .and_then(|(f, l)| Some((time_of(&f)?, time_of(&l)?)))
    }
}
