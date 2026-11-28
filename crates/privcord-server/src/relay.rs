//! The relay: a mailbox for sealed blobs. It never parses, logs or decrypts what it carries; it sees mailbox names, sizes
//! and timing. The rules are the same as the Node relay's (`relay/server.js`), message for message, so the apps already
//! talk to it. The state is split into shards by mailbox name, so many busy connections do not wait on one lock.

use crate::config::RelayConfig;
use crate::store::Store;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

pub const MAX_BLOB: usize = 64 * 1024;
pub const MAX_SIG_TTL: i64 = 120;
pub const MAX_MSG_TTL: i64 = 7 * 24 * 3600;
pub const MAX_PER_MAILBOX: usize = 500;
const MAX_FIELD: usize = 128;
const SHARDS: usize = 32;
const RATE_WINDOW_MS: i64 = 60_000;
/// Outbound bytes waiting for one connection before it is dropped as a peer that never reads.
pub const MAX_BUFFERED: usize = 2 * MAX_PER_MAILBOX * MAX_BLOB;

/// What a connection's writer is told to do.
#[derive(Debug)]
pub enum Out {
    Text(Arc<str>),
    Ping,
    Close(u16, &'static str),
}

/// Where a connection's output goes: its writer task, and how many bytes are waiting there.
#[derive(Clone, Debug)]
pub struct Conn {
    pub id: u64,
    pub tx: tokio::sync::mpsc::UnboundedSender<Out>,
    /// Bytes queued for this connection and not yet written.
    pub pending: Arc<AtomicUsize>,
    /// Bytes queued for every connection together (what the senders are slowed down by).
    pub total: Arc<AtomicUsize>,
    pub limit: usize,
}

impl Conn {
    /// Queues text for this connection. One allocation is shared by everyone who receives the same event.
    pub fn send(&self, text: impl Into<Arc<str>>) {
        let text: Arc<str> = text.into();
        if self.pending.load(Ordering::Relaxed) > self.limit {
            let _ = self.tx.send(Out::Close(1013, "too slow"));
            return;
        }
        self.pending.fetch_add(text.len(), Ordering::Relaxed);
        self.total.fetch_add(text.len(), Ordering::Relaxed);
        let _ = self.tx.send(Out::Text(text));
    }
}

#[derive(Clone, Debug)]
struct Item {
    id: String,
    blob: String,
    ts: i64,
    exp: i64,
    /// Kept in memory to count one sender's share of a mailbox; never written anywhere.
    ip: String,
}

#[derive(Default)]
struct Shard {
    queues: HashMap<String, Vec<Item>>,
    subs: HashMap<String, Vec<Conn>>,
}

#[derive(Default, Debug)]
pub struct Stats {
    pub connections: AtomicU64,
    pub put: AtomicU64,
    pub delivered: AtomicU64,
    pub rejected: AtomicU64,
}

/// One connection's own state (nobody else touches it, so no lock).
#[derive(Default, Debug)]
pub struct Session {
    pub mailboxes: HashSet<String>,
    pub nostr: crate::nostr::Session,
}

/// The limits an administrator can change while the server runs (the settings file gives the starting values).
#[derive(Debug)]
pub struct RelayLimits {
    /// Keep messages for people who are offline.
    pub keep_offline: std::sync::atomic::AtomicBool,
    pub frames_per_minute: AtomicU64,
    pub connections_per_address: AtomicUsize,
    pub mailboxes_per_connection: AtomicUsize,
}

pub struct Relay {
    limits: RelayLimits,
    cfg: RelayConfig,
    shards: Vec<Mutex<Shard>>,
    stats: Stats,
    queued: AtomicUsize,
    bytes: AtomicUsize,
    mailboxes: AtomicUsize,
    rates: Mutex<HashMap<String, (u64, i64)>>,
    conn_counts: Mutex<HashMap<String, usize>>,
    store: Option<Mutex<Store>>,
    out_total: Arc<AtomicUsize>,
    nostr: Option<Arc<crate::nostr::Nostr>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere must not take the relay down with it: the data is still consistent enough to carry on.
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn valid(v: Option<&Value>) -> Option<&str> {
    let s = v?.as_str()?;
    (!s.is_empty() && s.encode_utf16().count() <= MAX_FIELD).then_some(s)
}

fn shard_of(mailbox: &str) -> usize {
    // FNV-1a: cheap, and it spreads names evenly.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in mailbox.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    usize::try_from(h % SHARDS as u64).unwrap_or(0)
}

impl Relay {
    #[allow(clippy::indexing_slicing)] // shard_of is always below the number of shards
    fn shard(&self, mailbox: &str) -> &Mutex<Shard> {
        &self.shards[shard_of(mailbox)]
    }

