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
    routing::{delete, get},
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

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Stored {
    #[serde(default)]
    admins: Vec<Admin>,
    #[serde(default)]
    access_tokens: Vec<AccessToken>,
    /// The Nostr rules last saved from the panel; they win over the settings file.
    #[serde(default)]
    policy: Option<PolicyJson>,
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
    /// Opens (or starts) the admin accounts in `dir`. When there is no admin yet one owner is made, and its token returned:
    /// that is the only time it can be seen.
    pub fn open(
        dir: &std::path::Path,
        app: Arc<App>,
        tls_self_signed: bool,
    ) -> Result<(Arc<Self>, Option<String>), String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
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
            tls_self_signed,
        });
        let first = {
            let mut g = lock(&state.inner);
            if g.0.admins.is_empty() {
                let (secret, hash) = new_secret("pca_");
                g.0.admins.push(Admin {
                    id: new_id(),
                    name: "owner".into(),
                    role: Role::Owner,
                    hash,
                    created: now_ms() / 1000,
                    last_used: 0,
                });
                state.save(&mut g)?;
                Some(secret)
            } else {
                None
            }
        };
        state.apply();
        Ok((state, first))
    }

    /// Makes another admin from the command line (works while the server runs: it notices the file changed).
    pub fn add_admin_offline(
        dir: &std::path::Path,
        name: &str,
        role: Role,
    ) -> Result<String, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let path = dir.join("admin.json");
        let mut stored: Stored = match std::fs::read_to_string(&path) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(_) => Stored::default(),
        };
        let (secret, hash) = new_secret("pca_");
        stored.admins.push(Admin {
            id: new_id(),
            name: name.to_owned(),
            role,
            hash,
            created: now_ms() / 1000,
            last_used: 0,
        });
        write_atomic(&path, &stored)?;
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
        // (last_used is kept in memory only: writing the file on every request would be wasteful)
        admin.last_used = now;
        Ok(admin.clone())
    }
}

fn write_atomic(path: &std::path::Path, stored: &Stored) -> Result<(), String> {
    let text = serde_json::to_string_pretty(stored).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("could not save {}: {e}", path.display()))
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

async fn me(
    State(a): State<AdminApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let me = admin_or_return!(a, headers, addr, Need::Read);
    Json(json!({ "name": me.name, "role": me.role })).into_response()
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
    let mut warnings: Vec<String> = Vec::new();
    if !app.requires_token() && c.relay.enabled {
        warnings.push("Anyone who finds this server can use it: no access tokens are set. Add one under Access to make it private.".into());
    }
    if c.admin
        .listen
        .parse::<SocketAddr>()
        .is_ok_and(|s| !s.ip().is_loopback())
        && !c.admin.tls
    {
        warnings.push("This panel is served over plain HTTP on a non-local address: tokens travel unencrypted. Use localhost, a VPN, or turn on admin.tls.".into());
    }
    if a.state.tls_self_signed {
        warnings.push("The TLS certificate is self-signed: apps and browsers will not trust it. Use real certificate files for production.".into());
    }
    if c.turn.enabled && c.turn.tcp_listen.is_empty() && c.turn.tls_listen.is_empty() {
        warnings.push("TURN accepts UDP only: phones on networks that block UDP cannot use it. Set turn.tcp_listen and turn.tls_listen.".into());
    }
    if c.nostr.enabled && c.nostr.allowed_pubkeys.is_empty() && !app.requires_token() {
        warnings.push("The Nostr side accepts events from anyone. Set allowed authors under Nostr rules if this relay is for your own use.".into());
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
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; \
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
        .route("/admin/api/me", get(me))
        .route("/admin/api/overview", get(overview))
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
