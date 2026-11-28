//! The TURN role: relaying traffic for phones that cannot connect to each other directly.
//!
//! Credentials are short-lived: a user name that carries the moment it stops working, and a password that is a keyed hash
//! of it, so the server keeps no list of accounts and a leaked password stops working within the hour. (Fixed accounts can
//! also be set in the config, for apps that can only type a password in by hand.)

use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// The secret that signs credentials: made once and kept in the data folder.
pub fn load_or_make_secret(dir: &std::path::Path) -> Result<Vec<u8>, String> {
    load_or_make_secret_with(dir, &crate::record::Codec::plain())
}

/// The same, written through the settings' codec: encrypted at rest when a storage key is set (an older plain file is read, and
/// written again encrypted).
pub fn load_or_make_secret_with(
    dir: &std::path::Path,
    codec: &crate::record::Codec,
) -> Result<Vec<u8>, String> {
    let path = dir.join("turn-secret");
    if let Ok(text) = std::fs::read_to_string(&path) {
        let line = text.trim();
        let hex = match codec.decode("turn-secret", line) {
            crate::record::Decoded::Payload(p) => Some(p),
            crate::record::Decoded::NeedsKey => {
                return Err(format!(
                    "{} is encrypted but no storage key is set (storage.key_file, or the storage.key_env variable)",
                    path.display()
                ))
            }
            // A plain secret (the shape before encryption was an option) is just the hex; an encrypted one that does not open
            // is the wrong key: nothing is replaced.
            crate::record::Decoded::Corrupt if line.starts_with("e1 ") => {
                return Err(format!(
                    "{} cannot be read with this storage key (nothing was changed)",
                    path.display()
                ))
            }
            crate::record::Decoded::Corrupt => Some(line.to_owned()),
        };
        if let Some(hex) = hex.filter(|h| h.len() == 64) {
            if codec.encrypts() && !line.starts_with("e1 ") {
                crate::fsutil::write_private_atomic(
                    &path,
                    codec.encode("turn-secret", &hex).as_bytes(),
                )?;
            }
            return Ok(hex.into_bytes());
        }
    }
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| format!("no randomness: {e}"))?;
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    crate::fsutil::create_private_dir(dir)?;
    let written = if codec.encrypts() {
        codec.encode("turn-secret", &hex)
    } else {
        hex.clone()
    };
    crate::fsutil::write_private_atomic(&path, written.as_bytes())?;
    Ok(hex.into_bytes())
}

fn password_for(secret: &[u8], username: &str) -> String {
    // HMAC accepts a key of any length, so this does not fail; if it ever did, no password could match.
    let Ok(mut mac) = <HmacSha256 as KeyInit>::new_from_slice(secret) else {
        return String::new();
    };
    mac.update(username.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// A user name and password valid for `ttl_secs` from `now_secs`.
pub fn issue(secret: &[u8], now_secs: u64, ttl_secs: u64) -> (String, String) {
    let mut tag = [0u8; 4];
    crate::fsutil::random(&mut tag);
    let tag: String = tag.iter().map(|b| format!("{b:02x}")).collect();
    let username = format!("{}:{tag}", now_secs.saturating_add(ttl_secs));
    let password = password_for(secret, &username);
    (username, password)
}

/// The password for `username` if it is one of ours and has not expired.
pub fn password_if_valid(secret: &[u8], username: &str, now_secs: u64) -> Option<String> {
    let (expires, _) = username.split_once(':')?;
    let expires: u64 = expires.parse().ok()?;
    (expires > now_secs).then(|| password_for(secret, username))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_credential_works_until_it_expires_and_only_with_its_own_name() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let secret = &load_or_make_secret(a.path()).unwrap()[..];
        let other = load_or_make_secret(b.path()).unwrap();
        let (user, pass) = issue(secret, 1_000, 3_600);
        assert_eq!(
            password_if_valid(secret, &user, 1_000).as_deref(),
            Some(pass.as_str())
        );
        assert_eq!(
            password_if_valid(secret, &user, 4_599).as_deref(),
            Some(pass.as_str())
        );
        assert!(password_if_valid(secret, &user, 4_600).is_none(), "expired");
        assert!(
            password_if_valid(&other, &user, 1_000).is_some_and(|p| p != pass),
            "another server's secret gives another password"
        );
        assert!(password_if_valid(secret, "not-a-time:abc", 1_000).is_none());
        assert!(password_if_valid(secret, "nocolon", 1_000).is_none());
        // Changing the expiry changes the password: a name cannot be stretched.
        let stretched = user.replacen(
            &format!("{}:", 1_000 + 3_600),
            &format!("{}:", 9_999_999),
            1,
        );
        assert_ne!(password_if_valid(secret, &stretched, 1_000), Some(pass));
    }
}

/// Whether a relayed packet may go to this address. A TURN server that relays to anywhere can be used to reach the
/// machine it runs on and the network behind it (a router's login page, a cloud provider's metadata service), so those are
/// refused unless the administrator says otherwise.
pub fn is_blocked(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || (o[0] == 100 && (64..128).contains(&o[1])) // carrier-grade NAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // protocol assignments
                || (o[0] == 198 && (18..20).contains(&o[1])) // benchmarking
                || o[0] >= 240 // reserved
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00 // unique local
                || (seg[0] & 0xffc0) == 0xfe80 // link local
        }
    }
}

