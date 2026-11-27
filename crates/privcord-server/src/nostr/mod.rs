//! Nostr relay protocol (NIP-01 and friends) on the same port as the PrivCord protocol. A client that sends a JSON array is
//! talking Nostr; one that sends an object is talking PrivCord.
//!
//! Supported: NIP-01 (events, REQ/CLOSE, replaceable, ephemeral and addressable kinds), NIP-09 (deletion), NIP-11 (relay
//! information), NIP-13 (proof of work), NIP-40 (expiration), NIP-42 (authentication), NIP-45 (COUNT).

pub mod event;
pub mod filter;
pub mod nip98;
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
    /// Who the connection has proved to be (shared with its session, which may authenticate later).
    auth: Arc<RwLock<Option<Id>>>,
}

/// What one connection has open.
#[derive(Default, Debug)]
pub struct Session {
    subs: HashMap<String, Vec<SubKey>>,
    /// NIP-77 syncs in progress on this connection (the server's side of each).
    negs: HashMap<String, negentropy::Negentropy<'static, negentropy::NegentropyStorageVector>>,
    challenge: Option<String>,
    authed: Arc<RwLock<Option<Id>>>,
    /// Subscriptions opened in the current minute: (count, minute ends at ms).
    req_window: (u64, i64),
}

impl Session {
    fn auth(&self) -> Option<Id> {
        *self.authed.read().unwrap_or_else(PoisonError::into_inner)
    }
}

