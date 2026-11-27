//! The relay, through real sockets: the same cases the Node relay's tests cover, and the access token and discovery page.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use futures_util::{SinkExt, StreamExt};
use privcord_server::{config::Config, start, Running};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn config(dir: &std::path::Path) -> Config {
    let mut c = Config::default();
    c.server.listen = "127.0.0.1:0".into();
    c.server.data_dir = dir.to_path_buf();
    c.relay.rate_limit = 100_000;
    c
}

async fn spawn(c: Config) -> (Running, String) {
    let r = start(c).await.unwrap();
    let url = format!("ws://{}", r.addr);
    (r, url)
}

struct Client(Ws);

impl Client {
    async fn open(url: &str) -> Self {
        Self(connect_async(url).await.unwrap().0)
    }
    async fn send(&mut self, v: Value) {
        self.0
            .send(Message::Text(v.to_string().into()))
            .await
            .unwrap();
    }
    async fn raw(&mut self, text: &str) {
        self.0
            .send(Message::Text(text.to_owned().into()))
            .await
            .unwrap();
    }
    /// The next JSON message that satisfies `want`.
    async fn next(&mut self, want: impl Fn(&Value) -> bool) -> Value {
        loop {
            let m = tokio::time::timeout(Duration::from_secs(5), self.0.next())
                .await
                .expect("timed out")
                .expect("closed")
                .unwrap();
            if let Message::Text(t) = m {
                let v: Value = serde_json::from_str(&t).unwrap();
                if want(&v) {
                    return v;
                }
            }
        }
    }
    async fn any(&mut self) -> Value {
        self.next(|_| true).await
    }
    async fn quiet(&mut self) -> bool {
        tokio::time::timeout(Duration::from_millis(300), self.0.next())
            .await
            .is_err()
    }
}

fn put(mailbox: &str, id: &str, blob: &str, ttl: i64, kind: &str) -> Value {
    json!({ "op": "put", "mailbox": mailbox, "id": id, "blob": blob, "ttl": ttl, "kind": kind })
}

async fn http_get(addr: std::net::SocketAddr, path: &str, extra: &str) -> (u16, String) {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n{extra}\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let status = out.split_whitespace().nth(1).unwrap().parse().unwrap();
    (
        status,
        out.split("\r\n\r\n").nth(1).unwrap_or("").to_owned(),
    )
}

#[tokio::test]
async fn live_signals_reach_a_listener_and_are_never_stored() {
    let dir = tempfile::tempdir().unwrap();
    let (r, url) = spawn(config(dir.path())).await;
    let (mut a, mut b) = (Client::open(&url).await, Client::open(&url).await);
    b.send(json!({ "op": "sub", "mailbox": "mb" })).await;
    b.next(|m| m["op"] == "ok").await;
    a.send(put("mb", "s1", "SIGNAL", 60, "sig")).await;
    assert_eq!(a.next(|m| m["op"] == "ok").await["id"], "s1");
    let e = b.next(|m| m["op"] == "event").await;
    assert_eq!(
        (e["blob"].as_str(), e["mailbox"].as_str(), e["id"].as_str()),
        (Some("SIGNAL"), Some("mb"), Some("s1"))
    );
    // A listener who comes later finds nothing: a signal is live only.
    let mut c = Client::open(&url).await;
    c.send(json!({ "op": "sub", "mailbox": "mb" })).await;
    assert_eq!(c.any().await["op"], "ok");
    assert!(c.quiet().await);
    r.stop().await;
}

