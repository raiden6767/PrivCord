//! The events a Nostr relay keeps: indexed by time, author, kind and tag, with a log on disk.

use super::event::{Event, Id};
use super::filter::Filter;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

type Key = (i64, Id);

#[derive(Default)]
struct Inner {
    by_id: HashMap<Id, Arc<Event>>,
    by_time: BTreeSet<Key>,
    by_author: HashMap<Id, BTreeSet<Key>>,
    by_kind: HashMap<u64, BTreeSet<Key>>,
    by_tag: HashMap<(char, String), BTreeSet<Key>>,
    /// Which event currently holds a replaceable or addressable slot.
    slots: HashMap<(Id, u64, String), Id>,
    expiring: BTreeSet<Key>,
    /// Deletions asked for, as (target id, who asked). Kept even when the target has not arrived yet, so an event that
    /// shows up after its deletion is refused. One entry per asker: somebody else naming the same id cannot displace the
    /// author's own deletion. Bounded.
    deleted: HashSet<(Id, Id)>,
    /// Addresses (author, kind, d tag) deleted up to and including a time (NIP-09 `a` tags). Bounded.
    deleted_addrs: HashMap<(Id, u64, String), i64>,
    /// Events an administrator removed: refused whoever publishes them. Bounded.
    banned: HashSet<Id>,
    bytes: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Put {
    Stored,
    Duplicate,
    /// A newer event already holds this replaceable slot.
    Stale,
    Deleted,
    /// The store is at its size limit and this event alone is too big to keep.
    Full,
}

pub struct Store {
    inner: RwLock<Inner>,
    log: Option<Mutex<std::fs::File>>,
    path: Option<PathBuf>,
    /// The most bytes of events to hold (0: no limit); the oldest go first when it is exceeded.
    max_bytes: usize,
    lines: std::sync::atomic::AtomicUsize,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("nostr::Store")
    }
}

const MAX_TOMBSTONES: usize = 500_000;
/// The most index entries one query looks at, however many events there are: a wide or hostile filter cannot hold the
/// store's lock for long.
const MAX_SCAN: usize = 200_000;
/// The most events COUNT will count.
const MAX_COUNT: usize = 100_000;

fn tag_keys(e: &Event) -> impl Iterator<Item = (char, String)> + '_ {
    e.tags.iter().filter_map(|t| {
        let name = t.first()?;
        let mut c = name.chars();
        let (Some(letter), None) = (c.next(), c.next()) else {
            return None;
        };
        letter
            .is_ascii_alphabetic()
            .then(|| (letter, t.get(1).cloned().unwrap_or_default()))
    })
}

impl Inner {
    fn add(&mut self, e: Arc<Event>) {
        let key = (e.created_at, e.id);
        self.by_time.insert(key);
        self.by_author.entry(e.pubkey).or_default().insert(key);
        self.by_kind.entry(e.kind).or_default().insert(key);
        for t in tag_keys(&e) {
            self.by_tag.entry(t).or_default().insert(key);
        }
        if let Some(exp) = e.expiration() {
            self.expiring.insert((exp, e.id));
        }
        if let Some(slot) = e.replace_key() {
            self.slots.insert(slot, e.id);
        }
        self.bytes += e.size();
        self.by_id.insert(e.id, e);
    }

    fn remove(&mut self, id: &Id) -> Option<Arc<Event>> {
        let e = self.by_id.remove(id)?;
        let key = (e.created_at, e.id);
        self.by_time.remove(&key);
        if let Some(s) = self.by_author.get_mut(&e.pubkey) {
            s.remove(&key);
            if s.is_empty() {
                self.by_author.remove(&e.pubkey);
            }
        }
        if let Some(s) = self.by_kind.get_mut(&e.kind) {
            s.remove(&key);
            if s.is_empty() {
                self.by_kind.remove(&e.kind);
            }
        }
        for t in tag_keys(&e) {
            if let Some(s) = self.by_tag.get_mut(&t) {
                s.remove(&key);
                if s.is_empty() {
                    self.by_tag.remove(&t);
                }
            }
        }
        if let Some(exp) = e.expiration() {
            self.expiring.remove(&(exp, e.id));
        }
        if let Some(slot) = e.replace_key() {
            if self.slots.get(&slot) == Some(id) {
                self.slots.remove(&slot);
            }
        }
        self.bytes = self.bytes.saturating_sub(e.size());
        Some(e)
    }