/// An event that is waiting for the disk before it is confirmed and handed to subscribers.
struct Pending {
    conn: Conn,
    event: Arc<Event>,
    ok: Arc<str>,
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
        let keys = |v: &[String]| {
            v.iter()
                .filter_map(|s| unhex::<32>(&s.trim().to_lowercase()))
                .collect()
        };
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

/// The Nostr limits an administrator can change while the server runs.
#[derive(Debug)]
pub struct NostrLimits {
    pub events_per_minute: AtomicU64,
    pub requests_per_minute: AtomicU64,
    /// Days to keep events (0: for ever).
    pub keep_days: AtomicU64,
}

pub struct Nostr {
    limits: NostrLimits,
    pub cfg: NostrConfig,
    /// Name, description and icon set through the management API (they win over the settings file).
    info_override: RwLock<(Option<String>, Option<String>, Option<String>)>,
    store: Option<Store>,
    policy: RwLock<Policy>,
    subs: Vec<Mutex<HashMap<SubKey, Vec<SubRef>>>>,
    rates: Mutex<HashMap<String, (u64, i64)>>,
    stats: Stats,
    last_slow_sweep: AtomicU64,
    /// The relay-wide second in which events are being counted, and how many came in it.
    global_rate: Mutex<(i64, u64)>,
    waiting: Mutex<Vec<Pending>>,
    wake: tokio::sync::Notify,
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

/// A relay address reduced to what identifies it: host and port (scheme only decides the default port), lower case.
fn relay_key(url: &str) -> Option<(String, u16)> {
    let u = url.trim().to_lowercase();
    let (scheme, rest) = u.split_once("://")?;
    let default = match scheme {
        "wss" | "https" => 443,
        "ws" | "http" => 80,
        _ => return None,
    };
    let host_port = rest.split(['/', '?', '#']).next()?;
    if let Some(v6) = host_port.strip_prefix('[') {
        let (host, after) = v6.split_once(']')?;
        let port = after
            .strip_prefix(':')
            .map_or(Some(default), |p| p.parse().ok())?;
        return Some((host.to_owned(), port));
    }
    match host_port.rsplit_once(':') {
        Some((h, p)) => Some((h.to_owned(), p.parse().ok()?)),
        None => Some((host_port.to_owned(), default)),
    }
}

/// NIP-42: the `relay` tag must name this relay. Otherwise another relay could hand a client our challenge, get it signed
/// for itself, and replay it here as that client.
fn relay_matches(tag: Option<&str>, configured: &str) -> bool {
    match (tag.and_then(relay_key), relay_key(configured)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// The largest NEG-MSG the server sends back (bytes, before hex): it must fit in a WebSocket message with room to spare.
const NEG_FRAME_LIMIT: u64 = 60_000;

fn unhex_vec(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn frame_event(sub: &str, e: &Event) -> Arc<str> {
    format!("[\"EVENT\",{},{}]", Value::from(sub), e.json).into()
}

impl Nostr {
    pub fn new(cfg: NostrConfig, data_dir: &std::path::Path) -> Result<Self, String> {
        Self::with_codec(cfg, data_dir, Arc::new(crate::record::Codec::plain()))
    }

    /// With the settings' way of writing the log (a check on every record, and encryption when a key is set).
    pub fn with_codec(
        cfg: NostrConfig,
        data_dir: &std::path::Path,
        codec: Arc<crate::record::Codec>,
    ) -> Result<Self, String> {
        let store = cfg
            .store_events
            .then(|| Store::open_with(cfg.persist.then_some(data_dir), cfg.max_store_bytes, codec))
            .transpose()?;
        let policy = Policy::from_config(&cfg);
        Ok(Self {
            info_override: RwLock::new((None, None, None)),
            limits: NostrLimits {
                events_per_minute: AtomicU64::new(cfg.events_per_minute_per_ip),
                requests_per_minute: AtomicU64::new(cfg.reqs_per_minute_per_conn),
                keep_days: AtomicU64::new(cfg.retention_days),
            },
            cfg,
            store,
            policy: RwLock::new(policy),
            subs: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
            rates: Mutex::new(HashMap::new()),
            stats: Stats::default(),
            last_slow_sweep: AtomicU64::new(0),
            global_rate: Mutex::new((0, 0)),
            waiting: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),
        })
    }

    #[allow(clippy::indexing_slicing)] // shard_of is below SHARDS
    fn shard(&self, k: &SubKey) -> &Mutex<HashMap<SubKey, Vec<SubRef>>> {
        &self.subs[shard_of(k)]
    }

    pub fn limits(&self) -> &NostrLimits {
        &self.limits
    }

    /// Removes every stored event; how many there were.
    pub fn clear_events(&self) -> usize {
        self.store.as_ref().map_or(0, Store::clear)
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

    /// Moderation: removes the stored event with this id (hex) and refuses it from now on. Whether it was there.
    pub fn ban_event(&self, id_hex: &str) -> Option<bool> {
        let id = unhex::<32>(id_hex)?;
        Some(self.store.as_ref().is_some_and(|s| s.ban_event(&id)))
    }

    pub fn store(&self) -> Option<&Store> {
        self.store.as_ref()
    }

    /// Takes a ban on an event back. `None` for something that is not an event id; otherwise whether it was banned.
    pub fn unban_event(&self, id_hex: &str) -> Option<bool> {
        let id = unhex::<32>(id_hex)?;
        Some(self.store.as_ref().is_some_and(|s| s.unban_event(&id)))
    }

    /// The events banned by an administrator, as hex.
    pub fn banned_events(&self) -> Vec<String> {
        self.store
            .as_ref()
            .map(|s| s.banned_events().iter().map(|i| hex(i)).collect())
            .unwrap_or_default()
    }

    /// Public keys (hex) allowed to use the management API: `admin_pubkey` and `admin_pubkeys`.
    pub fn admin_keys(&self) -> Vec<Id> {
        std::iter::once(&self.cfg.admin_pubkey)
            .chain(self.cfg.admin_pubkeys.iter())
            .filter_map(|k| unhex::<32>(k.trim().to_lowercase().as_str()))
            .collect()
    }

    /// Changes what the information document says about this relay (kept until the next change).
    pub fn set_info(
        &self,
        name: Option<String>,
        description: Option<String>,
        icon: Option<String>,
    ) {
        *self
            .info_override
            .write()
            .unwrap_or_else(PoisonError::into_inner) = (name, description, icon);
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
        let over = self
            .info_override
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut doc = json!({
            "name": over.0.as_deref().unwrap_or(&c.name),
            "description": over.1.as_deref().unwrap_or(&c.description),
            "contact": contact,
            "pubkey": c.admin_pubkey,
            "supported_nips": [1, 2, 9, 11, 12, 13, 15, 16, 20, 22, 33, 40, 42, 45, 50, 62, 70, 77, 86, 98],
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
        let icon = over.2.clone().unwrap_or_else(|| c.icon.clone());
        if let (false, Some(o)) = (icon.is_empty(), doc.as_object_mut()) {
            o.insert("icon".into(), Value::from(icon));
        }
        doc
    }

    /// A cap on events per second for the whole relay, whoever sends them.
    fn global_limited(&self, now_ms: i64) -> bool {
        let cap = self.cfg.events_per_second_global;
        if cap == 0 {
            return false;
        }
        let mut g = lock(&self.global_rate);
        let second = now_ms / 1000;
        if g.0 != second {
            *g = (second, 0);
        }
        g.1 += 1;
        g.1 > cap
    }

    /// Whether `who` may be sent `e`: with `auth_dms`, direct messages go only to their sender and recipients.
    fn visible(&self, e: &Event, who: Option<Id>) -> bool {
        if !self.cfg.auth_dms || !(e.kind == 4 || e.kind == 1059) {
            return true;
        }
        let Some(me) = who else {
            return false;
        };
        let me_hex = hex(&me);
        e.pubkey == me
            || e.tags
                .iter()
                .any(|t| t.first().map(String::as_str) == Some("p") && t.get(1) == Some(&me_hex))
    }

    fn rate_limited(&self, ip: &str, now_ms: i64) -> bool {
        let limit = self.limits.events_per_minute.load(Ordering::Relaxed);
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

    /// NIP-42: a relay that wants authentication offers its challenge as soon as a client connects.
    pub fn challenge(&self, conn: &Conn, sess: &mut Session) {
        if (self.cfg.auth_required || self.cfg.auth_dms || !self.cfg.relay_url.is_empty())
            && sess.challenge.is_none()
        {
            let mut b = [0u8; 16];
            let _ = getrandom::fill(&mut b);
            let ch = hex(&b);
            conn.send(json!(["AUTH", ch]).to_string());
            sess.challenge = Some(ch);
        }
    }

    /// Handles one Nostr frame from a connection.
    pub fn frame(&self, conn: &Conn, sess: &mut Session, ip: &str, raw: &str, now_ms: i64) {
        let notice = |m: &str| conn.send(json!(["NOTICE", m]).to_string());
        self.challenge(conn, sess);
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
            Some("NEG-OPEN") => self.on_neg_open(conn, sess, &msg, now),
            Some("NEG-MSG") => self.on_neg_msg(conn, sess, &msg),
            Some("NEG-CLOSE") => {
                if let Some(id) = msg.get(1).and_then(Value::as_str) {
                    sess.negs.remove(id);
                }
            }
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
        if self.global_limited(now_ms) {
            return reject(
                "rate-limited: the relay is receiving too many events, try again in a moment",
            );
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
        if c.auth_required && sess.auth().is_none() {
            return reject("auth-required: authenticate first");
        }
        // NIP-70: a protected event (a lone "-" tag) is only taken from its own author, signed in (NIP-42).
        if e.tags
            .iter()
            .any(|t| t.len() == 1 && t.first().is_some_and(|n| n == "-"))
            && sess.auth() != Some(e.pubkey)
        {
            return reject(
                "auth-required: this event is protected, only its author may publish it (authenticate first)",
            );
        }
        // NIP-62: a request to vanish names this relay, or all relays.
        if e.kind == 62
            && !e.tags.iter().any(|t| {
                t.first().is_some_and(|n| n == "relay")
                    && t.get(1).is_some_and(|v| {
                        v == "ALL_RELAYS" || relay_matches(Some(v.as_str()), &c.relay_url)
                    })
            })
        {
            return reject("invalid: this request to vanish is not addressed to this relay");
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
                        if self.cfg.durable && self.cfg.persist {
                            // Confirmed, and shown to subscribers, only once it is on disk (see `run_committer`).
                            lock(&self.waiting).push(Pending {
                                conn: conn.clone(),
                                event: e,
                                ok: json!(["OK", id, true, ""]).to_string().into(),
                            });
                            self.wake.notify_one();
                            return;
                        }
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
                    Put::Full => return reject("blocked: this relay's storage is full"),
                    Put::Failed => return reject("error: could not write the event to disk"),
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
        let mut seen: HashSet<(u64, Arc<str>)> = HashSet::new();
        for key in Self::keys_for(e) {
            let candidates: Vec<SubRef> = lock(self.shard(&key))
                .get(&key)
                .cloned()
                .unwrap_or_default();
            for s in candidates {
                if !seen.insert((s.conn.id, s.id.clone())) {
                    continue;
                }
                if any_match(&s.filters, e)
                    && self.visible(e, *s.auth.read().unwrap_or_else(PoisonError::into_inner))
                {
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

    /// A cap on how fast one connection may ask (REQ and COUNT both cost the store work).
    fn req_allowed(&self, sess: &mut Session, now: i64) -> bool {
        let cap = self.limits.requests_per_minute.load(Ordering::Relaxed);
        if cap == 0 {
            return true;
        }
        let now_ms = now * 1000;
        if sess.req_window.1 <= now_ms {
            sess.req_window = (0, now_ms + 60_000);
        }
        sess.req_window.0 += 1;
        sess.req_window.0 <= cap
    }

    fn parse_filters(&self, msg: &[Value], from: usize) -> Result<Vec<Filter>, String> {
        let raw = msg.get(from..).unwrap_or(&[]);
        if raw.is_empty() {
            return Err("invalid: no filter".into());
        }
        if raw.len() > self.cfg.max_filters {
            return Err("invalid: too many filters".into());
        }
        let filters: Vec<Filter> = raw
            .iter()
            .map(|v| Filter::parse(v).map_err(str::to_owned))
            .collect::<Result<_, _>>()?;
        if filters.iter().map(Filter::value_count).sum::<usize>() > self.cfg.max_filter_values {
            return Err("invalid: too many values in the filters".into());
        }
        Ok(filters)
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
        if self.cfg.auth_required && sess.auth().is_none() {
            return closed("auth-required: authenticate first");
        }
        if !sess.subs.contains_key(sub) && sess.subs.len() >= self.cfg.max_subs_per_conn {
            return closed("error: too many subscriptions");
        }
        if !self.req_allowed(sess, now) {
            return closed("rate-limited: too many subscriptions requested, slow down");
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
                        auth: sess.authed.clone(),
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
                    if self.visible(&e, sess.auth()) && sent.insert(e.id) {
                        conn.send(frame_event(sub, &e));
                    }
                }
            }
        }
        conn.send(json!(["EOSE", sub]).to_string());
    }

    /// NIP-77: the client opens a sync with a filter and its first message; the answer says which events each side lacks, in
    /// as many rounds as it takes (`NEG-MSG` both ways), and the client then asks for what it needs with a plain `REQ`.
    fn on_neg_open(&self, conn: &Conn, sess: &mut Session, msg: &[Value], now: i64) {
        let Some(sub) = msg.get(1).and_then(Value::as_str) else {
            return conn
                .send(json!(["NOTICE", "error: NEG-OPEN needs a subscription id"]).to_string());
        };
        let err = |why: &str| conn.send(json!(["NEG-ERR", sub, why]).to_string());
        if sub.is_empty() || sub.len() > self.cfg.max_subid_len {
            return err("invalid: bad subscription id");
        }
        if self.cfg.auth_required && sess.auth().is_none() {
            return err("auth-required: authenticate first");
        }
        if !sess.negs.contains_key(sub) && sess.negs.len() >= self.cfg.max_negentropy_sessions {
            return err("blocked: too many sync sessions open on this connection");
        }
        if !self.req_allowed(sess, now) {
            return err("rate-limited: too many requests, slow down");
        }
        let Some(store) = &self.store else {
            return err("blocked: this relay keeps no events");
        };
        let Some(mut filter) = msg.get(2).and_then(|v| Filter::parse(v).ok()) else {
            return err("invalid: the filter is not valid");
        };
        if filter.value_count() > self.cfg.max_filter_values {
            return err("invalid: too many values in the filter");
        }
        let Some(first) = msg.get(3).and_then(Value::as_str).and_then(unhex_vec) else {
            return err("invalid: the first message must be hex");
        };
        filter.limit = None;
        let max = self.cfg.max_negentropy_items;
        let found = store.query(&filter, now, max.saturating_add(1));
        if found.len() > max {
            return err("blocked: too many results, narrow the filter");
        }
        let mut set = negentropy::NegentropyStorageVector::with_capacity(found.len());
        for e in found.iter().filter(|e| self.visible(e, sess.auth())) {
            let _ = set.insert(
                u64::try_from(e.created_at).unwrap_or(0),
                negentropy::Id::from_byte_array(e.id),
            );
        }
        let ready = set
            .seal()
            .ok()
            .and_then(|()| negentropy::Negentropy::owned(set, NEG_FRAME_LIMIT).ok());
        let Some(mut ng) = ready else {
            return err("error: could not start the sync");
        };
        match ng.reconcile(&first) {
            Ok(reply) => {
                sess.negs.insert(sub.to_owned(), ng);
                conn.send(json!(["NEG-MSG", sub, hex(&reply)]).to_string());
            }
            Err(e) => err(&format!("error: {e}")),
        }
    }

    fn on_neg_msg(&self, conn: &Conn, sess: &mut Session, msg: &[Value]) {
        let Some(sub) = msg.get(1).and_then(Value::as_str) else {
            return conn
                .send(json!(["NOTICE", "error: NEG-MSG needs a subscription id"]).to_string());
        };
        let err = |why: &str| conn.send(json!(["NEG-ERR", sub, why]).to_string());
        let Some(bytes) = msg.get(2).and_then(Value::as_str).and_then(unhex_vec) else {
            return err("invalid: the message must be hex");
        };
        let Some(ng) = sess.negs.get_mut(sub) else {
            return err("closed: there is no such sync session (it was closed, or never opened)");
        };
        match ng.reconcile(&bytes) {
            Ok(reply) => conn.send(json!(["NEG-MSG", sub, hex(&reply)]).to_string()),
            Err(e) => {
                sess.negs.remove(sub);
                err(&format!("error: {e}"));
            }
        }
    }

    fn on_count(&self, conn: &Conn, sess: &mut Session, msg: &[Value], now: i64) {
        let Some(sub) = msg.get(1).and_then(Value::as_str) else {
            return conn.send(json!(["NOTICE", "error: COUNT needs an id"]).to_string());
        };
        if self.cfg.auth_required && sess.auth().is_none() {
            return conn
                .send(json!(["CLOSED", sub, "auth-required: authenticate first"]).to_string());
        }
        if !self.req_allowed(sess, now) {
            return conn.send(json!(["CLOSED", sub, "rate-limited: slow down"]).to_string());
        }
        match self.parse_filters(msg, 2) {
            Ok(f) => {
                let n = self.store.as_ref().map_or(0, |s| {
                    if self.cfg.auth_dms {
                        // Count only what this connection may see.
                        let mut seen = HashSet::new();
                        for g in &f {
                            let g = Filter {
                                limit: None,
                                ..g.clone()
                            };
                            for e in s.query(&g, now, 100_000) {
                                if self.visible(&e, sess.auth()) {
                                    seen.insert(e.id);
                                }
                            }
                        }
                        seen.len()
                    } else {
                        s.count(&f, now)
                    }
                });
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
            || !relay_matches(e.tag("relay"), &self.cfg.relay_url)
        {
            return fail("invalid: not a valid authentication for this connection");
        }
        *sess.authed.write().unwrap_or_else(PoisonError::into_inner) = Some(e.pubkey);
        conn.send(json!(["OK", id, true, ""]).to_string());
    }

    /// Housekeeping; call it every second or so. Cheap work runs every call, the rest every minute.
    pub fn sweep(&self, now: i64) {
        let Some(store) = &self.store else {
            return;
        };
        store.sweep(now, 0, 0);
        store.sync();
        let last = self.last_slow_sweep.load(Ordering::Relaxed);
        if now.unsigned_abs() >= last + 60 {
            self.last_slow_sweep
                .store(now.unsigned_abs(), Ordering::Relaxed);
            store.sweep(
                now,
                i64::try_from(self.limits.keep_days.load(Ordering::Relaxed)).unwrap_or(0) * 86_400,
                self.cfg.max_events,
            );
            store.compact(false);
            lock(&self.rates).retain(|_, r| r.1 > now * 1000);
        }
    }

    /// Group commit: waits for events to pile up, writes them to disk in one go, then confirms them all and hands them to
    /// subscribers. While the disk is busy more pile up, so the cost of a sync is shared by everything that arrived.
    pub async fn run_committer(self: Arc<Self>, mut stop: tokio::sync::watch::Receiver<bool>) {
        loop {
            let stopping = *stop.borrow();
            let batch: Vec<Pending> = std::mem::take(&mut *lock(&self.waiting));
            if batch.is_empty() {
                if stopping {
                    return;
                }
                tokio::select! {
                    () = self.wake.notified() => {}
                    _ = stop.changed() => {}
                }
                continue;
            }
            let me = self.clone();
            // False when the disk refused the sync: nothing in this batch is confirmed as safe.
            let synced =
                tokio::task::spawn_blocking(move || me.store.as_ref().is_none_or(|s| s.sync()))
                    .await
                    .unwrap_or(false);
            for p in batch {
                if synced {
                    self.fan_out(&p.event);
                    p.conn.send(p.ok);
                } else {
                    let id = hex(&p.event.id);
                    p.conn.send(
                        json!(["OK", id, false, "error: could not write the event to disk"])
                            .to_string(),
                    );
                }
            }
        }
    }

    pub fn compact(&self) {
        if let Some(s) = &self.store {
            s.compact(true);
        }
    }
}