#[tokio::test]
async fn messages_wait_survive_a_restart_and_deleting_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (r, url) = spawn(config(dir.path())).await;
    let mut a = Client::open(&url).await;
    a.send(put("mb", "m1", "ONE", 3600, "msg")).await;
    a.next(|m| m["op"] == "ok").await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let mid = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    tokio::time::sleep(Duration::from_millis(30)).await;
    a.send(put("mb", "m2", "TWO", 3600, "msg")).await;
    a.next(|m| m["op"] == "ok").await;
    a.send(json!({ "op": "del", "mailbox": "mb", "id": "m2" }))
        .await;
    assert_eq!(
        a.next(|m| m["op"] == "ok" && m["id"] == "m2").await["id"],
        "m2"
    );
    drop(a);
    r.stop().await;

    // The relay starts again on the same folder: both are still there.
    let mut cfg = config(dir.path());
    cfg.server.listen = "127.0.0.1:0".into();
    let (r, url) = spawn(cfg).await;
    let mut b = Client::open(&url).await;
    b.send(json!({ "op": "sub", "mailbox": "mb" })).await;
    let mut blobs = vec![
        b.next(|m| m["op"] == "event").await["blob"]
            .as_str()
            .unwrap()
            .to_owned(),
        b.next(|m| m["op"] == "event").await["blob"]
            .as_str()
            .unwrap()
            .to_owned(),
    ];
    blobs.sort();
    assert_eq!(blobs, ["ONE", "TWO"]);
    b.next(|m| m["op"] == "ok").await;
    // `since` is respected.
    let mut c = Client::open(&url).await;
    c.send(json!({ "op": "sub", "mailbox": "mb", "since": mid }))
        .await;
    assert_eq!(c.next(|m| m["op"] == "event").await["blob"], "TWO");
    c.next(|m| m["op"] == "ok").await;
    assert!(c.quiet().await);
    r.stop().await;
}

#[tokio::test]
async fn an_expired_message_is_not_handed_over() {
    let dir = tempfile::tempdir().unwrap();
    let (r, url) = spawn(config(dir.path())).await;
    let mut a = Client::open(&url).await;
    a.send(put("mb", "m1", "SHORT", 1, "msg")).await;
    a.next(|m| m["op"] == "ok").await;
    tokio::time::sleep(Duration::from_millis(1_300)).await;
    a.send(json!({ "op": "sub", "mailbox": "mb" })).await;
    assert_eq!(a.any().await["op"], "ok", "no event before the ok");
    r.stop().await;
    // And it is not there after a restart either.
    let (r, url) = spawn(config(dir.path())).await;
    let mut b = Client::open(&url).await;
    b.send(json!({ "op": "sub", "mailbox": "mb" })).await;
    assert_eq!(b.any().await["op"], "ok");
    r.stop().await;
}

#[tokio::test]
async fn the_limits_hold_and_bad_frames_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (r, url) = spawn(config(dir.path())).await;
    let mut a = Client::open(&url).await;
    for (frame, reason) in [
        (
            put("mb", "x", &"x".repeat(64 * 1024 + 1), 60, "msg"),
            "blob too large",
        ),
        (put("mb", "x", "B", 120 + 1, "sig"), "ttl too large"),
        (
            put("mb", "x", "B", 7 * 24 * 3600 + 1, "msg"),
            "ttl too large",
        ),
        (put("mb", "x", "B", 0, "msg"), "bad ttl"),
        (put("mb", "x", "B", 60, "wat"), "bad kind"),
        (put("", "x", "B", 60, "msg"), "bad mailbox"),
        (put(&"m".repeat(129), "x", "B", 60, "msg"), "bad mailbox"),
        (put("mb", "", "B", 60, "msg"), "bad id"),
        (put("mb", "x", "", 60, "msg"), "bad blob"),
        (json!({ "op": "explode" }), "unknown op"),
        (json!([1, 2]), "bad frame"),
    ] {
        a.send(frame).await;
        assert_eq!(a.any().await["reason"], reason);
    }
    a.raw("{not json").await;
    assert_eq!(a.any().await["reason"], "bad json");
    // A mailbox holds 500, and then says so.
    for i in 0..500 {
        a.send(put("full", &format!("i{i}"), "B", 3600, "msg"))
            .await;
        a.next(|m| m["op"] == "ok" || m["op"] == "err").await;
    }
    // (one address is capped lower than the mailbox: the 101st from here is the first refusal)
    a.send(put("full", "last", "B", 3600, "msg")).await;
    let m = a.any().await;
    assert_eq!(m["op"], "err");
    r.stop().await;
}

