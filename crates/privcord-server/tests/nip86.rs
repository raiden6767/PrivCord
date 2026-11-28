//! The NIP-86 management API, authenticated with NIP-98, against the real server.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use futures_util::{SinkExt, StreamExt};
use k256::schnorr::SigningKey;
use privcord_server::nostr::event::hex;
use privcord_server::{config::Config, start, Running};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message};

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn key() -> SigningKey {
    loop {
        let mut b = [0u8; 32];
        getrandom::fill(&mut b).unwrap();
        if let Ok(k) = SigningKey::from_bytes(&k256::FieldBytes::from(b)) {
            return k;
        }
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn pk(k: &SigningKey) -> String {
    hex(&k.verifying_key().to_bytes())
}

fn make(k: &SigningKey, kind: u64, created_at: i64, tags: Vec<Vec<&str>>, content: &str) -> Value {
    let pubkey = pk(k);
    let id: [u8; 32] = Sha256::digest(
        json!([0, pubkey, created_at, kind, tags, content])
            .to_string()
            .as_bytes(),
    )
    .into();
    let mut aux = [0u8; 32];
    getrandom::fill(&mut aux).unwrap();
    let sig = k.sign_raw(&id, &aux).unwrap();
    json!({ "id": hex(&id), "pubkey": pubkey, "created_at": created_at, "kind": kind, "tags": tags, "content": content, "sig": hex(&sig.to_bytes()) })
}

struct C(Ws);

impl C {
    async fn open(url: &str) -> Self {
        Self(connect_async(url).await.unwrap().0)
    }
    async fn send(&mut self, v: Value) {
        self.0
            .send(Message::Text(v.to_string().into()))
            .await
            .unwrap();
    }
    async fn next(&mut self) -> Value {
        loop {
            let m = tokio::time::timeout(Duration::from_secs(5), self.0.next())
                .await
                .expect("timed out")
                .expect("closed")
                .unwrap();
            if let Message::Text(t) = m {
                return serde_json::from_str(&t).unwrap();
            }
        }
    }
    async fn quiet(&mut self) -> bool {
        tokio::time::timeout(Duration::from_millis(300), self.0.next())
            .await
            .is_err()
    }
    /// Publishes and returns the relay's verdict: (accepted, message).
    async fn publish(&mut self, ev: &Value) -> (bool, String) {
        self.send(json!(["EVENT", ev])).await;
        loop {
            let m = self.next().await;
            if m[0] == "OK" && m[1] == ev["id"] {
                return (m[2].as_bool().unwrap(), m[3].as_str().unwrap().to_owned());
            }
        }
    }
    /// Subscribes, and returns the stored events that come before EOSE.
    async fn req(&mut self, sub: &str, filters: Vec<Value>) -> Vec<Value> {
        let mut m = vec![json!("REQ"), json!(sub)];
        m.extend(filters);
        self.send(Value::Array(m)).await;
        let mut out = Vec::new();
        loop {
            let m = self.next().await;
            match m[0].as_str() {
                Some("EVENT") if m[1] == sub => out.push(m[2].clone()),
                Some("EOSE") if m[1] == sub => return out,
                Some("CLOSED") if m[1] == sub => panic!("closed: {}", m[2]),
                _ => {}
            }
        }
    }
}

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Rig {
    r: Running,
    admin: SigningKey,
    _dir: tempfile::TempDir,
    dir: std::path::PathBuf,
}

fn config(dir: &std::path::Path, admin: &SigningKey) -> Config {
    let mut c = Config::default();
    c.server.listen = "127.0.0.1:0".into();
    c.server.data_dir = dir.into();
    c.nostr.enabled = true;
    c.nostr.events_per_minute_per_ip = 0;
    c.nostr.admin_pubkeys = vec![pk(admin)];
    c.relay.rate_limit = 1_000_000;
    c.admin.enabled = true;
    c.admin.listen = "127.0.0.1:0".into();
    c
}

async fn rig() -> Rig {
    let (admin, d) = (key(), tempfile::tempdir().unwrap());
    let dir = d.path().to_path_buf();
    let r = start(config(&dir, &admin)).await.unwrap();
    Rig {
        r,
        admin,
        _dir: d,
        dir,
    }
}

/// A POST to the relay's address; the answer's status and body.
async fn post(
    addr: std::net::SocketAddr,
    body: &str,
    content_type: &str,
    auth: Option<&Value>,
) -> (u16, Value) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut req = format!(
        "POST / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(ev) = auth {
        use base64::Engine;
        req.push_str(&format!(
            "Authorization: Nostr {}\r\n",
            base64::engine::general_purpose::STANDARD.encode(ev.to_string())
        ));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, b) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (
        status,
        serde_json::from_str(b.trim()).unwrap_or(Value::Null),
    )
}

/// The NIP-98 event for a POST of `body` to `url`, signed by `k` at time `at`.
fn auth(k: &SigningKey, url: &str, body: &str, at: i64) -> Value {
    let payload = hex(&Sha256::digest(body.as_bytes()));
    make(
        k,
        27235,
        at,
        vec![
            vec!["u", url],
            vec!["method", "POST"],
            vec!["payload", &payload],
        ],
        "",
    )
}

/// One management call, signed by the administrator.
async fn call(rig: &Rig, method: &str, params: Value) -> Value {
    let body = json!({ "method": method, "params": params }).to_string();
    let url = format!("http://{}/", rig.r.addr);
    let (status, v) = post(
        rig.r.addr,
        &body,
        "application/nostr+json+rpc",
        Some(&auth(&rig.admin, &url, &body, now())),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    v
}

async fn publish_status(url: &str, e: &Value) -> (bool, String) {
    C::open(url).await.publish(e).await
}

#[tokio::test]
async fn the_api_needs_a_signed_request_from_an_administrator_and_nothing_else_will_do() {
    let g = rig().await;
    let body = json!({ "method": "supportedmethods", "params": [] }).to_string();
    let url = format!("http://{}/", g.r.addr);
    let ct = "application/nostr+json+rpc";
    // no Authorization
    assert_eq!(post(g.r.addr, &body, ct, None).await.0, 401);
    // signed by someone who is not an administrator
    assert_eq!(
        post(g.r.addr, &body, ct, Some(&auth(&key(), &url, &body, now())))
            .await
            .0,
        401
    );
    // signed for another body (the payload tag), another url, or a minute and more ago
    assert_eq!(
        post(
            g.r.addr,
            &body,
            ct,
            Some(&auth(&g.admin, &url, "{}", now()))
        )
        .await
        .0,
        401
    );
    assert_eq!(
        post(
            g.r.addr,
            &body,
            ct,
            Some(&auth(&g.admin, "http://other.example/", &body, now()))
        )
        .await
        .0,
        401
    );
    assert_eq!(
        post(
            g.r.addr,
            &body,
            ct,
            Some(&auth(&g.admin, &url, &body, now() - 120))
        )
        .await
        .0,
        401
    );
    // a request that is not a management call at all is not even answered
    assert_eq!(
        post(
            g.r.addr,
            &body,
            "application/json",
            Some(&auth(&g.admin, &url, &body, now()))
        )
        .await
        .0,
        404
    );
    // the administrator's own, in time, for this body and url: it works
    let ok = call(&g, "supportedmethods", json!([])).await;
    assert!(
        ok["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m == "banpubkey"),
        "{ok}"
    );
    // the relay's socket still works on the same address
    assert!(C::open(&format!("ws://{}", g.r.addr)).await.quiet().await);
    g.r.stop().await;
}

#[tokio::test]
async fn the_api_is_off_without_administrator_keys() {
    let d = tempfile::tempdir().unwrap();
    let mut c = config(d.path(), &key());
    c.nostr.admin_pubkeys.clear();
    let r = start(c).await.unwrap();
    let body = json!({ "method": "supportedmethods", "params": [] }).to_string();
    let url = format!("http://{}/", r.addr);
    assert_eq!(
        post(
            r.addr,
            &body,
            "application/nostr+json+rpc",
            Some(&auth(&key(), &url, &body, now()))
        )
        .await
        .0,
        404
    );
    r.stop().await;
}

#[tokio::test]
async fn banning_and_allowing_keys_events_and_kinds_works_at_once_and_survives_a_restart() {
    let g = rig().await;
    let url = format!("ws://{}", g.r.addr);
    let (spammer, friend) = (key(), key());
    // a banned key cannot publish, and the list says why
    assert_eq!(
        call(&g, "banpubkey", json!([pk(&spammer), "spam"])).await["result"],
        true
    );
    let e = make(&spammer, 1, now(), vec![], "buy now");
    assert!(publish_status(&url, &e).await.1.starts_with("blocked"));
    let listed = call(&g, "listbannedpubkeys", json!([])).await;
    assert_eq!(listed["result"][0]["pubkey"], pk(&spammer).as_str());
    assert_eq!(listed["result"][0]["reason"], "spam");
    // an event can be banned (and is refused when published) and taken back
    let ok = make(&friend, 1, now(), vec![], "hello");
    assert!(publish_status(&url, &ok).await.0);
    let id = ok["id"].as_str().unwrap().to_owned();
    assert_eq!(
        call(&g, "banevent", json!([id, "rude"])).await["result"],
        true
    );
    let mut c = C::open(&url).await;
    assert!(
        c.req("s", vec![json!({ "ids": [id] })]).await.is_empty(),
        "the event is gone"
    );
    assert!(publish_status(&url, &ok).await.1.starts_with("blocked"));
    assert_eq!(
        call(&g, "listbannedevents", json!([])).await["result"][0]["reason"],
        "rude"
    );
    // the key ban survives a restart
    g.r.stop().await;
    let r2 = start(config(&g.dir, &g.admin)).await.unwrap();
    let url2 = format!("ws://{}", r2.addr);
    assert!(
        publish_status(&url2, &e).await.1.starts_with("blocked"),
        "still banned after a restart"
    );
    assert!(
        publish_status(&url2, &ok).await.1.starts_with("blocked"),
        "the event ban too"
    );
    let g2 = Rig {
        r: r2,
        admin: g.admin.clone(),
        _dir: tempfile::tempdir().unwrap(),
        dir: g.dir.clone(),
    };
    // and both can be taken back
    assert_eq!(
        call(&g2, "allowevent", json!([ok["id"]])).await["result"],
        true
    );
    assert!(publish_status(&url2, &ok).await.0, "allowed again");
    assert_eq!(
        call(&g2, "allowpubkey", json!([pk(&spammer)])).await["result"],
        true
    );
    assert!(
        publish_status(&url2, &e).await.0,
        "the key is allowed (and now on the allow list)"
    );
    // an allow list of kinds
    assert_eq!(call(&g2, "allowkind", json!([7])).await["result"], true);
    assert_eq!(
        call(&g2, "listallowedkinds", json!([])).await["result"],
        json!([7])
    );
    assert!(
        publish_status(&url2, &make(&spammer, 1, now(), vec![], "a note"))
            .await
            .1
            .starts_with("restricted")
    );
    assert!(
        publish_status(&url2, &make(&spammer, 7, now(), vec![], "a reaction"))
            .await
            .0
    );
    g2.r.stop().await;
}

#[tokio::test]
async fn the_relays_name_changes_and_an_address_can_be_blocked_and_unblocked() {
    let g = rig().await;
    assert_eq!(
        call(&g, "changerelayname", json!(["Our relay"])).await["result"],
        true
    );
    assert_eq!(
        call(&g, "changerelaydescription", json!(["for us"])).await["result"],
        true
    );
    assert_eq!(
        call(&g, "changerelayicon", json!(["https://example.com/i.png"])).await["result"],
        true
    );
    let info = g.r.relay().unwrap().nostr().unwrap().info("");
    assert_eq!(
        (
            info["name"].as_str(),
            info["description"].as_str(),
            info["icon"].as_str()
        ),
        (
            Some("Our relay"),
            Some("for us"),
            Some("https://example.com/i.png")
        )
    );
    // a blocked address cannot open the relay's socket; it can still make management calls (that is how it is unblocked)
    assert_eq!(
        call(&g, "blockip", json!(["127.0.0.1", "testing"])).await["result"],
        true
    );
    assert!(
        connect_async(format!("ws://{}", g.r.addr)).await.is_err(),
        "the blocked address is refused"
    );
    assert_eq!(
        call(&g, "listblockedips", json!([])).await["result"][0]["reason"],
        "testing"
    );
    assert_eq!(
        call(&g, "unblockip", json!(["127.0.0.1"])).await["result"],
        true
    );
    assert!(
        connect_async(format!("ws://{}", g.r.addr)).await.is_ok(),
        "and let in again"
    );
    // mistakes are answered in words, not crashes
    assert!(call(&g, "banpubkey", json!(["not a key"])).await["error"]
        .as_str()
        .unwrap()
        .contains("64 hex"));
    assert!(call(&g, "frobnicate", json!([])).await["error"]
        .as_str()
        .unwrap()
        .contains("not supported"));
    g.r.stop().await;
}

#[tokio::test]
async fn a_signed_request_captured_on_the_way_cannot_be_played_again() {
    let rig = rig().await;
    let body = json!({ "method": "supportedmethods", "params": [] }).to_string();
    let url = format!("http://{}/", rig.r.addr);
    let signed = auth(&rig.admin, &url, &body, now());
    let go = || {
        post(
            rig.r.addr,
            &body,
            "application/nostr+json+rpc",
            Some(&signed),
        )
    };
    assert_eq!(go().await.0, 200, "the first use is accepted");
    let (status, v) = go().await;
    assert_eq!(
        status, 401,
        "the same signed request again is a replay: {v}"
    );
    assert!(v.to_string().contains("already used"), "{v}");
    // a freshly signed one (a later second) is fine
    let later = auth(&rig.admin, &url, &body, now() + 1);
    assert_eq!(
        post(
            rig.r.addr,
            &body,
            "application/nostr+json+rpc",
            Some(&later)
        )
        .await
        .0,
        200
    );
}
