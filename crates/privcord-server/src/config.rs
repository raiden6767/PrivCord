//! The server's settings: a TOML file (or the defaults), with a few switches on the command line. Every limit has a safe
//! default, so an empty file is a working relay.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub admin: AdminConfig,
    pub tls: TlsConfig,
    pub storage: StorageConfig,
    pub nostr: NostrConfig,
    pub server: ServerConfig,
    pub relay: RelayConfig,
    pub turn: TurnConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Where to listen for the relay (WebSocket) and the small web pages (`/healthz`, `/turn`).
    pub listen: String,
    /// Where the relay keeps what is waiting for people (and the TURN secret).
    pub data_dir: PathBuf,
    /// Read the visitor's address from `X-Forwarded-For` (only behind a proxy you control, such as Caddy or nginx).
    pub trust_proxy: bool,
    /// With `trust_proxy`: the header that carries the client's address (the last entry is used), for example
    /// `cf-connecting-ip` behind Cloudflare.
    pub real_ip_header: String,
    /// With `trust_proxy`: the addresses of your proxy. The header is believed only from these; from anyone else it is
    /// ignored (so a visitor who reaches the server directly cannot pick the address limits are counted against). Empty:
    /// believed from any caller, which is only safe if nothing but your proxy can reach this port.
    pub trusted_proxies: Vec<String>,
    /// Serve Prometheus-style counters at `/metrics` (needs an access token if tokens are set).
    pub metrics: bool,
    /// Who may use this server. Empty: anyone. Otherwise a visitor must present one of these (as `?token=...` on the relay
    /// address, or as `Authorization: Bearer ...`).
    pub access_tokens: Vec<String>,
    /// The words shown to people who add this server: who runs it and how to reach them.
    pub admin_contact: String,
    /// A client has this long to send the whole request head (and, between requests, to start the next one) before the
    /// connection is closed: a client that sends one byte a minute cannot hold a connection (the "slow header" attack).
    pub header_timeout_secs: u64,
    /// A request that has not been answered after this long gets a 408 (a body that trickles in, a stuck handler).
    pub request_timeout_secs: u64,
    /// Connections in the request/response phase, in all (a WebSocket leaves this count once it is upgraded: the relay's own
    /// limits apply to it).
    pub max_http_connections: usize,
    /// The same, from one address (a /64 for IPv6). 0: no limit; it is also off with `trust_proxy`, where every visitor
    /// arrives from the proxy.
    pub max_http_connections_per_ip: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RelayConfig {
    pub enabled: bool,
    /// Keep sealed messages for people who are offline (up to a week). Off: only live signals pass through.
    pub store_and_forward: bool,
    /// Messages per minute from one address.
    pub rate_limit: u64,
    pub max_subs_per_conn: usize,
    pub max_per_mailbox_per_ip: usize,
    pub max_conns_per_ip: usize,
    /// Messages kept in all, and their total size.
    pub max_queued: usize,
    pub max_queued_bytes: usize,
    /// What may wait to be written to one connection before it is cut off as too slow.
    pub max_buffered_per_conn: usize,
    /// When this much is waiting to be written across all connections, senders are slowed down (their sockets are not
    /// read) until listeners catch up. Keeps memory flat under a flood.
    pub backpressure_bytes: usize,
    /// A connection that has been sent something but has not taken any of it for this long is closed (a client that stops
    /// reading must not be able to hold the relay's memory). 0: never.
    pub stall_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TurnConfig {
    pub enabled: bool,
    /// Where the TURN server listens (UDP).
    pub listen: String,
    /// Also accept TURN over TCP here, for networks that block UDP (for example `0.0.0.0:3478`). Empty: off.
    pub tcp_listen: String,
    /// Also accept TURN over TLS here (needs `[tls]`), for networks that only let TLS through (for example `0.0.0.0:443`
    /// or `0.0.0.0:5349`). Empty: off.
    pub tls_listen: String,
    /// The address phones reach this machine at from the internet (the one in the TURN answers). Required when enabled.
    pub public_ip: String,
    pub realm: String,
    /// The ports used to relay traffic. Open this UDP range in your firewall.
    pub min_port: u16,
    pub max_port: u16,
    /// How long handed-out credentials last.
    pub ttl_secs: u64,
    /// Fixed accounts, `name:password`, for apps that can only type a TURN password in by hand.
    pub users: Vec<String>,
    /// Let people relay to private addresses (this machine, your home network). Off, and best left off.
    pub allow_private_peers: bool,
    /// A speed limit for each relayed connection, in kilobits per second. 0: none.
    pub max_kbps: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8787".into(),
            data_dir: PathBuf::from("./data"),
            trust_proxy: false,
            real_ip_header: "x-forwarded-for".into(),
            trusted_proxies: Vec::new(),
            metrics: false,
            access_tokens: Vec::new(),
            admin_contact: String::new(),
            header_timeout_secs: 10,
            request_timeout_secs: 30,
            max_http_connections: 20_000,
            max_http_connections_per_ip: 100,
        }
    }
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            store_and_forward: true,
            rate_limit: 600,
            max_subs_per_conn: 64,
            max_per_mailbox_per_ip: 100,
            max_conns_per_ip: 20,
            max_queued: 50_000,
            max_queued_bytes: 256 * 1024 * 1024,
            max_buffered_per_conn: 64 * 1024 * 1024,
            backpressure_bytes: 32 * 1024 * 1024,
            stall_secs: 30,
        }
    }
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "0.0.0.0:3478".into(),
            tcp_listen: String::new(),
            tls_listen: String::new(),
            public_ip: String::new(),
            realm: "privcord".into(),
            min_port: 49152,
            max_port: 49400,
            ttl_secs: 3600,
            users: Vec::new(),
            allow_private_peers: false,
            max_kbps: 0,
        }
    }
}

