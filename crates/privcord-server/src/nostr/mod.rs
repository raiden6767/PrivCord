//! Nostr relay protocol (NIP-01 and friends) on the same port as the PrivCord protocol. A client that sends a JSON array is
//! talking Nostr; one that sends an object is talking PrivCord.
//!
//! Supported: NIP-01 (events, REQ/CLOSE, replaceable, ephemeral and addressable kinds), NIP-09 (deletion), NIP-11 (relay
//! information), NIP-13 (proof of work), NIP-40 (expiration), NIP-42 (authentication), NIP-45 (COUNT).

pub mod event;
pub mod filter;
pub mod store;

use crate::config::NostrConfig;
use crate::relay::Conn;
use event::{hex, unhex, Event, Id};
use filter::{any_match, Filter};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use store::{Put, Store};

const SHARDS: usize = 64;
const AUTH_KIND: u64 = 22242;

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
enum SubKey {
    Tag(char, String),
    Author(Id),
    Kind(u64),
    All,
}

#[derive(Clone, Debug)]
struct SubRef {
    conn: Conn,
    id: Arc<str>,
    filters: Arc<Vec<Filter>>,
}

/// What one connection has open.
#[derive(Default, Debug)]
pub struct Session {
    subs: HashMap<String, Vec<SubKey>>,
    challenge: Option<String>,
    authed: Option<Id>,
}

/// Who may publish what. Changeable while the server runs (the admin panel does).
#[derive(Default, Clone, Debug)]
pub struct Policy {
    pub allowed_pubkeys: HashSet<Id>,
    pub blocked_pubkeys: HashSet<Id>,
    pub allowed_kinds: HashSet<u64>,
    pub blocked_kinds: HashSet<u64>,
}

impl Policy {
    pub fn from_config(c: &NostrConfig) -> Self {
        let keys = |v: &[String]| v.iter().filter_map(|s| unhex::<32>(s)).collect();
        Self {
            allowed_pubkeys: keys(&c.allowed_pubkeys),
            blocked_pubkeys: keys(&c.blocked_pubkeys),
            allowed_kinds: c.allowed_kinds.iter().copied().collect(),
            blocked_kinds: c.blocked_kinds.iter().copied().collect(),
        }
    }
}

#[derive(Default, Debug)]
struct Stats {
    received: AtomicU64,
    stored: AtomicU64,
    rejected: AtomicU64,
    delivered: AtomicU64,
    reqs: AtomicU64,
}

pub struct Nostr {
    pub cfg: NostrConfig,
    store: Option<Store>,
    policy: RwLock<Policy>,
    subs: Vec<Mutex<HashMap<SubKey, Vec<SubRef>>>>,
    rates: Mutex<HashMap<String, (u64, i64)>>,
    stats: Stats,
    last_slow_sweep: AtomicU64,
}

impl std::fmt::Debug for Nostr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Nostr")
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn shard_of(k: &SubKey) -> usize {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    k.hash(&mut h);
    (h.finish() % SHARDS as u64) as usize
}

fn frame_event(sub: &str, e: &Event) -> Arc<str> {
    format!("[\"EVENT\",{},{}]", Value::from(sub), e.json).into()
}

impl Nostr {
    pub fn new(cfg: NostrConfig, data_dir: &std::path::Path) -> Result<Self, String> {
        let store = cfg
            .store_events
            .then(|| Store::open(Some(data_dir)))
            .transpose()?;
        let policy = Policy::from_config(&cfg);
        Ok(Self {
            cfg,
            store,
            policy: RwLock::new(policy),
            subs: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
            rates: Mutex::new(HashMap::new()),
            stats: Stats::default(),
            last_slow_sweep: AtomicU64::new(0),
        })
    }

    #[allow(clippy::indexing_slicing)] // shard_of is below SHARDS
    fn shard(&self, k: &SubKey) -> &Mutex<HashMap<SubKey, Vec<SubRef>>> {
        &self.subs[shard_of(k)]
    }

