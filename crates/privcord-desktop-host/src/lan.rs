//! The account's other devices on the local network, with no internet and no relay (docs/design/syncengine.md step 7).
//!
//! Every few seconds this sends the engine's beacon (`lan_beacon`) to the network's broadcast address and listens for
//! the others'. A beacon only devices of the account can read names one of them and the port it listens on; the side
//! the engine picks opens a TCP connection there. Its first line is a fresh beacon of its own, so the other side knows
//! (and checks) who it is; after that each line is one frame, and the engine authenticates and encrypts the link with
//! both devices' own keys, as over Bluetooth. Only the standard library: one thread for beacons out, one for beacons
//! in, one to accept, and one reader per connection.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

/// What the network side needs from the engine.
pub trait LanEngine: Send + Sync + 'static {
    /// The beacon to send now, saying we listen on `port` (`None`: nothing to find).
    fn beacon(&self, port: u16) -> Option<String>;
    /// A beacon heard: `(address, port, we connect)` when it is another device of ours.
    fn seen(&self, beacon: &str) -> Option<(String, u16, bool)>;
    fn link_up(&self, peer: &str);
    fn frame(&self, peer: &str, text: &str);
    fn link_down(&self, peer: &str);
}

/// Where beacons go and how often.
#[derive(Debug, Clone)]
pub struct LanConfig {
    /// The UDP port beacons are heard on.
    pub beacon_port: u16,
    /// Where beacons are sent (the broadcast address; loopback ports in tests).
    pub targets: Vec<SocketAddr>,
    pub every: Duration,
}

/// The well-known UDP port for beacons.
pub const BEACON_PORT: u16 = 47_811;
/// The longest line read from a connection (a frame is at most 128 KiB, and JSON-escaped text fits well within this).
const MAX_LINE: usize = 512 * 1024;
/// The longest first line (a beacon) accepted from a connection that has not proved anything yet.
const MAX_HELLO: usize = 4096;

impl Default for LanConfig {
    fn default() -> Self {
        Self {
            beacon_port: BEACON_PORT,
            targets: vec![SocketAddr::from(([255, 255, 255, 255], BEACON_PORT))],
            every: Duration::from_secs(5),
        }
    }
}

type Conns = Arc<Mutex<HashMap<String, TcpStream>>>;

impl std::fmt::Debug for Lan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lan")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

pub struct Lan {
    conns: Conns,
    stop: Arc<AtomicBool>,
    /// The TCP port this device listens on.
    pub port: u16,
}

