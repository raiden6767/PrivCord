//! The web side: the relay's WebSocket, `/healthz`, `/.well-known/privcord-server` (what this server offers) and `/turn`
//! (short-lived TURN credentials).

use crate::config::Config;
use crate::relay::{Conn, Out, Relay, Session, MAX_BLOB};
use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const HEARTBEAT: Duration = Duration::from_secs(30);

pub struct App {
    pub cfg: Config,
    pub relay: Option<Arc<Relay>>,
    /// `Some((secret, urls))` when the TURN role is on.
    pub turn: Option<(Vec<u8>, Vec<String>)>,
    /// SHA-256 of each accepted token (empty: open to anyone): the ones in the settings file plus those the admin panel made.
    tokens: std::sync::RwLock<Vec<[u8; 32]>>,
    base_tokens: Vec<[u8; 32]>,
    pub trusted_proxies: Vec<std::net::IpAddr>,
    pub started: std::time::Instant,
    next_conn: AtomicU64,
    turn_asks: Mutex<HashMap<String, (u32, u64)>>,
    mgmt_asks: Mutex<HashMap<String, (u32, u64)>>,
    /// Addresses refused by the management API (`blockip`): they cannot open the relay's socket.
    blocked_ips: std::sync::RwLock<std::collections::HashSet<std::net::IpAddr>>,
    /// The admin state, once it exists: the management API changes what the panel saves.
    pub admin: std::sync::OnceLock<std::sync::Weak<crate::admin::AdminState>>,
    /// Traffic counted this month (and its cap).
    pub usage: Arc<crate::limits::Usage>,
    /// Set when the panel asks for a restart with new choices (see `supervise`).
    pub restart: tokio::sync::Notify,
}

impl App {
    pub fn new(
        cfg: Config,
        relay: Option<Arc<Relay>>,
        turn: Option<(Vec<u8>, Vec<String>)>,
        usage: Arc<crate::limits::Usage>,
    ) -> Self {
        let cfg_trusted: Vec<std::net::IpAddr> = cfg
            .server
            .trusted_proxies
            .iter()
            .filter_map(|a| a.trim().parse().ok())
            .collect();
        let tokens: Vec<[u8; 32]> = cfg
            .server
            .access_tokens
            .iter()
            .map(|t| Sha256::digest(t.as_bytes()).into())
            .collect();
        Self {
            cfg,
            relay,
            turn,
            tokens: std::sync::RwLock::new(tokens.clone()),
            base_tokens: tokens,
            trusted_proxies: cfg_trusted,
            started: std::time::Instant::now(),
            next_conn: AtomicU64::new(1),
            turn_asks: Mutex::new(HashMap::new()),
            mgmt_asks: Mutex::new(HashMap::new()),
            blocked_ips: std::sync::RwLock::new(std::collections::HashSet::new()),
            admin: std::sync::OnceLock::new(),
            usage,
            restart: tokio::sync::Notify::new(),
        }
    }

    /// Replaces the tokens made in the admin panel (the ones from the settings file always stay).
    pub fn set_extra_tokens(&self, extra: &[[u8; 32]]) {
        let mut all = self.base_tokens.clone();
        all.extend_from_slice(extra);
        *self
            .tokens
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = all;
    }

    /// Replaces the blocked addresses.
    pub fn set_blocked_ips(&self, ips: std::collections::HashSet<std::net::IpAddr>) {
        *self
            .blocked_ips
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ips;
    }

    pub fn ip_blocked(&self, ip: std::net::IpAddr) -> bool {
        self.blocked_ips
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&ip)
    }

    /// Whether clients must present a token.
    pub fn requires_token(&self) -> bool {
        !self
            .tokens
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    fn allowed(&self, headers: &HeaderMap, query: &HashMap<String, String>) -> bool {
        let tokens = self
            .tokens
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if tokens.is_empty() {
            return true;
        }
        let bearer = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        let presented = bearer
            .or_else(|| query.get("token").map(String::as_str))
            .unwrap_or("");
        let hash: [u8; 32] = Sha256::digest(presented.as_bytes()).into();
        // Compared as hashes, over every accepted token, without stopping early.
        tokens.iter().fold(false, |ok, t| ok | ct_eq(t, &hash))
    }
}

fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b.iter()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// The address limits are counted against. An IPv6 visitor controls a whole /64, so that is what counts as one visitor
/// (otherwise one machine could present billions of "different" addresses).
pub fn ip_group(ip: std::net::IpAddr) -> std::net::IpAddr {
    match ip {
        std::net::IpAddr::V4(_) => ip,
        std::net::IpAddr::V6(v6) => {
            let mut o = v6.octets();
            for b in o.iter_mut().skip(8) {
                *b = 0;
            }
            std::net::IpAddr::V6(std::net::Ipv6Addr::from(o))
        }
    }
}

pub fn client_ip(
    addr: SocketAddr,
    headers: &HeaderMap,
    trust_proxy: bool,
    header: &str,
    trusted_proxies: &[std::net::IpAddr],
) -> String {
    let believe =
        trust_proxy && (trusted_proxies.is_empty() || trusted_proxies.contains(&addr.ip()));
    if believe {
        // A proxy may add a header line of its own instead of extending the visitor's: the LAST value is the one our
        // proxy wrote; earlier ones are whatever the visitor claimed.
        let last = headers
            .get_all(header)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .next_back()
            .and_then(|v| v.rsplit(',').next())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(last) = last {
            return match last.parse::<std::net::IpAddr>() {
                Ok(ip) => ip_group(ip).to_string(),
                // Not an address: still one key, but never an unbounded string.
                Err(_) => last.chars().take(64).collect(),
            };
        }
    }
    ip_group(addr.ip()).to_string()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

/// What an app (a desktop window is a web page) needs to ask this server about itself from another origin: `GET` with an
/// `Authorization` header. `/turn` still needs the access token (when there is one); nothing is opened that was closed.
async fn cors(mut r: Response) -> Response {
    let h = r.headers_mut();
    for (k, v) in [
        ("access-control-allow-origin", "*"),
        (
            "access-control-allow-headers",
            "authorization, content-type",
        ),
        ("access-control-allow-methods", "GET, OPTIONS"),
        ("access-control-max-age", "600"),
    ] {
        h.insert(
            axum::http::HeaderName::from_static(k),
            axum::http::HeaderValue::from_static(v),
        );
    }
    r
}

async fn preflight() -> StatusCode {
    StatusCode::NO_CONTENT
}

pub fn router(app: Arc<App>) -> Router {
    let open = Router::new()
        .route(
            "/.well-known/privcord-server",
            get(about).options(preflight),
        )
        .route("/turn", get(turn_credentials).options(preflight))
        .layer(axum::middleware::map_response(cors));
    Router::new()
        .route("/healthz", get(healthz))
        .merge(open)
        .route("/metrics", get(metrics))
        // NIP-86: management calls are POSTs to the relay's own address; a GET there is still the relay's socket.
        .route(
            "/",
            post(management)
                .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
                .fallback(socket),
        )
        .fallback(socket)
        .with_state(app)
}

async fn healthz(State(app): State<Arc<App>>, headers: HeaderMap) -> Json<Value> {
    // Asked through a proxy: a minimal answer, and no counters.
    if headers.contains_key(app.cfg.server.real_ip_header.as_str())
        || headers.contains_key("x-forwarded-for")
    {
        return Json(json!({ "ok": true }));
    }
    Json(
        app.relay
            .as_ref()
            .map_or_else(|| json!({ "ok": true }), |r| r.health()),
    )
}

/// Prometheus-style counters, when the administrator turned them on.
async fn metrics(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !app.cfg.server.metrics {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !app.allowed(&headers, &q) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    fn flatten(prefix: &str, v: &Value, out: &mut String) {
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    flatten(&format!("{prefix}_{k}"), x, out);
                }
            }
            Value::Number(n) => {
                out.push_str(&format!("{prefix} {n}\n"));
            }
            _ => {}
        }
    }
    let mut text = String::new();
    if let Some(r) = &app.relay {
        flatten("privcord", &r.health(), &mut text);
    }
    (
        [
            ("content-type", "text/plain; version=0.0.4"),
            ("cache-control", "no-store"),
        ],
        text,
    )
        .into_response()
}