#[tokio::test]
async fn too_many_messages_a_minute_from_one_address_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.relay.rate_limit = 5;
    let (r, url) = spawn(c).await;
    let mut a = Client::open(&url).await;
    let mut limited = 0;
    for i in 0..10 {
        a.send(put("mb", &format!("p{i}"), "B", 60, "sig")).await;
        if a.any().await["reason"] == "rate limited" {
            limited += 1;
        }
    }
    assert!(limited >= 4, "{limited}");
    r.stop().await;
}

#[tokio::test]
async fn without_store_and_forward_only_live_signals_pass() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.relay.store_and_forward = false;
    let (r, url) = spawn(c).await;
    let mut a = Client::open(&url).await;
    a.send(put("mb", "m", "B", 60, "msg")).await;
    assert_eq!(a.any().await["reason"], "store-and-forward disabled");
    a.send(put("mb", "s", "B", 60, "sig")).await;
    assert_eq!(a.any().await["op"], "ok");
    r.stop().await;
}

#[tokio::test]
async fn healthz_gives_counters_only_and_less_behind_a_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let (r, _url) = spawn(config(dir.path())).await;
    let (status, body) = http_get(r.addr, "/healthz", "").await;
    assert_eq!(status, 200);
    let v: Value = serde_json::from_str(&body).unwrap();
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "connections",
            "delivered",
            "mailboxes",
            "ok",
            "put",
            "queued",
            "rejected"
        ]
    );
    let (_, proxied) = http_get(r.addr, "/healthz", "X-Forwarded-For: 1.2.3.4\r\n").await;
    assert_eq!(
        serde_json::from_str::<Value>(&proxied).unwrap(),
        json!({ "ok": true })
    );
    assert_eq!(http_get(r.addr, "/nope", "").await.0, 404);
    r.stop().await;
}

#[tokio::test]
async fn total_stored_bytes_are_capped() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.relay.max_queued_bytes = 2 * 64 * 1024;
    let (r, url) = spawn(c).await;
    let mut a = Client::open(&url).await;
    let big = "x".repeat(64 * 1024);
    for id in ["b1", "b2"] {
        a.send(put("mb", id, &big, 60, "msg")).await;
        assert_eq!(a.any().await["op"], "ok");
    }
    a.send(put("mb", "b3", &big, 60, "msg")).await;
    assert_eq!(a.any().await["reason"], "relay full");
    r.stop().await;
}

#[tokio::test]
async fn a_stranger_cannot_delete_what_waits_in_a_mailbox() {
    let dir = tempfile::tempdir().unwrap();
    let (r, url) = spawn(config(dir.path())).await;
    let (mut owner, mut stranger) = (Client::open(&url).await, Client::open(&url).await);
    owner.send(put("mine", "real", "SEALED", 3600, "msg")).await;
    owner.next(|m| m["op"] == "ok").await;
    stranger
        .send(json!({ "op": "sub", "mailbox": "mine" }))
        .await;
    stranger.next(|m| m["op"] == "event").await;
    stranger
        .send(json!({ "op": "del", "mailbox": "mine", "id": "real" }))
        .await;
    stranger
        .next(|m| m["op"] == "ok" && m["id"] == "real")
        .await;
    owner.send(json!({ "op": "sub", "mailbox": "mine" })).await;
    assert_eq!(owner.next(|m| m["op"] == "event").await["blob"], "SEALED");
    r.stop().await;
}

#[tokio::test]
async fn one_address_cannot_fill_a_mailbox_and_a_connection_cannot_listen_to_everything() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.relay.max_per_mailbox_per_ip = 3;
    c.relay.max_subs_per_conn = 2;
    let (r, url) = spawn(c).await;
    let mut a = Client::open(&url).await;
    for i in 0..3 {
        a.send(put("target", &format!("q{i}"), "B", 60, "msg"))
            .await;
        assert_eq!(a.any().await["op"], "ok");
    }
    a.send(put("target", "q4", "B", 60, "msg")).await;
    assert_eq!(a.any().await["reason"], "mailbox quota for this address");
    for mb in ["s1", "s2"] {
        a.send(json!({ "op": "sub", "mailbox": mb })).await;
        assert_eq!(a.any().await["op"], "ok");
    }
    a.send(json!({ "op": "sub", "mailbox": "s3" })).await;
    assert_eq!(a.any().await["reason"], "too many subscriptions");
    r.stop().await;
}