impl Lan {
    pub fn start(config: LanConfig, engine: Arc<dyn LanEngine>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", 0))?;
        let port = listener.local_addr()?.port();
        let udp = UdpSocket::bind(("0.0.0.0", config.beacon_port))?;
        udp.set_broadcast(true)?;
        udp.set_read_timeout(Some(Duration::from_millis(500)))?;
        let lan = Self {
            conns: Arc::new(Mutex::new(HashMap::new())),
            stop: Arc::new(AtomicBool::new(false)),
            port,
        };
        // Beacons out.
        {
            let (udp, engine, stop, cfg) =
                (udp.try_clone()?, engine.clone(), lan.stop.clone(), config);
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if let Some(b) = engine.beacon(port) {
                        for t in &cfg.targets {
                            let _ = udp.send_to(b.as_bytes(), t);
                        }
                    }
                    thread::sleep(cfg.every);
                }
            });
        }
        // Beacons in: the side the engine picks connects.
        {
            let (engine, stop, conns) = (engine.clone(), lan.stop.clone(), lan.conns.clone());
            thread::spawn(move || {
                let mut buf = vec![0u8; 2048];
                while !stop.load(Ordering::Relaxed) {
                    let Ok((n, from)) = udp.recv_from(&mut buf) else {
                        continue;
                    };
                    let Some(text) = buf.get(..n).and_then(|b| std::str::from_utf8(b).ok()) else {
                        continue;
                    };
                    let Some((peer, their_port, initiator)) = engine.seen(text) else {
                        continue;
                    };
                    let known = conns.lock().is_ok_and(|c| c.contains_key(&peer));
                    if !initiator || known {
                        continue;
                    }
                    let Ok(stream) = TcpStream::connect_timeout(
                        &SocketAddr::new(from.ip(), their_port),
                        Duration::from_secs(3),
                    ) else {
                        continue;
                    };
                    // Who we are, for the other side: a fresh beacon of ours.
                    let Some(hello) = engine.beacon(port) else {
                        continue;
                    };
                    let mut w = match stream.try_clone() {
                        Ok(w) => w,
                        Err(_) => continue,
                    };
                    if writeln!(w, "{hello}").is_err() {
                        continue;
                    }
                    open(peer, stream, &conns, &engine, &stop);
                }
            });
        }
        // Connections in.
        {
            let (engine, stop, conns) = (engine, lan.stop.clone(), lan.conns.clone());
            listener.set_nonblocking(false)?;
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(stream) = stream else { continue };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut first = String::new();
                    let Ok(mut r) = stream.try_clone().map(BufReader::new) else {
                        continue;
                    };
                    // The first line is a beacon of a few hundred bytes: anything longer is not one, and is not read to the end
                    // (a machine on the network could otherwise send endless text without a line break).
                    if r.by_ref()
                        .take(MAX_HELLO as u64)
                        .read_line(&mut first)
                        .is_err()
                        || !first.ends_with('\n')
                    {
                        continue;
                    }
                    // Only another device of ours, saying so with a beacon of its own (one we would not connect to:
                    // it is the side that connects).
                    let Some((peer, _, false)) = engine.seen(first.trim()) else {
                        continue;
                    };
                    let _ = stream.set_read_timeout(None);
                    open(peer, stream, &conns, &engine, &stop);
                }
            });
        }
        Ok(lan)
    }

    /// Sends one frame to a device we are connected to. `false` when there is no such connection (or it broke).
    pub fn send(&self, peer: &str, frame: &str) -> bool {
        let Ok(mut conns) = self.conns.lock() else {
            return false;
        };
        let Some(stream) = conns.get_mut(peer) else {
            return false;
        };
        if writeln!(stream, "{frame}").is_ok() {
            return true;
        }
        conns.remove(peer);
        false
    }

    pub fn connected(&self, peer: &str) -> bool {
        self.conns.lock().is_ok_and(|c| c.contains_key(peer))
    }

    /// Closes the connection to one device (the engine dropped the link).
    pub fn close(&self, peer: &str) {
        if let Some(s) = self.conns.lock().ok().and_then(|mut c| c.remove(peer)) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut c) = self.conns.lock() {
            for (_, s) in c.drain() {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
        }
        // Wake the accept loop so it sees the stop.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// A connection to another device of ours is open: register it, tell the engine, and read frames until it ends.
fn open(
    peer: String,
    stream: TcpStream,
    conns: &Conns,
    engine: &Arc<dyn LanEngine>,
    stop: &Arc<AtomicBool>,
) {
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    if let Ok(mut c) = conns.lock() {
        if let Some(old) = c.insert(peer.clone(), stream) {
            let _ = old.shutdown(std::net::Shutdown::Both);
        }
    }
    engine.link_up(&peer);
    let (conns, engine, stop) = (conns.clone(), engine.clone(), stop.clone());
    thread::spawn(move || {
        let mut r = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match r.by_ref().take(MAX_LINE as u64).read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) if !line.ends_with('\n') => break, // too long: not one of ours
                Ok(_) => engine.frame(&peer, line.trim_end()),
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
        }
        let gone = conns.lock().is_ok_and(|mut c| c.remove(&peer).is_some());
        if gone {
            engine.link_down(&peer);
        }
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::sync::mpsc::{channel, Sender};

    /// Two "devices" that know each other's beacon text.
    struct Fake {
        me: &'static str,
        other: &'static str,
        connect: bool,
        events: Mutex<Sender<String>>,
    }

    impl LanEngine for Fake {
        fn beacon(&self, port: u16) -> Option<String> {
            Some(format!("dcl1.{}.{port}", self.me))
        }
        fn seen(&self, beacon: &str) -> Option<(String, u16, bool)> {
            let rest = beacon.strip_prefix("dcl1.")?;
            let (who, port) = rest.split_once('.')?;
            (who == self.other).then(|| (who.to_owned(), port.parse().unwrap(), self.connect))
        }
        fn link_up(&self, peer: &str) {
            let _ = self
                .events
                .lock()
                .unwrap()
                .send(format!("{}: up {peer}", self.me));
        }
        fn frame(&self, peer: &str, text: &str) {
            let _ = self
                .events
                .lock()
                .unwrap()
                .send(format!("{}: {peer} says {text}", self.me));
        }
        fn link_down(&self, peer: &str) {
            let _ = self
                .events
                .lock()
                .unwrap()
                .send(format!("{}: down {peer}", self.me));
        }
    }

    fn free_udp_port() -> u16 {
        UdpSocket::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[test]
    fn two_devices_find_each_other_and_exchange_frames() {
        let (tx, rx) = channel();
        let (pa, pb) = (free_udp_port(), free_udp_port());
        let cfg = |to: u16, me: u16| LanConfig {
            beacon_port: me,
            targets: vec![SocketAddr::from(([127, 0, 0, 1], to))],
            every: Duration::from_millis(100),
        };
        let a = Lan::start(
            cfg(pb, pa),
            Arc::new(Fake {
                me: "a",
                other: "b",
                connect: true,
                events: Mutex::new(tx.clone()),
            }),
        )
        .unwrap();
        let b = Lan::start(
            cfg(pa, pb),
            Arc::new(Fake {
                me: "b",
                other: "a",
                connect: false,
                events: Mutex::new(tx),
            }),
        )
        .unwrap();
        let mut seen = Vec::new();
        let wait = |seen: &mut Vec<String>, what: &str| {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !seen.iter().any(|e| e == what) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "waiting for {what}: {seen:?}"
                );
                if let Ok(e) = rx.recv_timeout(Duration::from_millis(200)) {
                    seen.push(e);
                }
            }
        };
        wait(&mut seen, "a: up b");
        wait(&mut seen, "b: up a");
        assert!(a.send("b", "{\"kind\":\"ping\"}"));
        wait(&mut seen, "b: a says {\"kind\":\"ping\"}");
        assert!(b.send("a", "hello back"));
        wait(&mut seen, "a: b says hello back");
        // One connection, not two.
        assert_eq!(seen.iter().filter(|e| e.contains(": up ")).count(), 2);
        a.close("b");
        wait(&mut seen, "b: down a");
        a.stop();
        b.stop();
    }
}
