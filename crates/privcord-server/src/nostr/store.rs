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
    /// Ids that were deleted, so the same event cannot be put back. Bounded.
    deleted: HashSet<Id>,
    bytes: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Put {
    Stored,
    Duplicate,
    /// A newer event already holds this replaceable slot.
    Stale,
    Deleted,
}

pub struct Store {
    inner: RwLock<Inner>,
    log: Option<Mutex<std::fs::File>>,
    path: Option<PathBuf>,
    lines: std::sync::atomic::AtomicUsize,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("nostr::Store")
    }
}

const MAX_TOMBSTONES: usize = 200_000;

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

    fn tombstone(&mut self, id: Id) {
        if self.deleted.len() < MAX_TOMBSTONES {
            self.deleted.insert(id);
        }
    }

    /// NIP-09: a kind 5 event removes the events it names, if they are the author's own.
    fn apply_deletion(&mut self, d: &Event) {
        for t in &d.tags {
            match (t.first().map(String::as_str), t.get(1)) {
                (Some("e"), Some(id)) => {
                    if let Some(id) = super::event::unhex::<32>(id) {
                        if self
                            .by_id
                            .get(&id)
                            .is_some_and(|e| e.pubkey == d.pubkey && e.kind != 5)
                        {
                            self.remove(&id);
                            self.tombstone(id);
                        }
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
                    if let Some(id) = self.slots.get(&(pk, kind, dtag.to_owned())).copied() {
                        if self
                            .by_id
                            .get(&id)
                            .is_some_and(|e| e.created_at <= d.created_at)
                        {
                            self.remove(&id);
                            self.tombstone(id);
                        }
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
    pub fn open(dir: Option<&std::path::Path>) -> Result<Self, String> {
        let mut store = Self {
            inner: RwLock::new(Inner::default()),
            log: None,
            path: None,
            lines: std::sync::atomic::AtomicUsize::new(0),
        };
        if let Some(dir) = dir {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
            let path = dir.join("nostr-events.jsonl");
            if let Ok(text) = std::fs::read_to_string(&path) {
                let mut n = 0;
                for line in text.lines() {
                    if let Some(e) = serde_json::from_str(line)
                        .ok()
                        .and_then(|v| Event::parse_trusted(&v))
                    {
                        store.insert_inner(Arc::new(e), false);
                        n += 1;
                    }
                }
                store.lines.store(n, std::sync::atomic::Ordering::Relaxed);
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
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
        if w.deleted.contains(&e.id) {
            return Put::Deleted;
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
                for key in set.range(lo..=hi).rev() {
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
            for e in self.query(&g, now, usize::MAX) {
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