#[cfg(feature = "turn")]
pub mod server {
    use super::{is_blocked, password_if_valid};
    use crate::config::TurnConfig;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use turn::auth::{generate_auth_key, AuthHandler};
    use turn::relay::relay_range::RelayAddressGeneratorRanges;
    use turn::relay::RelayAddressGenerator;
    use turn::server::config::{ConnConfig, ServerConfig};
    use turn::server::Server;
    use webrtc_util::vnet::net::Net;
    use webrtc_util::Conn;

    struct Auth {
        usage: Arc<crate::limits::Usage>,
        /// STUN-only: nobody gets a login, so nobody can allocate a relay (Binding needs none).
        stun_only: bool,
        secret: Vec<u8>,
        users: HashMap<String, String>,
    }

    impl AuthHandler for Auth {
        fn auth_handle(
            &self,
            username: &str,
            realm: &str,
            _src: SocketAddr,
        ) -> Result<Vec<u8>, turn::Error> {
            if self.stun_only {
                return Err(turn::Error::Other("this server only answers STUN".into()));
            }
            if self.usage.over_cap() {
                return Err(turn::Error::Other("traffic allowance used up".into()));
            }
            if let Some(password) = self.users.get(username) {
                return Ok(generate_auth_key(username, realm, password));
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            match password_if_valid(&self.secret, username, now) {
                Some(password) => Ok(generate_auth_key(username, realm, &password)),
                None => Err(turn::Error::Other("unknown or expired user".into())),
            }
        }
    }

    /// One relayed connection: refuses to send to addresses it should not, and keeps to a speed limit.
    struct Guard {
        inner: Arc<dyn Conn + Send + Sync>,
        /// The relay ports in use, and this one's own, so the allocation can be taken off the list when it ends.
        live: LivePorts,
        port: u16,
        /// This server's address as phones see it. Traffic between two of our own allocations is delivered locally, because
        /// many routers (and some clouds) cannot loop a packet back to their own public address.
        public_ip: std::net::IpAddr,
        allow_private: bool,
        bucket: Option<Mutex<Bucket>>,
        usage: Arc<crate::limits::Usage>,
    }

    struct Bucket {
        tokens: f64,
        last: Instant,
        per_sec: f64,
    }

    impl Guard {
        /// A packet that came from this machine itself is, to everybody else, from the public address.
        fn is_live_port(&self, port: u16) -> bool {
            self.live
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&port)
        }

        fn as_seen_from_outside(&self, from: SocketAddr) -> SocketAddr {
            // Only another allocation of ours can be the source: anything else on this machine that happens to use a
            // loopback address is not presented as a legitimate peer.
            if from.ip().is_loopback() && self.is_live_port(from.port()) {
                SocketAddr::new(self.public_ip, from.port())
            } else {
                from
            }
        }