    fn tombstone(&mut self, id: Id, by: Id) {
        if self.deleted.len() < MAX_TOMBSTONES {
            self.deleted.insert((id, by));
        }
    }

    /// NIP-09: a kind 5 event removes the events it names, if they are the author's own, and refuses them if they arrive
    /// later. Whether the target is known yet does not matter.
    fn apply_deletion(&mut self, d: &Event) {
        // One deletion names at most this many targets: a single event must not be able to fill the record of deletions.
        for t in d.tags.iter().take(128) {
            match (t.first().map(String::as_str), t.get(1)) {
                (Some("e"), Some(id)) => {
                    if let Some(id) = super::event::unhex::<32>(id) {
                        if self
                            .by_id
                            .get(&id)
                            .is_some_and(|e| e.pubkey == d.pubkey && e.kind != 5)
                        {
                            self.remove(&id);
                        }
                        self.tombstone(id, d.pubkey);
                    }
                }
                (Some("a"), Some(addr)) => {
                    let mut parts = addr.splitn(3, ':');
                    let (Some(kind), Some(pk), Some(dtag)) =
                        (parts.next(), parts.next(), parts.next())
                    else {
                        continue;
                    };
                    let (Ok(kind), Some(pk)) = (kind.parse::<u64>(), super::event::unhex::<32>(pk))
                    else {
                        continue;
                    };
                    if pk != d.pubkey {
                        continue;
                    }
                    let slot = (pk, kind, dtag.to_owned());
                    if let Some(id) = self.slots.get(&slot).copied() {
                        if self
                            .by_id
                            .get(&id)
                            .is_some_and(|e| e.created_at <= d.created_at)
                        {
                            self.remove(&id);
                        }
                    }
                    if self.deleted_addrs.len() < MAX_TOMBSTONES
                        || self.deleted_addrs.contains_key(&slot)
                    {
                        let until = self
                            .deleted_addrs
                            .get(&slot)
                            .copied()
                            .unwrap_or(i64::MIN)
                            .max(d.created_at);
                        self.deleted_addrs.insert(slot, until);
                    }
                }
                _ => {}
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Store {
    /// A store that keeps its log in `dir` (or only in memory when `dir` is `None`), reading back what it finds.
    pub fn open(dir: Option<&std::path::Path>, max_bytes: usize) -> Result<Self, String> {
        let mut store = Self {
            inner: RwLock::new(Inner::default()),
            log: None,
            path: None,
            max_bytes,
            lines: std::sync::atomic::AtomicUsize::new(0),
        };
        if let Some(dir) = dir {
            crate::fsutil::create_private_dir(dir)?;
            let path = dir.join("nostr-events.jsonl");
            if let Ok(text) = std::fs::read_to_string(&path) {
                let mut n = 0;
                for line in text.lines() {
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                        continue;
                    };
                    if let Some(id) = v
                        .get("ban")
                        .and_then(serde_json::Value::as_str)
                        .and_then(super::event::unhex::<32>)
                    {
                        store.ban_inner(id);
                        n += 1;
                    } else if let Some(e) = Event::parse_trusted(&v) {
                        store.insert_inner(Arc::new(e), false);
                        n += 1;
                    }
                }
                store.lines.store(n, std::sync::atomic::Ordering::Relaxed);
            }
            let file = crate::fsutil::open_private_append(&path)
                .map_err(|e| format!("could not open {}: {e}", path.display()))?;
            store.log = Some(Mutex::new(file));
            store.path = Some(path);
        }
        Ok(store)
    }

    fn insert_inner(&self, e: Arc<Event>, log: bool) -> Put {
        let mut w = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        if w.by_id.contains_key(&e.id) {
            return Put::Duplicate;
        }
        if w.banned.contains(&e.id) || w.deleted.contains(&(e.id, e.pubkey)) {
            return Put::Deleted;
        }
        if let Some(slot) = e.replace_key() {
            if w.deleted_addrs
                .get(&slot)
                .is_some_and(|until| e.created_at <= *until)
            {
                return Put::Deleted;
            }
        }
        if let Some(slot) = e.replace_key() {
            if let Some(old_id) = w.slots.get(&slot).copied() {
                if let Some(old) = w.by_id.get(&old_id) {
                    // The newer one wins; at the same second, the lower id.
                    if (old.created_at, std::cmp::Reverse(old.id))
                        >= (e.created_at, std::cmp::Reverse(e.id))
                    {
                        return Put::Stale;
                    }
                }
                w.remove(&old_id);
            }
        }
        if e.kind == 5 {
            w.apply_deletion(&e);
        }
        w.add(e.clone());
        if self.max_bytes > 0 && w.bytes > self.max_bytes {
            // Over the size limit: the oldest events go first (never the one just added, unless it alone is too big).
            let mut over = w.bytes - self.max_bytes;
            let oldest: Vec<Id> = w
                .by_time
                .iter()
                .filter(|k| k.1 != e.id)
                .map(|k| k.1)
                .collect();
            for id in oldest {
                if over == 0 {
                    break;
                }
                if let Some(gone) = w.remove(&id) {
                    over = over.saturating_sub(gone.size());
                }
            }
            if over > 0 {
                w.remove(&e.id);
                return Put::Full;
            }
        }
        drop(w);
        if log {
            if let Some(f) = &self.log {
                let mut line = e.json.to_string();
                line.push('\n');
                let _ = lock(f).write_all(line.as_bytes());
                self.lines
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        Put::Stored
    }

    pub fn insert(&self, e: Arc<Event>) -> Put {
        self.insert_inner(e, true)
    }

    /// Asks the disk to keep what has been written so far (a power cut loses at most what came after the last call).
    pub fn sync(&self) {
        if let Some(f) = &self.log {
            let _ = lock(f).sync_data();
        }
    }

    /// An administrator removes an event for good (moderation): it goes, and nobody can publish it again.
    pub fn ban_event(&self, id: &Id) -> bool {
        let had = self.ban_inner(*id);
        // Written to the log, so the ban survives a restart (and a crash, which would otherwise reload the event).
        if let Some(f) = &self.log {
            let mut line = format!("{{\"ban\":\"{}\"}}\n", super::event::hex(id));
            let _ = lock(f).write_all(std::mem::take(&mut line).as_bytes());
            self.lines
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        had
    }

    fn ban_inner(&self, id: Id) -> bool {
        let mut w = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        let had = w.remove(&id).is_some();
        if w.banned.len() < MAX_TOMBSTONES {
            w.banned.insert(id);
        }
        had
    }

    /// Removes every stored event. Bans and deletions are kept, so what was removed on purpose stays refused.
    pub fn clear(&self) -> usize {
        let removed = {
            let mut w = self.inner.write().unwrap_or_else(PoisonError::into_inner);
            let n = w.by_id.len();
            let keep = (
                std::mem::take(&mut w.deleted),
                std::mem::take(&mut w.deleted_addrs),
                std::mem::take(&mut w.banned),
            );
            *w = Inner::default();
            (w.deleted, w.deleted_addrs, w.banned) = keep;
            n
        };
        self.compact(true);
        removed
    }

    pub fn len(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .by_id
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn bytes(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .bytes
    }

    /// The events that match `f`, newest first, at most `max` of them.
    pub fn query(&self, f: &Filter, now: i64, max: usize) -> Vec<Arc<Event>> {
        let limit = f.limit.map_or(max, |l| l.min(max));
        if limit == 0 {
            return Vec::new();
        }
        let r = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        let live = |e: &Arc<Event>| e.expiration().is_none_or(|x| x > now) && f.matches(e);
        let mut out: Vec<Arc<Event>> = if let Some(ids) = f.exact_ids() {
            ids.iter()
                .filter_map(|id| r.by_id.get(id))
                .filter(|e| live(e))
                .cloned()
                .collect()
        } else {
            let lo = (f.since.unwrap_or(i64::MIN), [0u8; 32]);
            let hi = (f.until.unwrap_or(i64::MAX), [0xffu8; 32]);
            let mut options: Vec<Vec<&BTreeSet<Key>>> = Vec::new();
            for (name, values) in &f.tags {
                options.push(
                    values
                        .iter()
                        .filter_map(|v| r.by_tag.get(&(*name, v.clone())))
                        .collect(),
                );
            }
            if !f.authors.is_empty() && f.authors.iter().all(|a| a.len() == 64) {
                options.push(
                    f.authors
                        .iter()
                        .filter_map(|a| super::event::unhex::<32>(a))
                        .filter_map(|a| r.by_author.get(&a))
                        .collect(),
                );
            }
            if !f.kinds.is_empty() {
                options.push(f.kinds.iter().filter_map(|k| r.by_kind.get(k)).collect());
            }
            // The smallest group of candidate sets that must contain every match.
            let best = options
                .into_iter()
                .min_by_key(|sets| sets.iter().map(|s| s.len()).sum::<usize>());
            let sets = best.unwrap_or_else(|| vec![&r.by_time]);
            let mut found = Vec::new();
            for set in sets {
                if lo > hi {
                    break;
                }
                let mut taken = 0;
                for (steps, key) in set.range(lo..=hi).rev().enumerate() {
                    if steps >= MAX_SCAN {
                        break;
                    }
                    if let Some(e) = r.by_id.get(&key.1) {
                        if live(e) {
                            found.push(e.clone());
                            taken += 1;
                            if taken >= limit {
                                break;
                            }
                        }
                    }
                }
            }
            found
        };
        out.sort_unstable_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        out.dedup_by_key(|e| e.id);
        out.truncate(limit);
        out
    }

    pub fn count(&self, filters: &[Filter], now: i64) -> usize {
        let mut seen = HashSet::new();
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
        let mut w = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        let mut gone = 0;
        let expired: Vec<Id> = w
            .expiring
            .range(..=(now, [0xffu8; 32]))
            .map(|k| k.1)
            .collect();
        for id in expired {
            gone += usize::from(w.remove(&id).is_some());
        }
        if retention_secs > 0 {
            let cutoff = now - retention_secs;
            let old: Vec<Id> = w
                .by_time
                .range(..(cutoff, [0u8; 32]))
                .filter_map(|k| w.by_id.get(&k.1))
                .filter(|e| e.replace_key().is_none() && e.kind != 5)
                .map(|e| e.id)
                .collect();
            for id in old {
                gone += usize::from(w.remove(&id).is_some());
            }
        }
        if max_events > 0 && w.by_id.len() > max_events {
            let excess = w.by_id.len() - max_events;
            let oldest: Vec<Id> = w.by_time.iter().take(excess).map(|k| k.1).collect();
            for id in oldest {
                gone += usize::from(w.remove(&id).is_some());
            }
        }
        gone
    }

    /// Rewrites the log with only what is still alive, when it has grown to twice that.
    pub fn compact(&self, force: bool) {
        let (Some(path), Some(log)) = (&self.path, &self.log) else {
            return;
        };
        let live = self.len();
        if !force
            && self.lines.load(std::sync::atomic::Ordering::Relaxed)
                < live.saturating_mul(2).max(10_000)
        {
            return;
        }
        let r = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        let mut text = String::new();
        for id in &r.banned {
            text.push_str(&format!("{{\"ban\":\"{}\"}}\n", super::event::hex(id)));
        }
        for key in &r.by_time {
            if let Some(e) = r.by_id.get(&key.1) {
                text.push_str(&e.json);
                text.push('\n');
            }
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, &text).is_ok() && std::fs::rename(&tmp, path).is_ok() {
            if let Ok(f) = std::fs::OpenOptions::new().append(true).open(path) {
                *lock(log) = f;
                self.lines.store(live, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}