    pub fn policy(&self) -> Policy {
        self.policy
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set_policy(&self, p: Policy) {
        *self.policy.write().unwrap_or_else(PoisonError::into_inner) = p;
    }

    pub fn store(&self) -> Option<&Store> {
        self.store.as_ref()
    }

    pub fn counters(&self) -> Value {
        json!({
            "events": self.store.as_ref().map_or(0, Store::len),
            "bytes": self.store.as_ref().map_or(0, Store::bytes),
            "received": self.stats.received.load(Ordering::Relaxed),
            "stored": self.stats.stored.load(Ordering::Relaxed),
            "rejected": self.stats.rejected.load(Ordering::Relaxed),
            "delivered": self.stats.delivered.load(Ordering::Relaxed),
            "reqs": self.stats.reqs.load(Ordering::Relaxed),
        })
    }

    /// NIP-11.
    pub fn info(&self, contact: &str) -> Value {
        let c = &self.cfg;
        let mut doc = json!({
            "name": c.name,
            "description": c.description,
            "contact": contact,
            "supported_nips": [1, 9, 11, 13, 40, 42, 45],
            "software": "https://github.com/raiden6767/privcord",
            "version": env!("CARGO_PKG_VERSION"),
            "limitation": {
                "max_message_length": c.max_message_bytes,
                "max_subscriptions": c.max_subs_per_conn,
                "max_filters": c.max_filters,
                "max_limit": c.max_limit,
                "max_subid_length": c.max_subid_len,
                "max_event_tags": c.max_tags,
                "max_content_length": c.max_event_bytes,
                "min_pow_difficulty": c.min_pow,
                "auth_required": c.auth_required,
                "restricted_writes": !c.allowed_pubkeys.is_empty() || !c.allowed_kinds.is_empty() || c.auth_required,
                "created_at_upper_limit": c.created_at_upper_secs,
            },
        });
        if let (false, Some(o)) = (c.icon.is_empty(), doc.as_object_mut()) {
            o.insert("icon".into(), Value::from(c.icon.clone()));
        }
        doc
    }

    fn rate_limited(&self, ip: &str, now_ms: i64) -> bool {
        let limit = self.cfg.events_per_minute_per_ip;
        if limit == 0 {
            return false;
        }
        let mut rates = lock(&self.rates);
        let r = rates.entry(ip.to_owned()).or_insert((0, now_ms + 60_000));
        if r.1 <= now_ms {
            *r = (0, now_ms + 60_000);
        }
        r.0 += 1;
        r.0 > limit
    }

    /// Handles one Nostr frame from a connection.
    pub fn frame(&self, conn: &Conn, sess: &mut Session, ip: &str, raw: &str, now_ms: i64) {
        let notice = |m: &str| conn.send(json!(["NOTICE", m]).to_string());
        if self.cfg.auth_required && sess.challenge.is_none() {
            let mut b = [0u8; 16];
            let _ = getrandom::fill(&mut b);
            let ch = hex(&b);
            conn.send(json!(["AUTH", ch]).to_string());
            sess.challenge = Some(ch);
        }
        let Ok(Value::Array(msg)) = serde_json::from_str::<Value>(raw) else {
            return notice("error: that is not a JSON array");
        };
        let now = now_ms / 1000;
        match msg.first().and_then(Value::as_str) {
            Some("EVENT") => match msg.get(1) {
                Some(ev) => self.on_event(conn, sess, ip, ev, raw.len(), now_ms),
                None => notice("error: EVENT needs an event"),
            },
            Some("REQ") => self.on_req(conn, sess, &msg, now),
            Some("COUNT") => self.on_count(conn, sess, &msg, now),
            Some("CLOSE") => {
                if let Some(id) = msg.get(1).and_then(Value::as_str) {
                    self.close_sub(sess, id, conn);
                }
            }
            Some("AUTH") => self.on_auth(conn, sess, msg.get(1), now),
            _ => notice("error: unknown message"),
        }
    }

    fn on_event(
        &self,
        conn: &Conn,
        sess: &Session,
        ip: &str,
        ev: &Value,
        raw_len: usize,
        now_ms: i64,
    ) {
        self.stats.received.fetch_add(1, Ordering::Relaxed);
        let c = &self.cfg;
        let id_text = ev
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let reject = |why: &str| {
            self.stats.rejected.fetch_add(1, Ordering::Relaxed);
            conn.send(json!(["OK", id_text, false, why]).to_string());
        };
        if self.rate_limited(ip, now_ms) {
            return reject("rate-limited: slow down");
        }
        if raw_len > c.max_event_bytes + 512 {
            return reject("invalid: event too large");
        }
        let e = match Event::parse(ev) {
            Ok(e) => e,
            Err(why) => return reject(why),
        };
        let now = now_ms / 1000;
        if e.tags.len() > c.max_tags {
            return reject("invalid: too many tags");
        }
        if e.created_at > now + c.created_at_upper_secs {
            return reject("invalid: created_at is too far in the future");
        }
        if c.created_at_lower_secs > 0 && e.created_at < now - c.created_at_lower_secs {
            return reject("invalid: created_at is too old");
        }
        if e.kind == AUTH_KIND {
            return reject("invalid: authentication events are not published");
        }
        if c.auth_required && sess.authed.is_none() {
            return reject("auth-required: authenticate first");
        }
        {
            let p = self.policy.read().unwrap_or_else(PoisonError::into_inner);
            if p.blocked_pubkeys.contains(&e.pubkey) || p.blocked_kinds.contains(&e.kind) {
                return reject("blocked: this author or kind is not accepted here");
            }
            if (!p.allowed_pubkeys.is_empty() && !p.allowed_pubkeys.contains(&e.pubkey))
                || (!p.allowed_kinds.is_empty() && !p.allowed_kinds.contains(&e.kind))
            {
                return reject(
                    "restricted: this relay only takes events from approved authors and kinds",
                );
            }
        }
        if c.min_pow > 0 && e.pow_bits() < c.min_pow {
            return reject(&format!(
                "pow: a difficulty of {} bits is required",
                c.min_pow
            ));
        }
        if e.expiration().is_some_and(|x| x <= now) {
            return reject("invalid: the event has already expired");
        }
        let e = Arc::new(e);
        let id = hex(&e.id);
        if !e.is_ephemeral() {
            if let Some(store) = &self.store {
                match store.insert(e.clone()) {
                    Put::Stored => {
                        self.stats.stored.fetch_add(1, Ordering::Relaxed);
                    }
                    Put::Duplicate => {
                        return conn.send(
                            json!(["OK", id, true, "duplicate: already have this event"])
                                .to_string(),
                        )
                    }
                    Put::Stale => {
                        return conn.send(
                            json!(["OK", id, true, "duplicate: a newer version is stored"])
                                .to_string(),
                        )
                    }
                    Put::Deleted => return reject("blocked: this event was deleted"),
                }
            }
        }
        self.fan_out(&e);
        conn.send(json!(["OK", id, true, ""]).to_string());
    }

    fn keys_for(e: &Event) -> Vec<SubKey> {
        let mut keys = vec![SubKey::All, SubKey::Kind(e.kind), SubKey::Author(e.pubkey)];
        for t in &e.tags {
            if let (Some(name), Some(v)) = (t.first(), t.get(1)) {
                let mut ch = name.chars();
                if let (Some(c), None) = (ch.next(), ch.next()) {
                    if c.is_ascii_alphabetic() {
                        keys.push(SubKey::Tag(c, v.clone()));
                    }
                }
            }
        }
        keys
    }

    fn fan_out(&self, e: &Arc<Event>) {
        let mut seen: Vec<(u64, Arc<str>)> = Vec::new();
        for key in Self::keys_for(e) {
            let candidates: Vec<SubRef> = lock(self.shard(&key))
                .get(&key)
                .cloned()
                .unwrap_or_default();
            for s in candidates {
                if seen.iter().any(|(c, i)| *c == s.conn.id && **i == *s.id) {
                    continue;
                }
                seen.push((s.conn.id, s.id.clone()));
                if any_match(&s.filters, e) {
                    self.stats.delivered.fetch_add(1, Ordering::Relaxed);
                    s.conn.send(frame_event(&s.id, e));
                }
            }
        }
    }

    fn keys_of_filter(f: &Filter) -> Vec<SubKey> {
        if let Some((name, values)) = f.tags.first() {
            return values
                .iter()
                .map(|v| SubKey::Tag(*name, v.clone()))
                .collect();
        }
        if !f.authors.is_empty() && f.authors.iter().all(|a| a.len() == 64) {
            return f
                .authors
                .iter()
                .filter_map(|a| unhex(a))
                .map(SubKey::Author)
                .collect();
        }
        if !f.kinds.is_empty() {
            return f.kinds.iter().map(|k| SubKey::Kind(*k)).collect();
        }
        vec![SubKey::All]
    }

    fn close_sub(&self, sess: &mut Session, id: &str, conn: &Conn) {
        if let Some(keys) = sess.subs.remove(id) {
            for key in keys {
                let mut shard = lock(self.shard(&key));
                if let Some(list) = shard.get_mut(&key) {
                    list.retain(|s| !(s.conn.id == conn.id && *s.id == *id));
                    if list.is_empty() {
                        shard.remove(&key);
                    }
                }
            }
        }
    }

    /// Forgets everything a connection had open.
    pub fn leave(&self, sess: &mut Session, conn: &Conn) {
        let ids: Vec<String> = sess.subs.keys().cloned().collect();
        for id in ids {
            self.close_sub(sess, &id, conn);
        }
    }

    fn parse_filters(&self, msg: &[Value], from: usize) -> Result<Vec<Filter>, String> {
        let raw = msg.get(from..).unwrap_or(&[]);
        if raw.is_empty() {
            return Err("invalid: no filter".into());
        }
        if raw.len() > self.cfg.max_filters {
            return Err("invalid: too many filters".into());
        }
        raw.iter()
            .map(|v| Filter::parse(v).map_err(str::to_owned))
            .collect()
    }

    fn on_req(&self, conn: &Conn, sess: &mut Session, msg: &[Value], now: i64) {
        self.stats.reqs.fetch_add(1, Ordering::Relaxed);
        let Some(sub) = msg.get(1).and_then(Value::as_str) else {
            return conn.send(json!(["NOTICE", "error: REQ needs a subscription id"]).to_string());
        };
        let closed = |why: &str| conn.send(json!(["CLOSED", sub, why]).to_string());
        if sub.is_empty() || sub.len() > self.cfg.max_subid_len {
            return closed("invalid: bad subscription id");
        }
        if self.cfg.auth_required && sess.authed.is_none() {
            return closed("auth-required: authenticate first");
        }
        if !sess.subs.contains_key(sub) && sess.subs.len() >= self.cfg.max_subs_per_conn {
            return closed("error: too many subscriptions");
        }
        let filters = match self.parse_filters(msg, 2) {
            Ok(f) => f,
            Err(why) => return closed(&why),
        };
        self.close_sub(sess, sub, conn);
        // Listen first, then read what is stored, so nothing published in between is missed (a client may see one twice).
        let filters = Arc::new(filters);
        let mut keys = Vec::new();
        let id: Arc<str> = Arc::from(sub);
        for f in filters.iter() {
            for key in Self::keys_of_filter(f) {
                let mut shard = lock(self.shard(&key));
                let list = shard.entry(key.clone()).or_default();
                if !list.iter().any(|s| s.conn.id == conn.id && s.id == id) {
                    list.push(SubRef {
                        conn: conn.clone(),
                        id: id.clone(),
                        filters: filters.clone(),
                    });
                }
                keys.push(key);
            }
        }
        sess.subs.insert(sub.to_owned(), keys);
        if let Some(store) = &self.store {
            let mut sent: HashSet<Id> = HashSet::new();
            for f in filters.iter() {
                if f.limit == Some(0) {
                    continue;
                }
                for e in store.query(f, now, self.cfg.max_limit) {
                    if sent.insert(e.id) {
                        conn.send(frame_event(sub, &e));
                    }
                }
            }
        }
        conn.send(json!(["EOSE", sub]).to_string());
    }

    fn on_count(&self, conn: &Conn, sess: &Session, msg: &[Value], now: i64) {
        let Some(sub) = msg.get(1).and_then(Value::as_str) else {
            return conn.send(json!(["NOTICE", "error: COUNT needs an id"]).to_string());
        };
        if self.cfg.auth_required && sess.authed.is_none() {
            return conn
                .send(json!(["CLOSED", sub, "auth-required: authenticate first"]).to_string());
        }
        match self.parse_filters(msg, 2) {
            Ok(f) => {
                let n = self.store.as_ref().map_or(0, |s| s.count(&f, now));
                conn.send(json!(["COUNT", sub, { "count": n }]).to_string());
            }
            Err(why) => conn.send(json!(["CLOSED", sub, why]).to_string()),
        }
    }

    fn on_auth(&self, conn: &Conn, sess: &mut Session, ev: Option<&Value>, now: i64) {
        let Some(ev) = ev else {
            return conn.send(json!(["NOTICE", "error: AUTH needs an event"]).to_string());
        };
        let id = ev
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let fail = |why: &str| conn.send(json!(["OK", id, false, why]).to_string());
        let e = match Event::parse(ev) {
            Ok(e) => e,
            Err(why) => return fail(why),
        };
        let challenge_ok = sess
            .challenge
            .as_deref()
            .is_some_and(|c| e.tag("challenge") == Some(c));
        if e.kind != AUTH_KIND
            || !challenge_ok
            || (e.created_at - now).abs() > 600
            || e.tag("relay").is_none()
        {
            return fail("invalid: not a valid authentication for this connection");
        }
        sess.authed = Some(e.pubkey);
        conn.send(json!(["OK", id, true, ""]).to_string());
    }

    /// Housekeeping; call it every second or so. Cheap work runs every call, the rest every minute.
    pub fn sweep(&self, now: i64) {
        let Some(store) = &self.store else {
            return;
        };
        store.sweep(now, 0, 0);
        let last = self.last_slow_sweep.load(Ordering::Relaxed);
        if now.unsigned_abs() >= last + 60 {
            self.last_slow_sweep
                .store(now.unsigned_abs(), Ordering::Relaxed);
            store.sweep(
                now,
                i64::try_from(self.cfg.retention_days).unwrap_or(0) * 86_400,
                self.cfg.max_events,
            );
            store.compact(false);
            lock(&self.rates).retain(|_, r| r.1 > now * 1000);
        }
    }

    pub fn compact(&self) {
        if let Some(s) = &self.store {
            s.compact(true);
        }
    }
}
