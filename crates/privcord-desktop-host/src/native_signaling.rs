//! The relay sockets, owned by the desktop shell in Rust instead of by the web page (docs/design/syncengine.md §10).
//!
//! With the sockets here, the window can be closed completely when DuoCord sits in the tray: messages, sync entries
//! and notifications keep arriving with only the Rust engine running. The relay and Nostr protocol logic is the
//! engine's own (`privcord_engine::signaling`), the same the phone uses; this file only carries out what it asks for
//! on real WebSockets: one connection per relay, whatever the number of mailboxes (`SignalingMux`).
//!
//! In private mode every connection goes through Tor's SOCKS proxy, and host names are resolved by Tor (never here).
//!
//! ponytail: one worker thread, blocking connects with a short timeout; per-connection threads if a slow relay ever
//! holds the others up noticeably.

use privcord_crypto::rng::{Entropy, OsEntropy};
use privcord_domain::{
    blob::SealedBlob,
    network::{NetworkConfig, SignalingConfig},
    signaling::{PutKind, SignalingStatus},
};
use privcord_engine::signaling::{
    mux::{MuxConfig, MuxEvent, MuxOut, SignalingMux},
    Action, PutRequest,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tungstenite::{client::IntoClientRequest, stream::MaybeTlsStream, Message, WebSocket};

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;
/// Called with (mailbox, encoded sealed blob) for every blob that arrives.
pub type BlobSink = Arc<dyn Fn(String, String) + Send + Sync>;
/// Called when the relays as a whole go online or offline.
pub type StatusSink = Arc<dyn Fn(bool) + Send + Sync>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// How long one read waits on an idle socket.
const READ_WAIT: Duration = Duration::from_millis(2);
const TICK_EVERY: Duration = Duration::from_millis(500);
/// A put waits this long for a relay to be online, as the page did.
const PUT_WAIT: Duration = Duration::from_secs(20);
const IDLE_SLEEP: Duration = Duration::from_millis(20);
/// A socket that has been silent this long is asked a question (a WebSocket ping, which every relay answers)...
const PING_EVERY_MS: i64 = 20_000;
/// ...and is replaced if nothing at all has come from it for this long. A laptop that slept or changed network leaves
/// sockets that look open and carry nothing; relays do not keep the live signals sent meanwhile.
const DEAD_AFTER_MS: i64 = 55_000;
/// The worker ticks twice a second: a gap this long between two ticks means the machine was asleep.
const RESUME_GAP_MS: i64 = 10_000;

/// When a socket is pinged and when it is given up on (milliseconds; shortened by the tests).
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    ping_every_ms: i64,
    dead_after_ms: i64,
    resume_gap_ms: i64,
}

impl Timing {
    pub fn new(ping_every_ms: i64, dead_after_ms: i64, resume_gap_ms: i64) -> Self {
        Self {
            ping_every_ms,
            dead_after_ms,
            resume_gap_ms,
        }
    }
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ping_every_ms: PING_EVERY_MS,
            dead_after_ms: DEAD_AFTER_MS,
            resume_gap_ms: RESUME_GAP_MS,
        }
    }
}

enum Cmd {
    Configure {
        config: NetworkConfig,
        socks: Option<String>,
    },
    Subscribe(String),
    Unsubscribe(String),
    Put {
        mailbox: String,
        blob: String,
        kind: PutKind,
        ttl_sec: u32,
    },
    Stop,
}

/// The relay sockets of one install, on their own thread.
pub struct NativeSignaling {
    tx: Sender<Cmd>,
    online: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl NativeSignaling {
    pub fn start(sink: BlobSink, status: StatusSink) -> Self {
        Self::start_with(sink, status, Timing::default())
    }

    pub fn start_with(sink: BlobSink, status: StatusSink, timing: Timing) -> Self {
        let (tx, rx) = channel();
        let online = Arc::new(AtomicBool::new(false));
        let flag = online.clone();
        let worker = std::thread::Builder::new()
            .name("relay-sockets".into())
            .spawn(move || Worker::new(sink, status, flag, timing).run(&rx))
            .ok();
        Self { tx, online, worker }
    }

    /// New network settings (the page's saved JSON) and, in private mode, Tor's SOCKS proxy. Every mailbox's sockets
    /// are opened again with them.
    pub fn configure(&self, network_json: &str, socks: Option<String>) {
        let config = NetworkConfig::parse(Some(network_json));
        let _ = self.tx.send(Cmd::Configure { config, socks });
    }

    pub fn subscribe(&self, mailbox: &str) -> bool {
        self.tx.send(Cmd::Subscribe(mailbox.to_owned())).is_ok()
    }

    pub fn unsubscribe(&self, mailbox: &str) {
        let _ = self.tx.send(Cmd::Unsubscribe(mailbox.to_owned()));
    }

    /// Queues a blob to publish. `false` when no relay is online, so the engine can try another way (mutual contacts).
    pub fn put(&self, mailbox: &str, blob: &str, kind: PutKind, ttl_sec: u32) -> bool {
        self.online()
            && self
                .tx
                .send(Cmd::Put {
                    mailbox: mailbox.to_owned(),
                    blob: blob.to_owned(),
                    kind,
                    ttl_sec,
                })
                .is_ok()
    }

    pub fn online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for NativeSignaling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeSignaling")
            .field("online", &self.online())
            .finish_non_exhaustive()
    }
}