        fn allows(&self, target: SocketAddr, len: usize) -> bool {
            if !self.allow_private && is_blocked(target.ip()) {
                return false;
            }
            let Some(b) = &self.bucket else {
                return true;
            };
            let mut b = b.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = Instant::now();
            let burst = b.per_sec; // a second's worth
            b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * b.per_sec).min(burst);
            b.last = now;
            if b.tokens >= len as f64 {
                b.tokens -= len as f64;
                true
            } else {
                false
            }
        }
    }

    #[async_trait]
    impl Conn for Guard {
        async fn connect(&self, addr: SocketAddr) -> webrtc_util::Result<()> {
            self.inner.connect(addr).await
        }
        async fn recv(&self, buf: &mut [u8]) -> webrtc_util::Result<usize> {
            self.inner.recv(buf).await
        }
        async fn recv_from(&self, buf: &mut [u8]) -> webrtc_util::Result<(usize, SocketAddr)> {
            let (n, from) = self.inner.recv_from(buf).await?;
            self.usage.turn(n, 0);
            Ok((n, self.as_seen_from_outside(from)))
        }
        async fn send(&self, buf: &[u8]) -> webrtc_util::Result<usize> {
            self.inner.send(buf).await
        }
        async fn send_to(&self, buf: &[u8], target: SocketAddr) -> webrtc_util::Result<usize> {
            if self.usage.over_cap() || !self.allows(target, buf.len()) {
                // Dropped, as a congested network would: the sender is told it went.
                return Ok(buf.len());
            }
            // To our own public address and another allocation's port: deliver it here instead (the receiving side maps it
            // back, see `recv_from`). Only for a live relay port: a client must not be able to aim a packet at any other
            // port on this machine's loopback (a local resolver, a database) by naming our public address.
            let target = match (target.ip() == self.public_ip, self.public_ip) {
                (true, std::net::IpAddr::V4(_)) if self.is_live_port(target.port()) => {
                    SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), target.port())
                }
                _ => target,
            };
            self.usage.turn(0, buf.len());
            self.inner.send_to(buf, target).await
        }
        fn local_addr(&self) -> webrtc_util::Result<SocketAddr> {
            self.inner.local_addr()
        }
        fn remote_addr(&self) -> Option<SocketAddr> {
            self.inner.remote_addr()
        }
        async fn close(&self) -> webrtc_util::Result<()> {
            self.inner.close().await
        }
        fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
            self
        }
    }

    /// The relay ports of the allocations that exist now (shared by every transport).
    type LivePorts = Arc<Mutex<std::collections::HashSet<u16>>>;

    struct Gen {
        inner: RelayAddressGeneratorRanges,
        allow_private: bool,
        kbps: u64,
        live: LivePorts,
        usage: Arc<crate::limits::Usage>,
    }

    #[async_trait]
    impl RelayAddressGenerator for Gen {
        fn validate(&self) -> Result<(), turn::Error> {
            self.inner.validate()
        }
        fn supports_family(&self, use_ipv4: bool) -> bool {
            self.inner.supports_family(use_ipv4)
        }
        async fn allocate_conn(
            &self,
            use_ipv4: bool,
            requested_port: u16,
        ) -> Result<(Arc<dyn Conn + Send + Sync>, SocketAddr), turn::Error> {
            let (conn, addr) = self.inner.allocate_conn(use_ipv4, requested_port).await?;
            let bucket = (self.kbps > 0).then(|| {
                Mutex::new(Bucket {
                    tokens: self.kbps as f64 * 125.0,
                    last: Instant::now(),
                    per_sec: self.kbps as f64 * 125.0,
                })
            });
            self.live
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(addr.port());
            Ok((
                Arc::new(Guard {
                    inner: conn,
                    live: self.live.clone(),
                    port: addr.port(),
                    public_ip: self.inner.relay_address,
                    allow_private: self.allow_private,
                    bucket,
                    usage: self.usage.clone(),
                }),
                addr,
            ))
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.live
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.port);
        }
    }

    /// A stand-in for a relay socket, for tests: remembers where packets were sent and plays back sources it is given.
    #[cfg(test)]
    #[derive(Clone, Default, Debug)]
    pub struct MockConn {
        sent: Arc<Mutex<Vec<SocketAddr>>>,
        incoming: Arc<Mutex<Vec<SocketAddr>>>,
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    impl MockConn {
        pub fn sent(&self) -> Vec<SocketAddr> {
            self.sent.lock().unwrap().clone()
        }
        pub fn queue_incoming(&self, a: SocketAddr) {
            self.incoming.lock().unwrap().push(a);
        }
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    #[async_trait]
    impl Conn for MockConn {
        async fn connect(&self, _: SocketAddr) -> webrtc_util::Result<()> {
            Ok(())
        }
        async fn recv(&self, b: &mut [u8]) -> webrtc_util::Result<usize> {
            self.recv_from(b).await.map(|x| x.0)
        }
        async fn recv_from(&self, _: &mut [u8]) -> webrtc_util::Result<(usize, SocketAddr)> {
            let mut q = self.incoming.lock().unwrap();
            if q.is_empty() {
                return Err(webrtc_util::Error::ErrUseClosedNetworkConn);
            }
            Ok((0, q.remove(0)))
        }
        async fn send(&self, b: &[u8]) -> webrtc_util::Result<usize> {
            Ok(b.len())
        }
        async fn send_to(&self, b: &[u8], t: SocketAddr) -> webrtc_util::Result<usize> {
            self.sent.lock().unwrap().push(t);
            Ok(b.len())
        }
        fn local_addr(&self) -> webrtc_util::Result<SocketAddr> {
            Ok("0.0.0.0:0".parse().unwrap())
        }
        fn remote_addr(&self) -> Option<SocketAddr> {
            None
        }
        async fn close(&self) -> webrtc_util::Result<()> {
            Ok(())
        }
        fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
            self
        }
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    pub fn test_guard(inner: MockConn, public_ip: &str, live_ports: &[u16]) -> impl Conn {
        Guard {
            inner: Arc::new(inner),
            live: Arc::new(Mutex::new(live_ports.iter().copied().collect())),
            port: 0,
            public_ip: public_ip.parse().unwrap(),
            allow_private: false,
            bucket: None,
            usage: crate::limits::Usage::unlimited(),
        }
    }

    pub struct TurnServer {
        server: Server,
        /// Where the UDP side listens.
        pub addr: SocketAddr,
        /// Where TURN over TCP / over TLS listens, if enabled.
        pub tcp_addr: Option<SocketAddr>,
        pub tls_addr: Option<SocketAddr>,
        stop: tokio::sync::watch::Sender<bool>,
        listeners: Vec<tokio::task::JoinHandle<()>>,
    }

    impl std::fmt::Debug for TurnServer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("TurnServer")
        }
    }

    impl TurnServer {
        pub async fn close(&self) {
            let _ = self.stop.send(true);
            let _ = self.server.close().await;
        }

        /// Waits for the TCP and TLS listeners to finish (after `close`).
        pub async fn finish(self) {
            for l in self.listeners {
                let _ = l.await;
            }
        }
    }

    /// Starts the TURN server: UDP, and TURN over TCP and over TLS when asked. `public_ip` is what phones are told to send
    /// traffic to.
    pub async fn start(
        cfg: &TurnConfig,
        secret: Vec<u8>,
        public_ip: std::net::IpAddr,
        tls: Option<&crate::tls::Tls>,
        usage: Arc<crate::limits::Usage>,
    ) -> Result<TurnServer, String> {
        let socket = tokio::net::UdpSocket::bind(&cfg.listen)
            .await
            .map_err(|e| format!("could not listen for TURN on {} (UDP): {e}", cfg.listen))?;
        let addr = socket.local_addr().map_err(|e| e.to_string())?;
        let users: HashMap<String, String> = cfg
            .users
            .iter()
            .filter_map(|u| u.split_once(':').map(|(n, p)| (n.to_owned(), p.to_owned())))
            .collect();
        // Every transport relays the same way: its own generator (they share the port range; a taken port is retried).
        let live: LivePorts = Arc::new(Mutex::new(std::collections::HashSet::new()));
        let generator = || -> Box<dyn RelayAddressGenerator + Send + Sync> {
            Box::new(Gen {
                inner: RelayAddressGeneratorRanges {
                    relay_address: public_ip,
                    // STUN-only never allocates, but the library wants a range.
                    min_port: if cfg.stun_only { 49152 } else { cfg.min_port },
                    max_port: if cfg.stun_only { 49152 } else { cfg.max_port },
                    max_retries: 20,
                    address: "0.0.0.0".into(),
                    net: Arc::new(Net::new(None)),
                },
                allow_private: cfg.allow_private_peers,
                kbps: cfg.max_kbps,
                live: live.clone(),
                usage: usage.clone(),
            })
        };
        let mut conn_configs = vec![ConnConfig {
            conn: Arc::new(socket),
            relay_addr_generator: generator(),
        }];
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let mut listeners = Vec::new();
        let stream_listener = |listen: &str, acceptor: Option<tokio_rustls::TlsAcceptor>| {
            let listen = listen.to_owned();
            let stopped = stopped.clone();
            async move {
                let tcp = tokio::net::TcpListener::bind(&listen)
                    .await
                    .map_err(|e| format!("could not listen for TURN on {listen} (TCP): {e}"))?;
                let local = tcp.local_addr().map_err(|e| e.to_string())?;
                let mux = crate::turn_stream::StreamMux::new(local);
                let task = crate::turn_stream::spawn_listener(mux.clone(), tcp, acceptor, stopped);
                Ok::<_, String>((mux, task, local))
            }
        };
        let (mut tcp_addr, mut tls_addr) = (None, None);
        if !cfg.tcp_listen.is_empty() {
            let (mux, task, local) = stream_listener(&cfg.tcp_listen, None).await?;
            conn_configs.push(ConnConfig {
                conn: mux,
                relay_addr_generator: generator(),
            });
            listeners.push(task);
            tcp_addr = Some(local);
        }
        if !cfg.tls_listen.is_empty() {
            let tls = tls.ok_or("turn.tls_listen needs a certificate ([tls])")?;
            let (mux, task, local) =
                stream_listener(&cfg.tls_listen, Some(tls.turn.clone())).await?;
            conn_configs.push(ConnConfig {
                conn: mux,
                relay_addr_generator: generator(),
            });
            listeners.push(task);
            tls_addr = Some(local);
        }
        let server = Server::new(ServerConfig {
            conn_configs,
            realm: cfg.realm.clone(),
            auth_handler: Arc::new(Auth {
                usage: usage.clone(),
                stun_only: cfg.stun_only,
                secret,
                users,
            }),
            channel_bind_timeout: Duration::from_secs(0),
            alloc_close_notify: None,
        })
        .await
        .map_err(|e| format!("could not start TURN: {e}"))?;
        Ok(TurnServer {
            server,
            addr,
            tcp_addr,
            tls_addr,
            stop,
            listeners,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod block_tests {
    use super::is_blocked;

    #[test]
    fn the_machine_itself_and_the_networks_behind_it_are_off_limits() {
        for bad in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:192.168.0.1",
            "::ffff:127.0.0.1",
            "255.255.255.255",
        ] {
            assert!(is_blocked(bad.parse().unwrap()), "{bad} should be blocked");
        }
        for fine in [
            "8.8.8.8",
            "1.1.1.1",
            "172.32.0.1",
            "100.63.255.255",
            "2606:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(
                !is_blocked(fine.parse().unwrap()),
                "{fine} should be allowed"
            );
        }
    }
}

#[cfg(all(test, feature = "turn"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod local_delivery_tests {
    use super::server::{test_guard, MockConn};
    use std::net::SocketAddr;
    use webrtc_util::Conn;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[tokio::test]
    async fn a_packet_to_our_own_public_address_goes_to_loopback_only_for_a_live_relay_port() {
        let mock = MockConn::default();
        let g = test_guard(mock.clone(), "203.0.113.10", &[49300]);
        g.send_to(b"x", addr("203.0.113.10:49300")).await.unwrap();
        g.send_to(b"y", addr("203.0.113.10:53")).await.unwrap(); // not an allocation: must not become 127.0.0.1:53
        g.send_to(b"z", addr("8.8.8.8:53")).await.unwrap();
        g.send_to(b"w", addr("127.0.0.1:53")).await.unwrap(); // loopback itself is refused outright
        assert_eq!(
            mock.sent(),
            vec![
                addr("127.0.0.1:49300"),
                addr("203.0.113.10:53"),
                addr("8.8.8.8:53")
            ]
        );
    }

    #[tokio::test]
    async fn only_a_live_relay_port_on_loopback_is_shown_as_coming_from_the_public_address() {
        let mock = MockConn::default();
        let g = test_guard(mock.clone(), "203.0.113.10", &[49300]);
        mock.queue_incoming(addr("127.0.0.1:49300"));
        mock.queue_incoming(addr("127.0.0.1:53")); // some other program on this machine
        mock.queue_incoming(addr("198.51.100.7:4000"));
        let mut buf = [0u8; 8];
        assert_eq!(
            g.recv_from(&mut buf).await.unwrap().1,
            addr("203.0.113.10:49300")
        );
        assert_eq!(
            g.recv_from(&mut buf).await.unwrap().1,
            addr("127.0.0.1:53"),
            "must not be presented as the public address"
        );
        assert_eq!(
            g.recv_from(&mut buf).await.unwrap().1,
            addr("198.51.100.7:4000")
        );
    }
}
