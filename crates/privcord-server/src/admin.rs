//! The admin panel: who may manage this server, and what they can change while it runs.
//!
//! Admins sign in with a token (shown once when it is made, kept only as a hash). Three roles: an **owner** can do
//! everything including adding and removing admins; a **moderator** can change the Nostr rules, the access tokens and remove
//! events; a **viewer** can only look. Everything changed here is saved in `data_dir/admin.json` and applied at once. The
//! panel has its own listener (see `[admin]`), meant for localhost or a private network.

use crate::net::{now_ms, App};
use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Moderator,
    Viewer,
}

impl Role {
    fn can_change(self) -> bool {
        matches!(self, Self::Owner | Self::Moderator)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Admin {
    pub id: String,
    pub name: String,
    pub role: Role,
    hash: String,
    pub created: i64,
    #[serde(default)]
    pub last_used: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccessToken {
    pub id: String,
    pub name: String,
    hash: String,
    pub created: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct PolicyJson {
    #[serde(default)]
    allowed_pubkeys: Vec<String>,
    #[serde(default)]
    blocked_pubkeys: Vec<String>,
    #[serde(default)]
    allowed_kinds: Vec<u64>,
    #[serde(default)]
    blocked_kinds: Vec<u64>,
}

/// Limits changed from the panel. Only what was changed is here; the rest follow the settings file.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct SettingsJson {
    #[serde(default)]
    keep_offline_messages: Option<bool>,
    #[serde(default)]
    frames_per_minute: Option<u64>,
    #[serde(default)]
    connections_per_address: Option<u64>,
    #[serde(default)]
    mailboxes_per_connection: Option<u64>,
    #[serde(default)]
    nostr_events_per_minute: Option<u64>,
    #[serde(default)]
    nostr_requests_per_minute: Option<u64>,
    #[serde(default)]
    nostr_keep_days: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Stored {
    #[serde(default)]
    settings: Option<SettingsJson>,
    #[serde(default)]
    admins: Vec<Admin>,
    #[serde(default)]
    access_tokens: Vec<AccessToken>,
    /// The Nostr rules last saved from the panel; they win over the settings file.
    #[serde(default)]
    policy: Option<PolicyJson>,
    /// One-time setup codes made with `admin-invite`: hashes only, each for one named admin, until it expires.
    #[serde(default)]
    invites: Vec<Invite>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Invite {
    hash: String,
    name: String,
    role: Role,
    /// Seconds since 1970.
    expires: i64,
}

fn sha_hex(s: &str) -> String {
    crate::nostr::event::hex(&Sha256::digest(s.as_bytes()))
}

/// A fresh secret: `prefix` and 43 URL-safe characters of randomness. Returns the secret and its hash.
fn new_secret(prefix: &str) -> (String, String) {
    use base64::Engine;
    let mut raw = [0u8; 32];
    let _ = getrandom::fill(&mut raw);
    let secret = format!(
        "{prefix}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
    );
    let hash = sha_hex(&secret);
    (secret, hash)
}

/// How long a setup code works, and what it is made of: 20 letters and digits without look-alikes (about 99 bits).
const SETUP_CODE_SECS: i64 = 15 * 60;
const SETUP_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// A one-time setup code like `abcd-efgh-jkmn-pqrs-tuvw` and its hash.
fn new_setup_code() -> (String, String) {
    // 248 = 8 x 31: bytes at or above it are skipped so every letter is equally likely.
    let limit = 248u8;
    let mut letters: Vec<char> = Vec::with_capacity(20);
    while letters.len() < 20 {
        let mut raw = [0u8; 32];
        let _ = getrandom::fill(&mut raw);
        for b in raw {
            if b < limit && letters.len() < 20 {
                if let Some(c) = SETUP_ALPHABET.get(usize::from(b) % SETUP_ALPHABET.len()) {
                    letters.push(char::from(*c));
                }
            }
        }
    }
    let code = letters
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-");
    let hash = sha_hex(&normalize_code(&code));
    (code, hash)
}

fn normalize_code(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn new_id() -> String {
    let mut b = [0u8; 6];
    let _ = getrandom::fill(&mut b);
    crate::nostr::event::hex(&b)
}

fn ct_eq_hex(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

pub struct AdminState {
    path: PathBuf,
    inner: Mutex<(Stored, Option<SystemTime>)>,
    app: Arc<App>,
    fails: Mutex<HashMap<IpAddr, (u32, i64)>>,
    /// The hash of the setup code that lets the first owner in, and when it stops working. Memory only: never written to disk.
    setup: Mutex<Option<(String, i64)>>,
    tls_self_signed: bool,
}

impl std::fmt::Debug for AdminState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AdminState")
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn mtime(p: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl AdminState {
    /// Opens (or starts) the admin accounts in `dir`. When there is no admin yet, a one-time setup code is returned (see
    /// `claim`); no token exists until it is used.
    pub fn open(
        dir: &std::path::Path,
        app: Arc<App>,
        tls_self_signed: bool,
    ) -> Result<(Arc<Self>, Option<String>), String> {
        crate::fsutil::create_private_dir(dir)?;
        let path = dir.join("admin.json");
        let stored: Stored = match std::fs::read_to_string(&path) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(_) => Stored::default(),
        };
        let state = Arc::new(Self {
            inner: Mutex::new((stored, mtime(&path))),
            path,
            app,
            fails: Mutex::new(HashMap::new()),
            setup: Mutex::new(None),
            tls_self_signed,
        });
        // No admin yet: nobody is made and no token exists. A setup code that works once, for 15 minutes, lets the person
        // who runs the server claim ownership from the panel (the token is then shown once, in their browser).
        let first = if !state.has_valid_owner() {
            let (code, hash) = new_setup_code();
            *lock(&state.setup) = Some((hash, now_ms() / 1000 + SETUP_CODE_SECS));
            Some(code)
        } else {
            None
        };
        state.apply();
        Ok((state, first))
    }

    /// Makes a one-time setup code for another admin from the command line (works while the server runs: it notices the
    /// file changed). The person enters it on the panel's login page and gets their own token there: no long-lived secret
    /// is printed. Only the code's hash is kept.
    pub fn add_admin_invite_offline(
        dir: &std::path::Path,
        name: &str,
        role: Role,
    ) -> Result<String, String> {
        crate::fsutil::create_private_dir(dir)?;
        let path = dir.join("admin.json");
        let mut stored: Stored = match std::fs::read_to_string(&path) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(_) => Stored::default(),
        };
        let now = now_ms() / 1000;
        stored.invites.retain(|i| i.expires > now);
        let (code, hash) = new_setup_code();
        stored.invites.push(Invite {
            hash,
            name: name.chars().take(60).collect(),
            role,
            expires: now + SETUP_CODE_SECS,
        });
        write_atomic(&path, &stored)?;
        Ok(code)
    }

    /// Whether this admin's token has run out (`admin.token_days`; 0 means it never does).
    fn expired(&self, a: &Admin, now: i64) -> bool {
        let days = i64::from(self.app.cfg.admin.token_days);
        days > 0 && a.created + days * 86_400 <= now
    }

    /// An owner whose token still works. With none, a setup code is printed at start so the panel cannot be locked out.
    fn has_valid_owner(&self) -> bool {
        let now = now_ms() / 1000;
        lock(&self.inner)
            .0
            .admins
            .iter()
            .any(|a| a.role == Role::Owner && !self.expired(a, now))
    }

    /// Whether a setup code is waiting to be used (no admin yet and the code has not expired).
    fn setup_open(&self) -> bool {
        self.reload_if_changed();
        let now = now_ms() / 1000;
        let open = !self.has_valid_owner()
            && lock(&self.setup)
                .as_ref()
                .is_some_and(|(_, until)| *until > now);
        if !open {
            *lock(&self.setup) = None;
        }
        open
    }

    /// A setup code (printed at start, or made with `admin-invite`) makes an admin and returns their token, once. Wrong codes
    /// count like wrong tokens.
    #[allow(clippy::result_large_err)]
    fn claim(&self, ip: IpAddr, code: &str) -> Result<String, Response> {
        let now = now_ms() / 1000;
        {
            let mut f = lock(&self.fails);
            f.retain(|_, (_, until)| *until > now);
            if f.get(&ip).is_some_and(|(n, _)| *n >= 10) {
                return Err(text(
                    StatusCode::TOO_MANY_REQUESTS,
                    "too many wrong codes; wait a minute",
                ));
            }
        }
        let hash = sha_hex(&normalize_code(code));
        let wrong = || {
            lock(&self.fails).entry(ip).or_insert((0, now + 60)).0 += 1;
            text(
                StatusCode::UNAUTHORIZED,
                "that setup code is not right, or it has expired or been used",
            )
        };
        // The code printed at start (when there is no owner), or a one-time code made with `admin-invite`.
        let first_run = self.setup_open()
            && lock(&self.setup)
                .as_ref()
                .is_some_and(|(h, _)| ct_eq_hex(h, &hash));
        self.reload_if_changed();
        let (secret, token_hash) = new_secret("pca_");
        let mut g = lock(&self.inner);
        let (name, role) = if first_run {
            ("owner".to_owned(), Role::Owner)
        } else {
            g.0.invites.retain(|i| i.expires > now);
            let Some(i) = g.0.invites.iter().position(|i| ct_eq_hex(&i.hash, &hash)) else {
                drop(g);
                return Err(wrong());
            };
            let inv = g.0.invites.remove(i);
            (inv.name, inv.role)
        };
        // Tokens that have run out are dropped when a new one is made, so the list stays honest.
        g.0.admins.retain(|a| !self.expired(a, now));
        g.0.admins.push(Admin {
            id: new_id(),
            name,
            role,
            hash: token_hash,
            created: now,
            last_used: 0,
        });
        if let Err(e) = self.save(&mut g) {
            return Err(text(StatusCode::INTERNAL_SERVER_ERROR, &e));
        }
        drop(g);
        if first_run {
            *lock(&self.setup) = None; // one use
        }
        Ok(secret)
    }

    /// Replaces the signed-in admin's own token (the old one stops working at once) and starts a new lifetime.
    fn rotate(&self, id: &str) -> Result<String, String> {
        let (secret, hash) = new_secret("pca_");
        let mut g = lock(&self.inner);
        let now = now_ms() / 1000;
        let a =
            g.0.admins
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or("no such admin")?;
        a.hash = hash;
        a.created = now;
        self.save(&mut g)?;
        Ok(secret)
    }

    fn save(&self, g: &mut MutexGuard<'_, (Stored, Option<SystemTime>)>) -> Result<(), String> {
        write_atomic(&self.path, &g.0)?;
        g.1 = mtime(&self.path);
        Ok(())
    }

    /// Pushes what is saved into the running server: the access tokens and the Nostr rules.
    fn apply(&self) {
        let g = lock(&self.inner);
        let extra: Vec<[u8; 32]> =
            g.0.access_tokens
                .iter()
                .filter_map(|t| crate::nostr::event::unhex::<32>(&t.hash))
                .collect();
        self.app.set_extra_tokens(&extra);
        if let (Some(p), Some(n)) = (&g.0.policy, self.app.relay.as_ref().and_then(|r| r.nostr())) {
            n.set_policy(policy_from_json(p));
        }
        if let (Some(s), Some(r)) = (&g.0.settings, self.app.relay.as_ref()) {
            use std::sync::atomic::Ordering::Relaxed;
            let l = r.limits();
            if let Some(v) = s.keep_offline_messages {
                l.keep_offline.store(v, Relaxed);
            }
            if let Some(v) = s.frames_per_minute {
                l.frames_per_minute.store(v, Relaxed);
            }
            if let Some(v) = s.connections_per_address {
                l.connections_per_address
                    .store(usize::try_from(v).unwrap_or(usize::MAX), Relaxed);
            }
            if let Some(v) = s.mailboxes_per_connection {
                l.mailboxes_per_connection
                    .store(usize::try_from(v).unwrap_or(usize::MAX), Relaxed);
            }
            if let Some(n) = r.nostr() {
                let nl = n.limits();
                if let Some(v) = s.nostr_events_per_minute {
                    nl.events_per_minute.store(v, Relaxed);
                }
                if let Some(v) = s.nostr_requests_per_minute {
                    nl.requests_per_minute.store(v, Relaxed);
                }
                if let Some(v) = s.nostr_keep_days {
                    nl.keep_days.store(v, Relaxed);
                }
            }
        }
    }

    fn reload_if_changed(&self) {
        let now = mtime(&self.path);
        let changed = lock(&self.inner).1 != now;
        if changed {
            if let Some(stored) = std::fs::read_to_string(&self.path)
                .ok()
                .and_then(|t| serde_json::from_str::<Stored>(&t).ok())
            {
                *lock(&self.inner) = (stored, now);
                self.apply();
            }
        }
    }

    #[allow(clippy::result_large_err)] // the error is the HTTP answer itself, built once and returned
    /// Who is calling. Wrong tokens are counted per address and, after too many, refused for a while.
    fn authenticate(&self, headers: &HeaderMap, ip: IpAddr) -> Result<Admin, Response> {
        let now = now_ms() / 1000;
        {
            let mut f = lock(&self.fails);
            f.retain(|_, (_, until)| *until > now);
            if f.get(&ip).is_some_and(|(n, _)| *n >= 10) {
                return Err(text(
                    StatusCode::TOO_MANY_REQUESTS,
                    "too many wrong tokens; wait a minute",
                ));
            }
        }
        self.reload_if_changed();
        let presented = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        let hash = sha_hex(presented);
        let mut g = lock(&self.inner);
        // Every admin is compared, without stopping at the first match.
        let mut found: Option<usize> = None;
        for (i, a) in g.0.admins.iter().enumerate() {
            if ct_eq_hex(&a.hash, &hash) {
                found = Some(i);
            }
        }
        let Some(i) = found else {
            drop(g);
            let mut f = lock(&self.fails);
            let e = f.entry(ip).or_insert((0, now + 60));
            e.0 += 1;
            return Err(text(
                StatusCode::UNAUTHORIZED,
                "sign in with an admin token",
            ));
        };
        let Some(admin) = g.0.admins.get_mut(i) else {
            return Err(text(
                StatusCode::UNAUTHORIZED,
                "sign in with an admin token",
            ));
        };
        if self.expired(admin, now) {
            return Err(text(
                StatusCode::UNAUTHORIZED,
                "that token has expired: make a new one with a setup code",
            ));
        }
        // (last_used is kept in memory only: writing the file on every request would be wasteful)
        admin.last_used = now;
        Ok(admin.clone())
    }
}

fn write_atomic(path: &std::path::Path, stored: &Stored) -> Result<(), String> {
    let text = serde_json::to_string_pretty(stored).map_err(|e| e.to_string())?;
    crate::fsutil::write_private_atomic(path, text.as_bytes())
}

fn policy_from_json(p: &PolicyJson) -> crate::nostr::Policy {
    let keys = |v: &[String]| {
        v.iter()
            .filter_map(|s| crate::nostr::event::unhex::<32>(s))
            .collect()
    };
    crate::nostr::Policy {
        allowed_pubkeys: keys(&p.allowed_pubkeys),
        blocked_pubkeys: keys(&p.blocked_pubkeys),
        allowed_kinds: p.allowed_kinds.iter().copied().collect(),
        blocked_kinds: p.blocked_kinds.iter().copied().collect(),
    }
}

fn text(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "error": msg }))).into_response()
}

#[derive(Clone, Debug)]
pub struct AdminApp {
    pub state: Arc<AdminState>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    Read,
    Change,
    Owner,
}

#[allow(clippy::result_large_err)] // the error is the HTTP answer itself
fn guard(
    a: &AdminApp,
    headers: &HeaderMap,
    addr: SocketAddr,
    need: Need,
) -> Result<Admin, Response> {
    let admin = a.state.authenticate(headers, addr.ip())?;
    let ok = match need {
        Need::Read => true,
        Need::Change => admin.role.can_change(),
        Need::Owner => admin.role == Role::Owner,
    };
    if ok {
        Ok(admin)
    } else {
        Err(text(StatusCode::FORBIDDEN, "your role cannot do that"))
    }
}

macro_rules! admin_or_return {
    ($a:expr, $h:expr, $addr:expr, $need:expr) => {
        match guard(&$a, &$h, $addr, $need) {
            Ok(x) => x,
            Err(r) => return r,
        }
    };
}

fn public_admin(a: &Admin) -> Value {
    json!({ "id": a.id, "name": a.name, "role": a.role, "created": a.created, "last_used": a.last_used })
}

async fn setup_status(State(a): State<AdminApp>) -> Response {
    Json(json!({ "open": a.state.setup_open() })).into_response()
}

async fn setup_claim(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Response {
    let code = body.get("code").and_then(Value::as_str).unwrap_or("");
    match a.state.claim(addr.ip(), code) {
        Ok(token) => Json(json!({ "token": token })).into_response(),
        Err(r) => r,
    }
}

async fn me(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let me = admin_or_return!(a, headers, addr, Need::Read);
    let days = i64::from(a.state.app.cfg.admin.token_days);
    let left = (days > 0).then(|| ((me.created + days * 86_400 - now_ms() / 1000) / 86_400).max(0));
    Json(json!({ "name": me.name, "role": me.role, "days_left": left })).into_response()
}

/// The signed-in admin replaces their own token: the old one stops working at once, the new one is shown once.
async fn rotate_me(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let me = admin_or_return!(a, headers, addr, Need::Read);
    match a.state.rotate(&me.id) {
        Ok(token) => Json(json!({ "token": token })).into_response(),
        Err(e) => text(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn overview(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Read);
    let app = &a.state.app;
    let c = &app.cfg;
    let host = if !c.tls.domain.is_empty() {
        c.tls.domain.clone()
    } else if !matches!(c.turn.public_ip.trim(), "" | "auto") {
        c.turn.public_ip.clone()
    } else {
        "YOUR-SERVER".to_owned()
    };
    let port_of = |listen: &str| listen.rsplit(':').next().unwrap_or("").to_owned();
    let https = !c.tls.https_listen.is_empty();
    let relay_url = if https {
        let p = port_of(&c.tls.https_listen);
        if p == "443" {
            format!("wss://{host}")
        } else {
            format!("wss://{host}:{p}")
        }
    } else {
        format!("ws://{host}:{}", port_of(&c.server.listen))
    };
    let from_panel = lock(&a.state.inner).0.access_tokens.len();
    let mut warnings: Vec<Value> = Vec::new();
    if !app.requires_token() && c.relay.enabled {
        warnings.push(json!({ "text": "Anyone who finds this server can use it: no access tokens are set. Make a token under People to make it private.", "go": "people" }));
    }
    if c.admin
        .listen
        .parse::<SocketAddr>()
        .is_ok_and(|s| !s.ip().is_loopback())
        && !c.admin.tls
    {
        warnings.push(json!({ "text": "This panel is served over plain HTTP on a non-local address: tokens travel unencrypted. Use localhost, a VPN, or turn on admin.tls.", "go": null }));
    }
    if a.state.tls_self_signed {
        warnings.push(json!({ "text": "The TLS certificate is self-signed: apps and browsers will not trust it. Use real certificate files for production.", "go": null }));
    }
    if c.turn.enabled && c.turn.tcp_listen.is_empty() && c.turn.tls_listen.is_empty() {
        warnings.push(json!({ "text": "TURN accepts UDP only: phones on networks that block UDP cannot use it. Set turn.tcp_listen and turn.tls_listen.", "go": null }));
    }
    if c.nostr.enabled && c.nostr.allowed_pubkeys.is_empty() && !app.requires_token() {
        warnings.push(json!({ "text": "The Nostr side accepts events from anyone. Set allowed authors under Rules if this relay is for your own use.", "go": "rules" }));
    }
    let mut counters = app.relay.as_ref().map_or(json!({}), |r| r.health());
    if let Some(o) = counters.as_object_mut() {
        o.remove("ok");
    }
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": app.started.elapsed().as_secs(),
        "roles": { "relay": c.relay.enabled, "nostr": c.nostr.enabled, "turn": app.turn.is_some() },
        "access": { "private": app.requires_token(), "from_settings_file": c.server.access_tokens.len(), "from_panel": from_panel },
        "addresses": {
            "relay": relay_url,
            "nostr": c.nostr.enabled.then(|| relay_url.clone()),
            "turn": app.turn.as_ref().map(|(_, urls)| urls),
            // The TURN port also answers STUN (no login), so the same server is a STUN server for the apps' address lookup.
            "stun": app.turn.as_ref().map(|(_, urls)| {
                urls.iter()
                    .filter(|u| u.starts_with("turn:") && !u.contains('?'))
                    .map(|u| u.replacen("turn:", "stun:", 1))
                    .collect::<Vec<_>>()
            }),
        },
        "tls": { "https": https, "self_signed": a.state.tls_self_signed },
        "limits": {
            "max_connections_per_address": c.relay.max_conns_per_ip, "messages_kept_at_most": c.relay.max_queued,
            "store_and_forward": c.relay.store_and_forward, "nostr_durable": c.nostr.durable,
        },
        "counters": counters,
        "warnings": warnings,
    }))
    .into_response()
}

fn nostr_of(a: &AdminApp) -> Option<&Arc<crate::nostr::Nostr>> {
    a.state.app.relay.as_ref().and_then(|r| r.nostr())
}

async fn get_policy(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Read);
    let Some(n) = nostr_of(&a) else {
        return text(StatusCode::NOT_FOUND, "the Nostr role is off");
    };
    let p = n.policy();
    let hexes = |s: &std::collections::HashSet<[u8; 32]>| {
        let mut v: Vec<String> = s.iter().map(|k| crate::nostr::event::hex(k)).collect();
        v.sort();
        v
    };
    let kinds = |s: &std::collections::HashSet<u64>| {
        let mut v: Vec<u64> = s.iter().copied().collect();
        v.sort_unstable();
        v
    };
    Json(json!({
        "allowed_pubkeys": hexes(&p.allowed_pubkeys), "blocked_pubkeys": hexes(&p.blocked_pubkeys),
        "allowed_kinds": kinds(&p.allowed_kinds), "blocked_kinds": kinds(&p.blocked_kinds),
    }))
    .into_response()
}

async fn put_policy(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Change);
    let Some(n) = nostr_of(&a) else {
        return text(StatusCode::NOT_FOUND, "the Nostr role is off");
    };
    let keys = |k: &str| -> Result<Vec<String>, String> {
        let list = body
            .get(k)
            .and_then(Value::as_array)
            .ok_or(format!("{k} must be a list"))?;
        let mut out = Vec::new();
        for v in list.iter().take(10_000) {
            let s = v
                .as_str()
                .ok_or(format!("{k} holds something that is not text"))?
                .trim()
                .to_lowercase();
            if crate::nostr::event::unhex::<32>(&s).is_none() {
                return Err(format!("\"{s}\" is not a 64-character hex public key"));
            }
            out.push(s);
        }
        Ok(out)
    };
    let kinds = |k: &str| -> Result<Vec<u64>, String> {
        let list = body
            .get(k)
            .and_then(Value::as_array)
            .ok_or(format!("{k} must be a list"))?;
        list.iter()
            .take(10_000)
            .map(|v| {
                v.as_u64().filter(|n| *n <= 65_535).ok_or(format!(
                    "{k} holds a kind that is not a number from 0 to 65535"
                ))
            })
            .collect()
    };
    let parsed = (|| -> Result<PolicyJson, String> {
        Ok(PolicyJson {
            allowed_pubkeys: keys("allowed_pubkeys")?,
            blocked_pubkeys: keys("blocked_pubkeys")?,
            allowed_kinds: kinds("allowed_kinds")?,
            blocked_kinds: kinds("blocked_kinds")?,
        })
    })();
    let p = match parsed {
        Ok(p) => p,
        Err(e) => return text(StatusCode::BAD_REQUEST, &e),
    };
    n.set_policy(policy_from_json(&p));
    let mut g = lock(&a.state.inner);
    g.0.policy = Some(p);
    if let Err(e) = a.state.save(&mut g) {
        return text(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    Json(json!({ "ok": true })).into_response()
}

fn settings_view(a: &AdminApp) -> Value {
    use std::sync::atomic::Ordering::Relaxed;
    let app = &a.state.app;
    let relay = app.relay.as_ref();
    let nostr = relay.and_then(|r| r.nostr());
    json!({
        "relay": relay.map(|r| json!({
            "keep_offline_messages": r.limits().keep_offline.load(Relaxed) && r.store_and_forward(),
            "can_keep_offline_messages": r.store_and_forward(),
            "frames_per_minute": r.limits().frames_per_minute.load(Relaxed),
            "connections_per_address": r.limits().connections_per_address.load(Relaxed),
            "mailboxes_per_connection": r.limits().mailboxes_per_connection.load(Relaxed),
            "stored_messages": r.queued(),
        })),
        "nostr": nostr.map(|n| json!({
            "events_per_minute": n.limits().events_per_minute.load(Relaxed),
            "requests_per_minute": n.limits().requests_per_minute.load(Relaxed),
            "keep_days": n.limits().keep_days.load(Relaxed),
            "stored_events": n.store().map_or(0, crate::nostr::store::Store::len),
        })),
    })
}

async fn get_settings(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Read);
    Json(settings_view(&a)).into_response()
}

async fn put_settings(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Change);
    // Each value is checked against a sensible range; only what is present is changed.
    let num = |k: &str, lo: u64, hi: u64| -> Result<Option<u64>, String> {
        match body.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .filter(|n| (lo..=hi).contains(n))
                .map(Some)
                .ok_or(format!("{k} must be a whole number from {lo} to {hi}")),
        }
    };
    let parsed = (|| -> Result<SettingsJson, String> {
        Ok(SettingsJson {
            keep_offline_messages: match body.get("keep_offline_messages") {
                None | Some(Value::Null) => None,
                Some(v) => Some(
                    v.as_bool()
                        .ok_or("keep_offline_messages must be true or false")?,
                ),
            },
            frames_per_minute: num("frames_per_minute", 10, 1_000_000)?,
            connections_per_address: num("connections_per_address", 1, 10_000)?,
            mailboxes_per_connection: num("mailboxes_per_connection", 1, 1_000)?,
            nostr_events_per_minute: num("nostr_events_per_minute", 0, 100_000)?,
            nostr_requests_per_minute: num("nostr_requests_per_minute", 0, 100_000)?,
            nostr_keep_days: num("nostr_keep_days", 0, 3_650)?,
        })
    })();
    let new = match parsed {
        Ok(n) => n,
        Err(e) => return text(StatusCode::BAD_REQUEST, &e),
    };
    {
        let mut g = lock(&a.state.inner);
        let cur = g.0.settings.get_or_insert_with(SettingsJson::default);
        macro_rules! take {
            ($f:ident) => {
                if new.$f.is_some() {
                    cur.$f = new.$f;
                }
            };
        }
        take!(keep_offline_messages);
        take!(frames_per_minute);
        take!(connections_per_address);
        take!(mailboxes_per_connection);
        take!(nostr_events_per_minute);
        take!(nostr_requests_per_minute);
        take!(nostr_keep_days);
        if let Err(e) = a.state.save(&mut g) {
            return text(StatusCode::INTERNAL_SERVER_ERROR, &e);
        }
    }
    a.state.apply();
    Json(settings_view(&a)).into_response()
}