    /// `store` is `Some` when messages are kept for offline people.
    pub fn new(cfg: RelayConfig, store: Option<Store>, now_ms: i64) -> Self {
        let relay = Self {
            limits: RelayLimits {
                keep_offline: std::sync::atomic::AtomicBool::new(true),
                frames_per_minute: AtomicU64::new(cfg.rate_limit),
                connections_per_address: AtomicUsize::new(cfg.max_conns_per_ip),
                mailboxes_per_connection: AtomicUsize::new(cfg.max_subs_per_conn),
            },
            cfg,
            shards: (0..SHARDS).map(|_| Mutex::new(Shard::default())).collect(),
            stats: Stats::default(),
            queued: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            mailboxes: AtomicUsize::new(0),
            rates: Mutex::new(HashMap::new()),
            conn_counts: Mutex::new(HashMap::new()),
            store: store.map(Mutex::new),
            out_total: Arc::new(AtomicUsize::new(0)),
            nostr: None,
        };
        if let Some(s) = &relay.store {
            let items = lock(s).load(now_ms);
            for (mailbox, id, blob, ts, exp) in items {
                relay.bytes.fetch_add(blob.len(), Ordering::Relaxed);
                relay.queued.fetch_add(1, Ordering::Relaxed);
                let mut shard = lock(relay.shard(&mailbox));
                shard.queues.entry(mailbox).or_default().push(Item {
                    id,
                    blob,
                    ts,
                    exp,
                    ip: String::new(),
                });
            }
            relay.compact(now_ms);
        }
        relay
    }