#[tokio::test]
async fn behind_a_trusted_proxy_the_address_is_the_one_the_proxy_added() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.server.trust_proxy = true;
    c.relay.max_conns_per_ip = 1;
    let (r, url) = spawn(c).await;
    let open = |xff: &str| {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = url.clone().into_client_request().unwrap();
        req.headers_mut()
            .insert("x-forwarded-for", xff.parse().unwrap());
        async move { connect_async(req).await.unwrap().0 }
    };
    let _first = open("6.6.6.6, 10.0.0.1").await;
    // A different rightmost address is a different visitor, whatever the visitor claims to the left of it.
    let mut second = open("6.6.6.6, 10.0.0.2").await;
    second
        .send(Message::Text(
            put("mb", "x", "B", 60, "sig").to_string().into(),
        ))
        .await
        .unwrap();
    // The same rightmost address as the first: refused with a close.
    let mut third = open("1.1.1.1, 10.0.0.1").await;
    let m = tokio::time::timeout(Duration::from_secs(3), third.next())
        .await
        .unwrap();
    assert!(matches!(m, Some(Ok(Message::Close(_)))), "{m:?}");
    r.stop().await;
}

#[tokio::test]
async fn a_private_server_needs_its_token_and_says_what_it_offers() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.server.access_tokens = vec!["let-me-in".into()];
    c.server.admin_contact = "admin@example.org".into();
    let (r, url) = spawn(c).await;
    assert!(connect_async(&url).await.is_err(), "no token");
    assert!(connect_async(format!("{url}/?token=wrong")).await.is_err());
    let mut ok = Client::open(&format!("{url}/?token=let-me-in")).await;
    ok.send(put("mb", "s", "B", 60, "sig")).await;
    assert_eq!(ok.any().await["op"], "ok");
    // The discovery page is open: an app reads it before it has a token.
    let (status, body) = http_get(r.addr, "/.well-known/privcord-server", "").await;
    assert_eq!(status, 200);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["roles"], json!(["relay"]));
    assert_eq!(v["requires_token"], true);
    assert_eq!(v["admin"], "admin@example.org");
    assert_eq!(v["relay"]["max_blob_bytes"], 65_536);
    r.stop().await;
}

#[tokio::test]
async fn the_header_that_carries_the_client_address_is_configurable() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.server.trust_proxy = true;
    c.server.real_ip_header = "cf-connecting-ip".into();
    c.relay.max_conns_per_ip = 1;
    let (r, url) = spawn(c).await;
    let open = |header: &'static str, ip: &str| {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = url.clone().into_client_request().unwrap();
        req.headers_mut().insert(header, ip.parse().unwrap());
        async move { connect_async(req).await.unwrap().0 }
    };
    let _first = open("cf-connecting-ip", "6.6.6.6").await;
    // another visitor, as the configured header says
    let mut second = open("cf-connecting-ip", "7.7.7.7").await;
    second
        .send(Message::Text(
            put("mb", "x", "B", 60, "sig").to_string().into(),
        ))
        .await
        .unwrap();
    // x-forwarded-for is not the configured header, so it does not make a connection look like somebody else
    let mut third = open("x-forwarded-for", "9.9.9.9").await;
    let m = tokio::time::timeout(Duration::from_secs(3), third.next()).await;
    // third is seen as the proxy's own address (one connection from it is allowed), so it is not refused as a duplicate
    assert!(!matches!(m, Ok(Some(Ok(Message::Close(_))))), "{m:?}");
    r.stop().await;
}