impl Config {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Whether anything here needs a certificate.
    pub fn wants_tls(&self) -> bool {
        !self.tls.https_listen.is_empty()
            || (self.turn.enabled && !self.turn.tls_listen.is_empty())
            || (self.admin.enabled && self.admin.tls)
    }

    /// What would stop this from working, in words (empty: fine).
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.relay.enabled && !self.turn.enabled && !self.nostr.enabled {
            out.push("the relay, Nostr and TURN are all off: there is nothing to run".into());
        }
        if self.server.listen.parse::<std::net::SocketAddr>().is_err() {
            out.push(format!(
                "server.listen \"{}\" is not an address like 0.0.0.0:8787",
                self.server.listen
            ));
        }
        if self.admin.enabled
            && !self.admin.tls
            && !self.admin.allow_plain_http
            && self
                .admin
                .listen
                .parse::<std::net::SocketAddr>()
                .is_ok_and(|a| !a.ip().is_loopback())
        {
            out.push(format!(
                "admin.listen \"{}\" is not this machine and admin.tls is off: admin tokens would cross the network unencrypted. \
                 Set admin.tls = true (with [tls]), listen on 127.0.0.1 and use an SSH tunnel, or set admin.allow_plain_http = true \
                 if the network is a VPN you trust",
                self.admin.listen
            ));
        }
        if self.nostr.enabled
            && (self.nostr.auth_required || self.nostr.auth_dms)
            && self.nostr.relay_url.trim().is_empty()
        {
            out.push("nostr.relay_url is needed when auth_required or auth_dms is on (NIP-42 checks the client signed for this relay), for example \"wss://relay.example.org\"".into());
        }
        for k in self
            .nostr
            .allowed_pubkeys
            .iter()
            .chain(&self.nostr.blocked_pubkeys)
        {
            if k.len() != 64 || !k.bytes().all(|b| b.is_ascii_hexdigit()) {
                out.push(format!("nostr key \"{k}\" is not 64 hex characters"));
            }
        }
        if self.admin.enabled && self.admin.listen.parse::<std::net::SocketAddr>().is_err() {
            out.push(format!(
                "admin.listen \"{}\" is not an address like 127.0.0.1:8788",
                self.admin.listen
            ));
        }
        if self.wants_tls() && self.tls.cert_file.is_empty() && !self.tls.self_signed {
            out.push("TLS is asked for (tls.https_listen or turn.tls_listen) but there is no certificate: set tls.cert_file and tls.key_file, or tls.self_signed = true to try things out".into());
        }
        if self.tls.cert_file.is_empty() != self.tls.key_file.is_empty() {
            out.push("tls.cert_file and tls.key_file go together".into());
        }
        for (name, v) in [
            ("tls.https_listen", &self.tls.https_listen),
            ("turn.tcp_listen", &self.turn.tcp_listen),
            ("turn.tls_listen", &self.turn.tls_listen),
        ] {
            if !v.is_empty() && v.parse::<std::net::SocketAddr>().is_err() {
                out.push(format!("{name} \"{v}\" is not an address like 0.0.0.0:443"));
            }
        }
        if self.turn.enabled {
            if self.turn.public_ip.parse::<std::net::IpAddr>().is_err() {
                out.push("turn.public_ip must be this machine's public IP address (phones are told to send traffic there)".into());
            }
            if self.turn.listen.parse::<std::net::SocketAddr>().is_err() {
                out.push(format!(
                    "turn.listen \"{}\" is not an address like 0.0.0.0:3478",
                    self.turn.listen
                ));
            }
            if self.turn.min_port == 0 || self.turn.max_port < self.turn.min_port {
                out.push(
                    "turn.min_port and turn.max_port must be a range of ports, lowest first".into(),
                );
            }
            for u in &self.turn.users {
                if !u.contains(':') {
                    out.push(format!(
                        "turn.users entry \"{u}\" must look like name:password"
                    ));
                }
            }
        }
        out
    }
}