    /// Records in the offline-message queue that could not be trusted when it was read.
    pub fn queue_corrupt(&self) -> usize {
        self.store.as_ref().map_or(0, |s| lock(s).corrupt_lines())
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    pub fn store_and_forward(&self) -> bool {
        self.store.is_some()
    }

    pub fn limits(&self) -> &RelayLimits {
        &self.limits
    }

    /// Devices connected right now.
    pub fn open_connections(&self) -> usize {
        lock(&self.conn_counts).values().sum()
    }

    /// Removes every message waiting for a recipient (the store is emptied too). How many there were.
    pub fn clear_stored(&self, now: i64) -> usize {
        let mut removed = 0;
        for shard in &self.shards {
            let mut s = lock(shard);
            removed += s.queues.values().map(Vec::len).sum::<usize>();
            s.queues.clear();
        }
        self.queued.store(0, Ordering::Relaxed);
        self.bytes.store(0, Ordering::Relaxed);
        self.mailboxes.store(0, Ordering::Relaxed);
        self.compact(now);
        removed
    }

    pub fn queued(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }

    pub fn mailbox_count(&self) -> usize {
        self.mailboxes.load(Ordering::Relaxed)
    }

    /// A connection arrives from `ip`: false when that address already holds too many.
    pub fn admit(&self, ip: &str) -> bool {
        let mut counts = lock(&self.conn_counts);
        let open = counts.entry(ip.to_owned()).or_insert(0);
        if *open >= self.limits.connections_per_address.load(Ordering::Relaxed) {
            self.stats.rejected.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        *open += 1;
        self.stats.connections.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// A connection ended: forget its subscriptions.
    /// Adds the Nostr protocol to this relay.
    #[must_use]
    pub fn with_nostr(mut self, n: Arc<crate::nostr::Nostr>) -> Self {
        self.nostr = Some(n);
        self
    }

    pub fn nostr(&self) -> Option<&Arc<crate::nostr::Nostr>> {
        self.nostr.as_ref()
    }

    /// Called once when a connection opens.
    pub fn connected(&self, conn: &Conn, session: &mut Session) {
        if let Some(n) = &self.nostr {
            n.challenge(conn, &mut session.nostr);
        }
    }

    pub fn leave(&self, ip: &str, conn: &Conn, session: &mut Session) {
        let conn_id = conn.id;
        if let Some(n) = &self.nostr {
            n.leave(&mut session.nostr, conn);
        }
        {
            let mut counts = lock(&self.conn_counts);
            if let Some(n) = counts.get_mut(ip) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    counts.remove(ip);
                }
            }
        }
        for mailbox in &session.mailboxes {
            let mut shard = lock(self.shard(mailbox));
            if let Some(list) = shard.subs.get_mut(mailbox) {
                list.retain(|c| c.id != conn_id);
                if list.is_empty() {
                    shard.subs.remove(mailbox);
                    self.mailboxes.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }
    }

    fn limited(&self, ip: &str, now: i64) -> bool {
        let mut rates = lock(&self.rates);
        let entry = rates
            .entry(ip.to_owned())
            .or_insert((0, now + RATE_WINDOW_MS));
        if entry.1 <= now {
            *entry = (0, now + RATE_WINDOW_MS);
        }
        entry.0 += 1;
        entry.0 > self.limits.frames_per_minute.load(Ordering::Relaxed)
    }

    /// One frame from a connection.
    pub fn frame(&self, conn: &Conn, session: &mut Session, ip: &str, raw: &str, now: i64) {
        // A JSON array is a Nostr message, an object is ours.
        if let Some(n) = &self.nostr {
            if raw.trim_start().starts_with('[') {
                return n.frame(conn, &mut session.nostr, ip, raw, now);
            }
        }
        if !self.cfg.enabled {
            return conn.send(
                json!({ "op": "err", "reason": "the PrivCord protocol is off on this server" })
                    .to_string(),
            );
        }
        if self.limited(ip, now) {
            self.stats.rejected.fetch_add(1, Ordering::Relaxed);
            return conn.send(json!({ "op": "err", "reason": "rate limited" }).to_string());
        }
        let Ok(m) = serde_json::from_str::<Value>(raw) else {
            return conn.send(json!({ "op": "err", "reason": "bad json" }).to_string());
        };
        let Some(obj) = m.as_object() else {
            return conn.send(json!({ "op": "err", "reason": "bad frame" }).to_string());
        };
        let echo = obj
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| s.encode_utf16().count() <= MAX_FIELD)
            .map(str::to_owned);
        let fail = |reason: &str| {
            self.stats.rejected.fetch_add(1, Ordering::Relaxed);
            let mut o = json!({ "op": "err", "reason": reason });
            if let (Some(id), Some(map)) = (&echo, o.as_object_mut()) {
                map.insert("id".into(), json!(id));
            }
            conn.send(o.to_string());
        };
        match obj.get("op").and_then(Value::as_str) {
            Some("sub") => self.sub(conn, session, obj, now, &fail),
            Some("put") => self.put(conn, ip, obj, now, &fail),
            Some("del") => {
                if let (Some(_), Some(id)) = (valid(obj.get("mailbox")), valid(obj.get("id"))) {
                    // Deleting on request is not supported: anyone who can work a mailbox name out from a contact card could
                    // wipe what waits there, and the apps never ask. Messages leave by expiry. "ok" so older clients carry on.
                    conn.send(json!({ "op": "ok", "id": id }).to_string());
                } else {
                    fail("bad del");
                }
            }
            _ => fail("unknown op"),
        }
    }

    fn sub(
        &self,
        conn: &Conn,
        session: &mut Session,
        obj: &serde_json::Map<String, Value>,
        now: i64,
        fail: &dyn Fn(&str),
    ) {
        let Some(mailbox) = valid(obj.get("mailbox")) else {
            return fail("bad mailbox");
        };
        if session.mailboxes.len() >= self.limits.mailboxes_per_connection.load(Ordering::Relaxed)
            && !session.mailboxes.contains(mailbox)
        {
            return fail("too many subscriptions");
        }
        session.mailboxes.insert(mailbox.to_owned());
        let since = obj
            .get("since")
            .and_then(Value::as_f64)
            .filter(|f| f.is_finite())
            .unwrap_or(0.0);
        let mut replay: Vec<String> = Vec::new();
        {
            let mut shard = lock(self.shard(mailbox));
            let list = shard.subs.entry(mailbox.to_owned()).or_default();
            if list.is_empty() {
                self.mailboxes.fetch_add(1, Ordering::Relaxed);
            }
            if !list.iter().any(|c| c.id == conn.id) {
                list.push(conn.clone());
            }
            if let Some(q) = shard.queues.get(mailbox) {
                for it in q {
                    if it.exp > now && (it.ts as f64) > since {
                        replay.push(json!({ "op": "event", "mailbox": mailbox, "id": it.id, "blob": it.blob, "ts": it.ts }).to_string());
                    }
                }
            }
        }
        self.stats
            .delivered
            .fetch_add(replay.len() as u64, Ordering::Relaxed);
        for text in replay {
            conn.send(text);
        }
        conn.send(json!({ "op": "ok" }).to_string());
    }

    fn put(
        &self,
        conn: &Conn,
        ip: &str,
        obj: &serde_json::Map<String, Value>,
        now: i64,
        fail: &dyn Fn(&str),
    ) {
        let Some(mailbox) = valid(obj.get("mailbox")) else {
            return fail("bad mailbox");
        };
        let Some(id) = valid(obj.get("id")) else {
            return fail("bad id");
        };
        let blob = match obj.get("blob").and_then(Value::as_str) {
            Some(b) if !b.is_empty() => b,
            _ => return fail("bad blob"),
        };
        if blob.len() > MAX_BLOB {
            return fail("blob too large");
        }
        let kind = obj.get("kind").and_then(Value::as_str);
        if kind != Some("sig") && kind != Some("msg") {
            return fail("bad kind");
        }
        let ttl = match obj.get("ttl").and_then(Value::as_f64) {
            Some(t) if t.fract() == 0.0 && t > 0.0 && t < 1e12 => t as i64,
            _ => return fail("bad ttl"),
        };
        if ttl
            > if kind == Some("sig") {
                MAX_SIG_TTL
            } else {
                MAX_MSG_TTL
            }
        {
            return fail("ttl too large");
        }
        let mut stored_event_ts = now;
        if kind == Some("msg") {
            let keeping = self.limits.keep_offline.load(Ordering::Relaxed);
            let Some(store) = self.store.as_ref().filter(|_| keeping) else {
                return fail("store-and-forward disabled");
            };
            let mut shard = lock(self.shard(mailbox));
            let q = shard.queues.entry(mailbox.to_owned()).or_default();
            let exists = q.iter().any(|i| i.id == id);
            if !exists {
                if q.len() >= MAX_PER_MAILBOX {
                    return self.drop_empty(&mut shard, mailbox, || fail("mailbox full"));
                }
                if !q.is_empty()
                    && q.iter().filter(|i| i.ip == ip).count() >= self.cfg.max_per_mailbox_per_ip
                {
                    return fail("mailbox quota for this address");
                }
                if self.queued.load(Ordering::Relaxed) >= self.cfg.max_queued
                    || self.bytes.load(Ordering::Relaxed) + blob.len() > self.cfg.max_queued_bytes
                {
                    return self.drop_empty(&mut shard, mailbox, || fail("relay full"));
                }
                let exp = now + ttl * 1000;
                if !lock(store).append_put(mailbox, id, blob, now, exp) {
                    return self.drop_empty(&mut shard, mailbox, || fail("storage error"));
                }
                if let Some(q) = shard.queues.get_mut(mailbox) {
                    q.push(Item {
                        id: id.to_owned(),
                        blob: blob.to_owned(),
                        ts: now,
                        exp,
                        ip: ip.to_owned(),
                    });
                }
                self.queued.fetch_add(1, Ordering::Relaxed);
                self.bytes.fetch_add(blob.len(), Ordering::Relaxed);
            }
            stored_event_ts = now;
        }
        self.stats.put.fetch_add(1, Ordering::Relaxed);
        // Everyone listening gets it (a copy of each connection's handle, so no lock is held while sending).
        let listeners: Vec<Conn> = lock(self.shard(mailbox))
            .subs
            .get(mailbox)
            .cloned()
            .unwrap_or_default();
        if !listeners.is_empty() {
            let event: Arc<str> = json!({ "op": "event", "mailbox": mailbox, "id": id, "blob": blob, "ts": stored_event_ts }).to_string().into();
            self.stats
                .delivered
                .fetch_add(listeners.len() as u64, Ordering::Relaxed);
            for l in &listeners {
                l.send(event.clone());
            }
        }
        conn.send(json!({ "op": "ok", "id": id }).to_string());
    }

    /// A mailbox the failed put created for nothing is not left behind; then the failure is reported.
    fn drop_empty(&self, shard: &mut Shard, mailbox: &str, report: impl FnOnce()) {
        if shard.queues.get(mailbox).is_some_and(Vec::is_empty) {
            shard.queues.remove(mailbox);
        }
        report();
    }

    /// Drops what has expired and forgets old rate counters.
    pub fn sweep(&self, now: i64) {
        if let Some(n) = &self.nostr {
            n.sweep(now / 1000);
        }
        if let Some(store) = &self.store {
            lock(store).sync();
        }
        for shard in &self.shards {
            let mut s = lock(shard);
            let mut freed = 0usize;
            let mut gone = 0usize;
            s.queues.retain(|_, q| {
                q.retain(|it| {
                    let keep = it.exp > now;
                    if !keep {
                        freed += it.blob.len();
                        gone += 1;
                    }
                    keep
                });
                !q.is_empty()
            });
            self.bytes.fetch_sub(
                freed.min(self.bytes.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
            self.queued.fetch_sub(
                gone.min(self.queued.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
        }
        lock(&self.rates).retain(|_, (_, reset)| *reset > now);
    }

    /// Rewrites the log from what is held (drops what expired).
    pub fn compact(&self, now: i64) {
        if let Some(n) = &self.nostr {
            n.compact();
        }
        let Some(store) = &self.store else {
            return;
        };
        let mut live: Vec<(String, String, String, i64, i64)> = Vec::new();
        for shard in &self.shards {
            let s = lock(shard);
            for (mailbox, q) in &s.queues {
                for it in q.iter().filter(|i| i.exp > now) {
                    live.push((
                        mailbox.clone(),
                        it.id.clone(),
                        it.blob.clone(),
                        it.ts,
                        it.exp,
                    ));
                }
            }
        }
        lock(store).rewrite(&live);
    }

    /// The numbers `/healthz` reports (counters only; nothing about people).
    /// Bytes waiting to be written to connections, in all, and the handle each connection adds to.
    pub fn out_total(&self) -> &Arc<AtomicUsize> {
        &self.out_total
    }

    pub fn stall_ms(&self) -> u64 {
        self.cfg.stall_secs.saturating_mul(1000)
    }

    pub fn out_limit(&self) -> (usize, usize) {
        (self.cfg.max_buffered_per_conn, self.cfg.backpressure_bytes)
    }

    pub fn health(&self) -> Value {
        let mut v = json!({
            "ok": true,
            "queued": self.queued(),
            "mailboxes": self.mailbox_count(),
            "connections": self.stats.connections.load(Ordering::Relaxed),
            "open_connections": self.open_connections(),
            "put": self.stats.put.load(Ordering::Relaxed),
            "delivered": self.stats.delivered.load(Ordering::Relaxed),
            "rejected": self.stats.rejected.load(Ordering::Relaxed),
        });
        if let (Some(n), Some(o)) = (&self.nostr, v.as_object_mut()) {
            o.insert("nostr".into(), n.counters());
        }
        v
    }
}

impl std::fmt::Debug for Relay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Relay")
    }
}