impl Drop for NativeSignaling {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

struct PendingPut {
    mailbox: String,
    blob: SealedBlob,
    kind: PutKind,
    ttl_sec: u32,
    until: Instant,
}

struct Worker {
    /// A mailbox nobody writes to, kept subscribed so the relay sockets are open (and `online` is true) even before any
    /// conversation or link has a mailbox of its own. Without it a fresh install, with no contacts, reports "offline"
    /// and cannot start linking to another device (which has to find a relay online before it subscribes).
    idle: String,
    config: NetworkConfig,
    socks: Option<String>,
    /// Every mailbox listened on, over one connection per relay (the engine's `SignalingMux` decides what to send where).
    mux: SignalingMux,
    mailboxes: BTreeSet<String>,
    sockets: BTreeMap<String, Ws>,
    /// Puts waiting for a relay to be online, and puts a relay refused (tried again until their time is up).
    puts: Vec<PendingPut>,
    in_flight: BTreeMap<String, PendingPut>,
    sink: BlobSink,
    status: StatusSink,
    online: Arc<AtomicBool>,
    began: Instant,
    last_tick: Instant,
    /// The wall-clock time of the last tick, to notice that the machine was asleep.
    last_tick_wall: i64,
    timing: Timing,
    put_seq: u64,
}

fn mux_for(config: &NetworkConfig, timing: Timing) -> SignalingMux {
    let mut cfg = MuxConfig::default();
    for s in &config.signaling {
        match s {
            SignalingConfig::Relay { url } => cfg.relays.push(url.clone()),
            SignalingConfig::Nostr { relays } => cfg.nostr.extend(relays.iter().cloned()),
        }
    }
    SignalingMux::new(&cfg).with_timing(timing.ping_every_ms, timing.dead_after_ms)
}

impl Worker {
    fn new(sink: BlobSink, status: StatusSink, online: Arc<AtomicBool>, timing: Timing) -> Self {
        let mut raw = [0u8; 16];
        let _ = OsEntropy.fill(&mut raw);
        let config = NetworkConfig {
            signaling: Vec::new(),
            ..NetworkConfig::default()
        };
        Self {
            idle: raw.iter().map(|b| format!("{b:02x}")).collect(),
            mux: mux_for(&config, timing),
            config,
            socks: None,
            mailboxes: BTreeSet::new(),
            sockets: BTreeMap::new(),
            puts: Vec::new(),
            in_flight: BTreeMap::new(),
            sink,
            status,
            online,
            began: Instant::now(),
            last_tick: Instant::now(),
            last_tick_wall: 0,
            timing,
            put_seq: 0,
        }
    }

    fn now(&self) -> i64 {
        // Wall-clock milliseconds: the Nostr client stamps events with them.
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or_else(|_| i64::try_from(self.began.elapsed().as_millis()).unwrap_or(0))
    }

    fn run(mut self, rx: &Receiver<Cmd>) {
        loop {
            let mut busy = false;
            loop {
                match rx.try_recv() {
                    Ok(Cmd::Stop) => {
                        self.stop_all();
                        return;
                    }
                    Ok(cmd) => {
                        busy = true;
                        self.handle(cmd);
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.stop_all();
                        return;
                    }
                }
            }
            busy |= self.read_all();
            if self.last_tick.elapsed() >= TICK_EVERY {
                self.last_tick = Instant::now();
                self.tick_all();
            }
            busy |= self.flush_puts();
            self.report();
            if !busy {
                std::thread::sleep(IDLE_SLEEP);
            }
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Configure { config, socks } => {
                // New settings: close everything and listen again, on the new relays, for every mailbox we had.
                self.stop_all();
                self.config = config;
                self.socks = socks;
                self.mux = mux_for(&self.config, self.timing);
                let mut mailboxes: Vec<String> = self.mailboxes.iter().cloned().collect();
                if !mailboxes.contains(&self.idle) {
                    mailboxes.push(self.idle.clone());
                }
                for m in mailboxes {
                    self.subscribe(&m);
                }
            }
            Cmd::Subscribe(m) => {
                if !self.mailboxes.contains(&m) {
                    self.subscribe(&m);
                }
            }
            Cmd::Unsubscribe(m) => {
                if m == self.idle {
                    return;
                }
                if self.mailboxes.remove(&m) {
                    let out = self.mux.unsubscribe(&m);
                    self.apply(out);
                }
            }
            Cmd::Put {
                mailbox,
                blob,
                kind,
                ttl_sec,
            } => {
                if let Some(blob) = SealedBlob::decode(&blob) {
                    self.puts.push(PendingPut {
                        mailbox,
                        blob,
                        kind,
                        ttl_sec,
                        until: Instant::now() + PUT_WAIT,
                    });
                }
            }
            Cmd::Stop => {}
        }
    }