/// The Nostr relay role (NIP-01 and friends), on the same port as the PrivCord relay.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NostrConfig {
    /// Off unless you want this server to be a Nostr relay too. Anything published is stored readable by you, the operator.
    pub enabled: bool,
    /// Keep events (regular, replaceable, addressable). Off: only live delivery.
    pub store_events: bool,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub max_event_bytes: usize,
    pub max_message_bytes: usize,
    pub max_tags: usize,
    pub max_subs_per_conn: usize,
    pub max_filters: usize,
    pub max_limit: usize,
    pub max_subid_len: usize,
    pub events_per_minute_per_ip: u64,
    /// Events per second the whole relay accepts, all authors together (protects the disk). 0: no cap.
    pub events_per_second_global: u64,
    /// Subscriptions (REQ) one connection may open per minute. 0: no cap.
    pub reqs_per_minute_per_conn: u64,
    /// Most values (ids, authors, kinds, tag values) in one filter. Each costs work on every stored event.
    pub max_filter_values: usize,
    /// The most bytes of events kept in memory and on disk; the oldest go first. 0: no limit (not for a public relay).
    pub max_store_bytes: usize,
    /// This relay's own address, for example `wss://relay.example.org`. NIP-42 authentication checks the client signed for
    /// it, so another relay cannot pass a challenge on. Required when `auth_required` or `auth_dms` is on.
    pub relay_url: String,
    /// Confirm an event (and hand it to subscribers) only after it is safely on disk. Batched, so it stays fast.
    pub durable: bool,
    /// Keep events on disk at all. Off: memory only, gone at restart.
    pub persist: bool,
    /// NIP-42: send direct messages (kinds 4 and 1059) only to the authenticated sender and recipient.
    pub auth_dms: bool,
    /// The administrator's Nostr public key (hex), shown in the information document. It may also use the management API.
    pub admin_pubkey: String,
    /// More public keys (hex) allowed to use the NIP-86 management API, signed with NIP-98. With `admin_pubkey` empty too, the
    /// API is off. It also needs `[admin] enabled = true`: what it changes is saved with the panel's settings.
    pub admin_pubkeys: Vec<String>,
    /// Leading zero bits an event id needs (NIP-13). 0: none.
    pub min_pow: u32,
    pub auth_required: bool,
    pub created_at_upper_secs: i64,
    /// 0: any age.
    pub created_at_lower_secs: i64,
    /// Events older than this are dropped (replaceable ones stay). 0: keep.
    pub retention_days: u64,
    pub max_events: usize,
    /// Hex public keys. If any are listed, only they may publish.
    pub allowed_pubkeys: Vec<String>,
    pub blocked_pubkeys: Vec<String>,
    pub allowed_kinds: Vec<u64>,
    pub blocked_kinds: Vec<u64>,
}