/// Removing data cannot be undone, so the request has to say it is meant.
fn confirmed(body: &Value) -> bool {
    body.get("confirm").and_then(Value::as_bool) == Some(true)
}

async fn clear_messages(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Change);
    if !confirmed(&body) {
        return text(
            StatusCode::BAD_REQUEST,
            "send {\"confirm\": true} to remove every stored message",
        );
    }
    let Some(r) = a.state.app.relay.as_ref() else {
        return text(StatusCode::NOT_FOUND, "the relay is off");
    };
    Json(json!({ "removed": r.clear_stored(now_ms()) })).into_response()
}

async fn clear_nostr(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Change);
    if !confirmed(&body) {
        return text(
            StatusCode::BAD_REQUEST,
            "send {\"confirm\": true} to remove every stored Nostr event",
        );
    }
    let Some(n) = nostr_of(&a) else {
        return text(StatusCode::NOT_FOUND, "the Nostr role is off");
    };
    Json(json!({ "removed": n.clear_events() })).into_response()
}

async fn ban_event(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Change);
    let Some(n) = nostr_of(&a) else {
        return text(StatusCode::NOT_FOUND, "the Nostr role is off");
    };
    match n.ban_event(id.trim()) {
        None => text(StatusCode::BAD_REQUEST, "an event id is 64 hex characters"),
        Some(was_there) => Json(json!({ "ok": true, "removed": was_there })).into_response(),
    }
}