    fn subscribe(&mut self, mailbox: &str) {
        self.mailboxes.insert(mailbox.to_owned());
        let now = self.now();
        let out = self.mux.subscribe(mailbox, now);
        self.apply(out);
    }

    fn stop_all(&mut self) {
        let out = self.mux.stop();
        self.apply(out);
        for (_, mut s) in std::mem::take(&mut self.sockets) {
            let _ = s.close(None);
        }
    }

    /// Carries out what the mux asked for, and passes on what it reported.
    fn apply(&mut self, out: MuxOut) {
        let mut pending = vec![out];
        while let Some(out) = pending.pop() {
            for e in out.events {
                match e {
                    MuxEvent::Blob { mailbox, blob, .. } => (self.sink)(mailbox, blob.encode()),
                    MuxEvent::PutOk { put_id } => {
                        self.in_flight.remove(&put_id);
                    }
                    // A relay refused it, or none answered: it goes round again until its time is up.
                    MuxEvent::PutFailed { put_id, .. } => {
                        if let Some(p) = self.in_flight.remove(&put_id) {
                            if Instant::now() < p.until {
                                self.puts.push(p);
                            }
                        }
                    }
                }
            }
            for a in out.actions {
                let now = self.now();
                match a {
                    Action::Connect { url } => match connect(&url, self.socks.as_deref()) {
                        Ok(ws) => {
                            self.sockets.insert(url.clone(), ws);
                            pending.push(self.mux.socket_opened(&url, now));
                        }
                        Err(_) => pending.push(self.mux.socket_closed(&url, now)),
                    },
                    Action::Send { url, text } => {
                        let failed = self
                            .sockets
                            .get_mut(&url)
                            .is_none_or(|s| s.send(Message::text(text)).is_err());
                        if failed {
                            self.sockets.remove(&url);
                            pending.push(self.mux.socket_closed(&url, now));
                        }
                    }
                    Action::Ping { url } => {
                        let failed = self
                            .sockets
                            .get_mut(&url)
                            .is_none_or(|s| s.send(Message::Ping(Vec::new().into())).is_err());
                        if failed {
                            self.sockets.remove(&url);
                            pending.push(self.mux.socket_closed(&url, now));
                        }
                    }
                    Action::Close { url } => {
                        if let Some(mut s) = self.sockets.remove(&url) {
                            let _ = s.close(None);
                        }
                    }
                }
            }
        }
    }

    /// Reads every socket once. Returns whether anything arrived.
    fn read_all(&mut self) -> bool {
        let mut any = false;
        let urls: Vec<String> = self.sockets.keys().cloned().collect();
        for url in urls {
            let now = self.now();
            let Some(sock) = self.sockets.get_mut(&url) else {
                continue;
            };
            let out = match sock.read() {
                Ok(Message::Text(t)) => {
                    any = true;
                    self.mux.socket_message(&url, t.as_str(), now)
                }
                Ok(Message::Close(_)) => {
                    self.sockets.remove(&url);
                    self.mux.socket_closed(&url, now)
                }
                Ok(_) => {
                    // A pong, a ping, anything: the other end is there.
                    self.mux.socket_alive(&url, now);
                    continue;
                }
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue
                }
                Err(_) => {
                    self.sockets.remove(&url);
                    self.mux.socket_closed(&url, now)
                }
            };
            self.apply(out);
        }
        any
    }

    fn tick_all(&mut self) {
        let now = self.now();
        // The worker ticks twice a second. A long gap means the machine slept: every socket is suspect, so all are
        // replaced at once instead of waiting out the silence.
        let slept =
            self.last_tick_wall != 0 && now - self.last_tick_wall > self.timing.resume_gap_ms;
        let out = if slept {
            self.mux.suspect_all(now)
        } else {
            self.mux.tick(now)
        };
        self.apply(out);
        // Measured from the end of the work, not the start: a tick that spent its time connecting to slow relays must not
        // make the next one believe the machine slept (that closed every healthy socket and started the storm again).
        self.last_tick_wall = self.now();
    }