impl Default for NostrConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            store_events: true,
            name: "PrivCord server".into(),
            description: "A relay run by its administrator.".into(),
            icon: String::new(),
            max_event_bytes: 131_072,
            max_message_bytes: 262_144,
            max_tags: 2_000,
            max_subs_per_conn: 32,
            max_filters: 16,
            max_limit: 500,
            max_subid_len: 256,
            events_per_minute_per_ip: 600,
            events_per_second_global: 0,
            reqs_per_minute_per_conn: 600,
            max_filter_values: 256,
            max_store_bytes: 512 * 1024 * 1024,
            relay_url: String::new(),
            durable: false,
            persist: true,
            auth_dms: false,
            admin_pubkey: String::new(),
            admin_pubkeys: Vec::new(),
            min_pow: 0,
            auth_required: false,
            created_at_upper_secs: 900,
            created_at_lower_secs: 0,
            retention_days: 30,
            max_events: 1_000_000,
            allowed_pubkeys: Vec::new(),
            blocked_pubkeys: Vec::new(),
            allowed_kinds: Vec::new(),
            blocked_kinds: Vec::new(),
        }
    }
}

/// TLS: `wss://` and `https://` for the relay and the admin panel, and TURN over TLS. Certificates come from files (read
/// again when they change) or, for trying things out, a self-signed one that apps will not trust.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsConfig {
    pub cert_file: String,
    pub key_file: String,
    /// Make a certificate for testing. Apps and browsers will refuse it; use real files in production.
    pub self_signed: bool,
    /// The name people use to reach this server. In TURN-over-TLS addresses and in the self-signed certificate.
    pub domain: String,
    /// Serve the relay (and admin panel) over TLS here, for example `0.0.0.0:443`. The plain listener stays as it is.
    pub https_listen: String,
}

/// The admin panel: a web page and API for managing this server, on its own listener so it can sit on localhost or a
/// private network (a VPN such as Tailscale) and never on the public internet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdminConfig {
    pub enabled: bool,
    pub listen: String,
    /// Serve it over TLS (needs `[tls]`). Without it, keep `listen` on localhost or a private network: admin tokens travel
    /// in the clear over plain HTTP.
    pub tls: bool,
    /// Allow the panel over plain HTTP on an address other than this machine (a VPN or a private network you trust).
    /// Off: the server refuses to start that way, because admin tokens would cross the network unencrypted.
    pub allow_plain_http: bool,
    /// Days an admin token works before it must be replaced (from the panel, or with a new setup code). 0: never expires.
    pub token_days: u32,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "127.0.0.1:8788".into(),
            tls: false,
            allow_plain_http: false,
            token_days: 90,
        }
    }
}

/// How the server's logs are written to disk (see `record.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// A check on every record, so damage in the middle of a log is noticed (and reported) instead of silently skipped or,
    /// worse, believed. Off writes plain lines as the earlier versions did, which the Node relay can also read.
    pub checksums: bool,
    /// Encrypt every record (AES-256-GCM) with a key kept outside `data_dir`. The key is 32 random bytes as 64 hex
    /// characters: make one with `privcord-server store-key --out FILE`. Lose the key and the data is gone.
    pub encrypt: bool,
    /// A file holding the key (for example a systemd credential or a file on another volume).
    pub key_file: String,
    /// Or an environment variable holding it, when no `key_file` is set.
    pub key_env: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            checksums: true,
            encrypt: false,
            key_file: String::new(),
            key_env: "PRIVCORD_STORE_KEY".into(),
        }
    }
}