async fn about(State(app): State<Arc<App>>) -> Json<Value> {
    let c = &app.cfg;
    let mut roles = Vec::new();
    if app.relay.is_some() {
        roles.push("relay");
    }
    if app.turn.is_some() {
        roles.push("turn");
    }
    Json(json!({
        "name": "privcord-server",
        "version": env!("CARGO_PKG_VERSION"),
        "roles": roles,
        "admin": c.server.admin_contact,
        "requires_token": app.requires_token(),
        "relay": app.relay.as_ref().map(|r| json!({
            "store_and_forward": r.store_and_forward(),
            "max_blob_bytes": MAX_BLOB,
            "max_message_ttl_secs": crate::relay::MAX_MSG_TTL,
        })),
        "turn": app.turn.as_ref().map(|(_, urls)| json!({ "urls": urls, "credentials": "/turn", "ttl_secs": c.turn.ttl_secs })),
        "note": "This server is run by its administrator, who is responsible for it. It carries sealed messages and cannot read them.",
    }))
}

async fn turn_credentials(
    State(app): State<Arc<App>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some((secret, urls)) = &app.turn else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !app.allowed(&headers, &q) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let ip = client_ip(
        addr,
        &headers,
        app.cfg.server.trust_proxy,
        &app.cfg.server.real_ip_header,
        &app.trusted_proxies,
    );
    let now = now_ms().max(0) as u64 / 1000;
    {
        // A few asks a minute from one address: credentials are cheap, but not free.
        let mut asks = app
            .turn_asks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        asks.retain(|_, (_, reset)| *reset > now);
        let e = asks.entry(ip).or_insert((0, now + 60));
        e.0 += 1;
        if e.0 > 20 {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
    }
    let ttl = app.cfg.turn.ttl_secs;
    let (username, credential) = crate::turn_role::issue(secret, now, ttl);
    (
        [("cache-control", "no-store")],
        Json(json!({ "urls": urls, "username": username, "credential": credential, "ttl": ttl })),
    )
        .into_response()
}

/// NIP-86 (relay management), authenticated with NIP-98: `POST /` with `Content-Type: application/nostr+json+rpc`, the body
/// `{"method": ..., "params": [...]}` and `Authorization: Nostr <signed event>` from one of the configured administrator keys.
async fn management(
    State(app): State<Arc<App>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let reply = |code: StatusCode, v: Value| {
        (
            code,
            [("content-type", "application/nostr+json+rpc")],
            Json(v),
        )
            .into_response()
    };
    let rpc = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.starts_with("application/nostr+json+rpc"));
    let Some(n) = app.relay.as_ref().and_then(|r| r.nostr()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let keys = n.admin_keys();
    if !rpc || keys.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let now = now_ms().max(0) as u64 / 1000;
    {
        // A few calls a minute from one address: checking a signature is cheap, but not free.
        let ip = client_ip(
            addr,
            &headers,
            app.cfg.server.trust_proxy,
            &app.cfg.server.real_ip_header,
            &app.trusted_proxies,
        );
        let mut asks = app
            .mgmt_asks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        asks.retain(|_, (_, reset)| *reset > now);
        let e = asks.entry(ip).or_insert((0, now + 60));
        e.0 += 1;
        if e.0 > 60 {
            return reply(
                StatusCode::TOO_MANY_REQUESTS,
                json!({ "error": "too many calls; wait a minute" }),
            );
        }
    }
    let mut hosts: Vec<String> = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .into_iter()
        .collect();
    hosts.extend(crate::nostr::nip98::host_and_path(&app.cfg.nostr.relay_url).map(|(h, _)| h));
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    match crate::nostr::nip98::verify(
        auth,
        "POST",
        &hosts,
        path,
        &body,
        i64::try_from(now).unwrap_or(0),
    ) {
        Err(why) => return reply(StatusCode::UNAUTHORIZED, json!({ "error": why })),
        Ok(pk) if !keys.contains(&pk) => {
            return reply(
                StatusCode::UNAUTHORIZED,
                json!({ "error": "that key is not an administrator of this relay" }),
            )
        }
        Ok(_) => {}
    }
    let Some(admin) = app.admin.get().and_then(std::sync::Weak::upgrade) else {
        return reply(
            StatusCode::OK,
            json!({ "error": "turn the admin panel on ([admin] enabled = true): it keeps what the management API changes" }),
        );
    };
    // A small, typed request: a method name and a list of parameters, nothing else is read.
    #[derive(serde::Deserialize)]
    struct Call {
        method: String,
        #[serde(default)]
        params: Vec<Value>,
    }
    let call = match std::str::from_utf8(&body)
        .ok()
        .filter(|t| t.len() <= 64 * 1024)
        .and_then(|t| serde_json::from_str::<Call>(t).ok())
    {
        Some(c) if c.method.len() <= 64 && c.params.len() <= 64 => c,
        _ => {
            return reply(
                StatusCode::OK,
                json!({ "error": "the body must be JSON: {\"method\": ..., \"params\": [...]}" }),
            )
        }
    };
    let (method, params) = (call.method.as_str(), call.params);
    match admin.nip86(method, &params) {
        Ok(result) => reply(StatusCode::OK, json!({ "result": result })),
        Err(e) => reply(StatusCode::OK, json!({ "error": e })),
    }
}

/// Anything else: a WebSocket for the relay (the apps connect to the root path, with or without a query).
async fn socket(
    State(app): State<Arc<App>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    // NIP-11: a plain request that asks for the relay information document.
    if let (Some(n), Err(_)) = (app.relay.as_ref().and_then(|r| r.nostr()), &ws) {
        let wants = headers
            .get("accept")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("application/nostr+json"));
        if wants {
            return (
                [
                    ("content-type", "application/nostr+json"),
                    ("access-control-allow-origin", "*"),
                    ("access-control-allow-headers", "*"),
                    ("access-control-allow-methods", "GET, OPTIONS"),
                ],
                Json(n.info(&app.cfg.server.admin_contact)),
            )
                .into_response();
        }
    }
    let (Some(_), Ok(ws)) = (&app.relay, ws) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !app.allowed(&headers, &q) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // The address is used in memory for limits only; it is never logged or stored.
    let ip = client_ip(
        addr,
        &headers,
        app.cfg.server.trust_proxy,
        &app.cfg.server.real_ip_header,
        &app.trusted_proxies,
    );
    // An address an administrator blocked (the management API): the peer's own, and the one a proxy reported.
    if app.ip_blocked(addr.ip()) || ip.parse().is_ok_and(|a| app.ip_blocked(a)) {
        return StatusCode::FORBIDDEN.into_response();
    }
    // At the monthly traffic cap: not now. A long Retry-After keeps clients from retrying in a loop.
    if app.usage.over_cap() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [("retry-after", crate::limits::RETRY_AFTER_SECS.to_string())],
            "this server has used its traffic allowance for the month",
        )
            .into_response();
    }
    let biggest = if app.cfg.nostr.enabled {
        app.cfg.nostr.max_message_bytes
    } else {
        MAX_BLOB + 1024
    };
    ws.max_message_size(biggest)
        .read_buffer_size(4 * 1024)
        .max_write_buffer_size(256 * 1024)
        .max_frame_size(biggest)
        .on_upgrade(move |socket| serve(socket, app, ip))
}