#[tokio::test]
async fn metrics_are_off_by_default_and_need_the_token_when_on() {
    let dir = tempfile::tempdir().unwrap();
    let (r, _url) = spawn(config(dir.path())).await;
    assert_eq!(http_get(r.addr, "/metrics", "").await.0, 404);
    r.stop().await;

    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.server.metrics = true;
    c.server.access_tokens = vec!["sesame".into()];
    let (r, _url) = spawn(c).await;
    assert_eq!(http_get(r.addr, "/metrics", "").await.0, 401);
    let (code, body) = http_get(r.addr, "/metrics", "Authorization: Bearer sesame\r\n").await;
    assert_eq!(code, 200);
    assert!(
        body.contains("privcord_put ") && body.contains("privcord_connections "),
        "{body}"
    );
    r.stop().await;
}

#[tokio::test]
async fn a_client_that_stops_reading_is_cut_off_and_its_queue_released() {
    // Found in review: a client could subscribe to big mailboxes, never read, and hold the relay's memory (and slow everyone).
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.relay.stall_secs = 1;
    c.relay.max_conns_per_ip = 100;
    c.relay.max_per_mailbox_per_ip = 100_000;
    c.relay.backpressure_bytes = 1 << 30;
    let (r, url) = spawn(c).await;
    let blob = "A".repeat(60_000);
    let mut filler = Client::open(&url).await;
    for mb in 0..4 {
        for i in 0..120 {
            filler
                .send(put(
                    &format!("big{mb}"),
                    &format!("m{i}"),
                    &blob,
                    600,
                    "msg",
                ))
                .await;
            filler.next(|m| m["op"] == "ok" || m["op"] == "err").await;
        }
    }
    // a reader that subscribes to everything and then never reads
    let mut lazy = Client::open(&url).await;
    for mb in 0..4 {
        lazy.send(json!({ "op": "sub", "mailbox": format!("big{mb}") }))
            .await;
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    let queued = r
        .relay()
        .unwrap()
        .out_total()
        .load(std::sync::atomic::Ordering::Relaxed);
    assert!(queued > 0, "the replay should be waiting to be written");
    let mut freed = false;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if r.relay()
            .unwrap()
            .out_total()
            .load(std::sync::atomic::Ordering::Relaxed)
            == 0
        {
            freed = true;
            break;
        }
    }
    assert!(
        freed,
        "the stalled client's queue should have been released"
    );
    drop(lazy);
    r.stop().await;
}

#[tokio::test]
async fn the_address_header_is_believed_only_from_the_proxies_you_name() {
    // Found in review: a visitor reaching the server directly could choose which address the limits counted.
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.server.trust_proxy = true;
    c.server.trusted_proxies = vec!["10.9.9.9".into()]; // not the test client (127.0.0.1)
    c.relay.max_conns_per_ip = 1;
    let (r, url) = spawn(c).await;
    let open = |xff: &'static str| {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = url.clone().into_client_request().unwrap();
        req.headers_mut()
            .insert("x-forwarded-for", xff.parse().unwrap());
        async move { connect_async(req).await.unwrap().0 }
    };
    let _first = open("6.6.6.6").await;
    // a different claimed address does not make it a different visitor: both are really 127.0.0.1
    let mut second = open("7.7.7.7").await;
    let m = tokio::time::timeout(Duration::from_secs(3), second.next())
        .await
        .unwrap();
    assert!(matches!(m, Some(Ok(Message::Close(_)))), "{m:?}");
    r.stop().await;
}

#[tokio::test]
async fn an_ipv6_visitor_counts_as_one_however_many_addresses_it_has() {
    use privcord_server::net::ip_group;
    let a: std::net::IpAddr = "2001:db8:1:2:aaaa:bbbb:cccc:dddd".parse().unwrap();
    let b: std::net::IpAddr = "2001:db8:1:2:1111:2222:3333:4444".parse().unwrap();
    let other: std::net::IpAddr = "2001:db8:1:3::1".parse().unwrap();
    assert_eq!(ip_group(a), ip_group(b));
    assert_ne!(ip_group(a), ip_group(other));
    let v4: std::net::IpAddr = "203.0.113.5".parse().unwrap();
    assert_eq!(ip_group(v4), v4);
}
