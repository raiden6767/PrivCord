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
    routing::get,
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
    /// SHA-256 of each accepted token (empty: open to anyone).
    tokens: Vec<[u8; 32]>,
    next_conn: AtomicU64,
    turn_asks: Mutex<HashMap<String, (u32, u64)>>,
}

impl App {
    pub fn new(
        cfg: Config,
        relay: Option<Arc<Relay>>,
        turn: Option<(Vec<u8>, Vec<String>)>,
    ) -> Self {
        let tokens = cfg
            .server
            .access_tokens
            .iter()
            .map(|t| Sha256::digest(t.as_bytes()).into())
            .collect();
        Self {
            cfg,
            relay,
            turn,
            tokens,
            next_conn: AtomicU64::new(1),
            turn_asks: Mutex::new(HashMap::new()),
        }
    }

    fn allowed(&self, headers: &HeaderMap, query: &HashMap<String, String>) -> bool {
        if self.tokens.is_empty() {
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
        self.tokens.iter().fold(false, |ok, t| ok | ct_eq(t, &hash))
    }
}

fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b.iter()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

pub fn client_ip(addr: SocketAddr, headers: &HeaderMap, trust_proxy: bool) -> String {
    if trust_proxy {
        if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            // The rightmost entry is the one our proxy appended; earlier ones are whatever the visitor claimed.
            if let Some(last) = xff
                .rsplit(',')
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return last.to_owned();
            }
        }
    }
    addr.ip().to_string()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/.well-known/privcord-server", get(about))
        .route("/turn", get(turn_credentials))
        .fallback(socket)
        .with_state(app)
}

async fn healthz(State(app): State<Arc<App>>, headers: HeaderMap) -> Json<Value> {
    // Asked through a proxy: a minimal answer, and no counters.
    if headers.contains_key("x-forwarded-for") {
        return Json(json!({ "ok": true }));
    }
    Json(
        app.relay
            .as_ref()
            .map_or_else(|| json!({ "ok": true }), |r| r.health()),
    )
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
        "requires_token": !app.tokens.is_empty(),
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
    let ip = client_ip(addr, &headers, app.cfg.server.trust_proxy);
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

/// Anything else: a WebSocket for the relay (the apps connect to the root path, with or without a query).
async fn socket(
    State(app): State<Arc<App>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    let (Some(_), Ok(ws)) = (&app.relay, ws) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !app.allowed(&headers, &q) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // The address is used in memory for limits only; it is never logged or stored.
    let ip = client_ip(addr, &headers, app.cfg.server.trust_proxy);
    ws.max_message_size(MAX_BLOB + 1024)
        .read_buffer_size(4 * 1024)
        .write_buffer_size(0)
        .max_write_buffer_size(256 * 1024)
        .max_frame_size(MAX_BLOB + 1024)
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
    };
    let writer = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                Out::Text(t) => {
                    let n = t.len();
                    let sent = sink.send(Message::Text(t.into())).await;
                    pending.fetch_sub(n.min(pending.load(Ordering::Relaxed)), Ordering::Relaxed);
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
    let mut alive = true;
    let mut beat = tokio::time::interval(HEARTBEAT);
    beat.tick().await;
    loop {
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
                relay.frame(&conn, &mut session, &ip, &text, now_ms());
            }
            _ = beat.tick() => {
                // Nothing heard since the last beat (not even a pong): the other end is gone.
                if !alive { break }
                alive = false;
                let _ = tx.send(Out::Ping);
            }
        }
    }
    relay.leave(&ip, conn.id, &session);
    writer.abort();
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("App")
    }
}