async fn serve(mut socket: WebSocket, app: Arc<App>, ip: String) {
    let Some(relay) = app.relay.clone() else {
        return;
    };
    if !relay.admit(&ip) {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: 1013,
                reason: "too many connections".into(),
            })))
            .await;
        return;
    }
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Out>();
    let pending = Arc::new(AtomicUsize::new(0));
    let conn = Conn {
        id: app.next_conn.fetch_add(1, Ordering::Relaxed),
        tx: tx.clone(),
        pending: pending.clone(),
        total: relay.out_total().clone(),
        limit: relay.out_limit().0,
    };
    let total = relay.out_total().clone();
    let writer_pending = pending.clone();
    // When the last write to this client completed: a client that takes nothing for a long time is cut off.
    let progress = Arc::new(AtomicU64::new(now_ms().unsigned_abs()));
    let writer_progress = progress.clone();
    let usage_w = app.usage.clone();
    let mut pace_out = crate::limits::Pace::new(app.cfg.limits.connection_kbps);
    let writer = tokio::spawn(async move {
        let pending = writer_pending;
        let progress = writer_progress;
        while let Some(out) = rx.recv().await {
            match out {
                Out::Text(t) => {
                    let n = t.len();
                    let sent = sink.send(Message::Text(t.as_ref().into())).await;
                    usage_w.web(0, n);
                    if let Some(p) = pace_out.as_mut() {
                        p.wait(n).await;
                    }
                    progress.store(now_ms().unsigned_abs(), Ordering::Relaxed);
                    pending.fetch_sub(n.min(pending.load(Ordering::Relaxed)), Ordering::Relaxed);
                    total.fetch_sub(n.min(total.load(Ordering::Relaxed)), Ordering::Relaxed);
                    if sent.is_err() {
                        break;
                    }
                }
                Out::Ping => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
                Out::Close(code, reason) => {
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code,
                            reason: reason.into(),
                        })))
                        .await;
                    break;
                }
            }
        }
    });
    let mut session = Session::default();
    relay.connected(&conn, &mut session);
    let mut alive = true;
    let mut beat = tokio::time::interval(HEARTBEAT);
    beat.tick().await;
    let soft = relay.out_limit().1;
    let mut pace_in = crate::limits::Pace::new(app.cfg.limits.connection_kbps);
    let stall_ms = relay.stall_ms();
    let mut reaper = tokio::time::interval(Duration::from_millis((stall_ms / 3).clamp(250, 5000)));
    reaper.tick().await;
    loop {
        // A flood is slowed at the door: while too much is waiting to be written, this socket is not read, so the
        // sender's own connection fills up and it slows down, instead of the relay growing.
        // (For a quarter of a second at most per frame: a client that never reads is cut off by the stall check below, so
        // it cannot keep everyone else slowed down.)
        let mut waited = 0;
        while relay.out_total().load(Ordering::Relaxed) > soft && waited < 250 {
            tokio::time::sleep(Duration::from_millis(1)).await;
            waited += 1;
        }
        tokio::select! {
            incoming = stream.next() => {
                let Some(Ok(msg)) = incoming else { break };
                alive = true;
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                    Message::Close(_) => break,
                    _ => continue,
                };
                app.usage.web(text.len(), 0);
                if let Some(p) = pace_in.as_mut() {
                    p.wait(text.len()).await;
                }
                relay.frame(&conn, &mut session, &ip, &text, now_ms());
            }
            _ = reaper.tick() => {
                if stall_ms > 0
                    && pending.load(Ordering::Relaxed) > 0
                    && now_ms().unsigned_abs().saturating_sub(progress.load(Ordering::Relaxed)) > stall_ms
                {
                    break;
                }
            }
            _ = beat.tick() => {
                // Nothing heard since the last beat (not even a pong): the other end is gone.
                if !alive { break }
                alive = false;
                let _ = tx.send(Out::Ping);
            }
        }
    }
    relay.leave(&ip, &conn, &mut session);
    writer.abort();
    // Whatever was still queued for this connection will never be written.
    let left = pending.load(Ordering::Relaxed);
    relay.out_total().fetch_sub(
        left.min(relay.out_total().load(Ordering::Relaxed)),
        Ordering::Relaxed,
    );
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("App")
    }
}