    /// Sends queued puts once a relay is online. Returns whether one went out.
    fn flush_puts(&mut self) -> bool {
        let mut sent = false;
        let mut keep = Vec::new();
        for p in std::mem::take(&mut self.puts) {
            if self.mux.status() != SignalingStatus::Online {
                if Instant::now() < p.until {
                    keep.push(p);
                }
                continue;
            }
            self.put_seq += 1;
            let put_id = format!("n{}", self.put_seq);
            let now = self.now();
            let req = PutRequest {
                mailbox: &p.mailbox,
                blob: &p.blob,
                kind: p.kind,
                ttl_sec: p.ttl_sec,
                put_id: &put_id,
            };
            if let Ok(out) = self.mux.put(req, now, &mut OsEntropy) {
                self.in_flight.insert(put_id, p);
                self.apply(out);
                sent = true;
            }
        }
        self.puts.append(&mut keep);
        sent
    }

    fn report(&mut self) {
        let online = self.mux.status() == SignalingStatus::Online;
        if self.online.swap(online, Ordering::SeqCst) != online {
            (self.status)(online);
        }
    }
}

/// Opens a WebSocket (TLS for `wss://`), through a SOCKS5 proxy when one is given.
fn connect(url: &str, socks: Option<&str>) -> Result<Ws, String> {
    let request = url.into_client_request().map_err(|e| e.to_string())?;
    let uri = request.uri().clone();
    let host = uri
        .host()
        .ok_or("no host")?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("wss") {
            443
        } else {
            80
        });
    let tcp = match socks {
        Some(proxy) => socks5(proxy, &host, port)?,
        None => {
            let addr = (host.as_str(), port)
                .to_socket_addrs()
                .map_err(|e| e.to_string())?
                .next()
                .ok_or("no address")?;
            TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(|e| e.to_string())?
        }
    };
    tcp.set_read_timeout(Some(CONNECT_TIMEOUT))
        .map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(CONNECT_TIMEOUT))
        .map_err(|e| e.to_string())?;
    #[cfg(feature = "tls")]
    let (ws, _) = tungstenite::client_tls(request, tcp).map_err(|e| e.to_string())?;
    #[cfg(not(feature = "tls"))]
    let (ws, _) = {
        if uri.scheme_str() == Some("wss") {
            return Err("built without TLS".into());
        }
        tungstenite::client(request, MaybeTlsStream::Plain(tcp)).map_err(|e| e.to_string())?
    };
    // From here on a read only waits a moment, so one quiet relay does not hold up the others.
    let stream = match ws.get_ref() {
        MaybeTlsStream::Plain(s) => Some(s),
        #[cfg(feature = "tls")]
        MaybeTlsStream::NativeTls(s) => Some(s.get_ref()),
        _ => None,
    };
    if let Some(s) = stream {
        s.set_read_timeout(Some(READ_WAIT))
            .map_err(|e| e.to_string())?;
    }
    Ok(ws)
}

/// A minimal SOCKS5 CONNECT (no authentication), with the host name resolved by the proxy (Tor).
fn socks5(proxy: &str, host: &str, port: u16) -> Result<TcpStream, String> {
    let addr = proxy
        .trim_start_matches("socks5h://")
        .trim_start_matches("socks5://");
    let proxy_addr = addr
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("bad proxy")?;
    let mut s =
        TcpStream::connect_timeout(&proxy_addr, CONNECT_TIMEOUT).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(CONNECT_TIMEOUT))
        .map_err(|e| e.to_string())?;
    s.write_all(&[5, 1, 0]).map_err(|e| e.to_string())?;
    let mut reply = [0u8; 2];
    s.read_exact(&mut reply).map_err(|e| e.to_string())?;
    if reply != [5, 0] {
        return Err("the proxy refused".into());
    }
    let name = host.as_bytes();
    let len = u8::try_from(name.len()).map_err(|_| "host name too long")?;
    let mut req = vec![5, 1, 0, 3, len];
    req.extend_from_slice(name);
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).map_err(|e| e.to_string())?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).map_err(|e| e.to_string())?;
    if head[1] != 0 {
        return Err(format!("the proxy could not connect ({})", head[1]));
    }
    // The bound address that follows, which we do not need.
    let rest = match head[3] {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => {
            let mut l = [0u8; 1];
            s.read_exact(&mut l).map_err(|e| e.to_string())?;
            usize::from(l[0]) + 2
        }
        _ => return Err("bad proxy reply".into()),
    };
    let mut skip = vec![0u8; rest];
    s.read_exact(&mut skip).map_err(|e| e.to_string())?;
    Ok(s)
}