async fn list_tokens(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Read);
    let g = lock(&a.state.inner);
    let list: Vec<Value> =
        g.0.access_tokens
            .iter()
            .map(|t| json!({ "id": t.id, "name": t.name, "created": t.created }))
            .collect();
    Json(
        json!({ "tokens": list, "from_settings_file": a.state.app.cfg.server.access_tokens.len() }),
    )
    .into_response()
}

fn clean_name(v: Option<&Value>) -> Result<String, String> {
    let s = v.and_then(Value::as_str).unwrap_or("").trim().to_owned();
    if s.is_empty() || s.chars().count() > 60 || s.chars().any(char::is_control) {
        return Err("give it a name of 1 to 60 characters".into());
    }
    Ok(s)
}

async fn add_token(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Change);
    let name = match clean_name(body.get("name")) {
        Ok(n) => n,
        Err(e) => return text(StatusCode::BAD_REQUEST, &e),
    };
    let (secret, hash) = new_secret("pcs_");
    let mut g = lock(&a.state.inner);
    if g.0.access_tokens.len() >= 500 {
        return text(
            StatusCode::BAD_REQUEST,
            "that is plenty of tokens; remove some first",
        );
    }
    let id = new_id();
    g.0.access_tokens.push(AccessToken {
        id: id.clone(),
        name,
        hash,
        created: now_ms() / 1000,
    });
    if let Err(e) = a.state.save(&mut g) {
        return text(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    drop(g);
    a.state.apply();
    Json(json!({ "id": id, "token": secret, "note": "this is the only time the token is shown" }))
        .into_response()
}

async fn del_token(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Change);
    let mut g = lock(&a.state.inner);
    // Removing the last token opens the server to everyone, so that has to be said out loud.
    if g.0.access_tokens.len() == 1
        && g.0.access_tokens.iter().any(|t| t.id == id)
        && a.state.app.cfg.server.access_tokens.is_empty()
        && q.get("open").map(String::as_str) != Some("true")
    {
        return text(
            StatusCode::CONFLICT,
            "this is the last token: removing it opens the server to anyone (add open=true to confirm)",
        );
    }
    let before = g.0.access_tokens.len();
    g.0.access_tokens.retain(|t| t.id != id);
    if g.0.access_tokens.len() == before {
        return text(StatusCode::NOT_FOUND, "no such token");
    }
    if let Err(e) = a.state.save(&mut g) {
        return text(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    drop(g);
    a.state.apply();
    Json(json!({ "ok": true })).into_response()
}

async fn list_admins(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Owner);
    let g = lock(&a.state.inner);
    Json(json!({ "admins": g.0.admins.iter().map(public_admin).collect::<Vec<_>>() }))
        .into_response()
}

async fn add_admin(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Owner);
    let name = match clean_name(body.get("name")) {
        Ok(n) => n,
        Err(e) => return text(StatusCode::BAD_REQUEST, &e),
    };
    let role = match body.get("role").and_then(Value::as_str) {
        Some("owner") => Role::Owner,
        Some("moderator") => Role::Moderator,
        Some("viewer") => Role::Viewer,
        _ => {
            return text(
                StatusCode::BAD_REQUEST,
                "role must be owner, moderator or viewer",
            )
        }
    };
    let (secret, hash) = new_secret("pca_");
    let mut g = lock(&a.state.inner);
    if g.0.admins.len() >= 100 {
        return text(StatusCode::BAD_REQUEST, "that is plenty of admins");
    }
    let id = new_id();
    g.0.admins.push(Admin {
        id: id.clone(),
        name,
        role,
        hash,
        created: now_ms() / 1000,
        last_used: 0,
    });
    if let Err(e) = a.state.save(&mut g) {
        return text(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    Json(json!({ "id": id, "token": secret, "note": "this is the only time the token is shown" }))
        .into_response()
}

async fn del_admin(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let _ = admin_or_return!(a, headers, addr, Need::Owner);
    let mut g = lock(&a.state.inner);
    let Some(target) = g.0.admins.iter().find(|x| x.id == id).cloned() else {
        return text(StatusCode::NOT_FOUND, "no such admin");
    };
    let owners = g.0.admins.iter().filter(|x| x.role == Role::Owner).count();
    if target.role == Role::Owner && owners <= 1 {
        return text(StatusCode::CONFLICT, "there must always be one owner");
    }
    g.0.admins.retain(|x| x.id != id);
    if let Err(e) = a.state.save(&mut g) {
        return text(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    Json(json!({ "ok": true })).into_response()
}

// ---- the page itself: fixed files, no external resources ----

const INDEX: &str = include_str!("admin/index.html");
const CSS: &str = include_str!("admin/app.css");
const JS: &str = include_str!("admin/app.js");
const THEME_JS: &str = include_str!("admin/theme.js");
const BRAND: &str = include_str!("admin/brand.svg");
const FONT_DISPLAY: &[u8] = include_bytes!("admin/fonts/bricolage.woff2");
const FONT_TEXT: &[u8] = include_bytes!("admin/fonts/figtree.ttf");

fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

/// Headers for everything the panel serves: nothing cached, nothing framed, nothing loaded from elsewhere.
async fn secure_headers(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut r = next.run(req).await;
    let h = r.headers_mut();
    for (k, v) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        ("strict-transport-security", "max-age=31536000"),
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; connect-src 'self'; img-src 'self' data:; \
             base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
    ] {
        if let Ok(v) = header::HeaderValue::from_str(v) {
            h.insert(header::HeaderName::from_static(k), v);
        }
    }
    r
}

pub fn router(state: Arc<AdminState>) -> Router {
    let app = AdminApp { state };
    Router::new()
        .route(
            "/",
            get(|| async { axum::response::Redirect::to("/admin") }),
        )
        .route(
            "/admin",
            get(|| async { asset("text/html; charset=utf-8", INDEX) }),
        )
        .route(
            "/admin/app.css",
            get(|| async { asset("text/css; charset=utf-8", CSS) }),
        )
        .route(
            "/admin/app.js",
            get(|| async { asset("text/javascript; charset=utf-8", JS) }),
        )
        .route(
            "/admin/theme.js",
            get(|| async { asset("text/javascript; charset=utf-8", THEME_JS) }),
        )
        .route(
            "/admin/brand.svg",
            get(|| async { asset("image/svg+xml", BRAND) }),
        )
        .route(
            "/admin/fonts/bricolage.woff2",
            get(|| async {
                ([(header::CONTENT_TYPE, "font/woff2")], FONT_DISPLAY).into_response()
            }),
        )
        .route(
            "/admin/fonts/figtree.ttf",
            get(|| async { ([(header::CONTENT_TYPE, "font/ttf")], FONT_TEXT).into_response() }),
        )
        .route("/admin/api/setup", get(setup_status).post(setup_claim))
        .route("/admin/api/me", get(me))
        .route("/admin/api/me/token", post(rotate_me))
        .route("/admin/api/overview", get(overview))
        .route("/admin/api/settings", get(get_settings).put(put_settings))
        .route("/admin/api/clear/messages", post(clear_messages))
        .route("/admin/api/clear/nostr", post(clear_nostr))
        .route("/admin/api/nostr/policy", get(get_policy).put(put_policy))
        .route("/admin/api/nostr/events/{id}", delete(ban_event))
        .route("/admin/api/access-tokens", get(list_tokens).post(add_token))
        .route("/admin/api/access-tokens/{id}", delete(del_token))
        .route("/admin/api/admins", get(list_admins).post(add_admin))
        .route("/admin/api/admins/{id}", delete(del_admin))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(axum::middleware::from_fn(secure_headers))
        .with_state(app)
}
