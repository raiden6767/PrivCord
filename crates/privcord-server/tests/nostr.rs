//! The Nostr side, through real sockets, with really signed events.
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

async fn server(tweak: impl FnOnce(&mut Config)) -> (Running, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Config::default();
    c.server.listen = "127.0.0.1:0".into();
    c.server.data_dir = dir.path().into();
    c.nostr.enabled = true;
    c.nostr.events_per_minute_per_ip = 0;
    c.relay.rate_limit = 100_000;
    tweak(&mut c);
    let r = start(c).await.unwrap();
    let url = format!("ws://{}", r.addr);
    (r, url, dir)
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

fn ids(v: &[Value]) -> Vec<&str> {
    v.iter().map(|e| e["id"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn an_event_can_be_published_and_read_back_and_a_repeat_is_noticed() {
    let (r, url, _d) = server(|_| {}).await;
    let (k, mut c) = (key(), C::open(&url).await);
    let ev = make(&k, 1, now(), vec![], "hello");
    assert_eq!(c.publish(&ev).await, (true, String::new()));
    let got = c.req("s", vec![json!({ "authors": [pk(&k)] })]).await;
    assert_eq!(ids(&got), vec![ev["id"].as_str().unwrap()]);
    let (ok, why) = c.publish(&ev).await;
    assert!(ok && why.starts_with("duplicate"), "{why}");
    r.stop().await;
}

#[tokio::test]
async fn forged_events_are_refused() {
    let (r, url, _d) = server(|_| {}).await;
    let (k, mut c) = (key(), C::open(&url).await);
    let mut bad = make(&k, 1, now(), vec![], "original");
    bad["content"] = json!("tampered");
    let (ok, why) = c.publish(&bad).await;
    assert!(!ok && why.starts_with("invalid"), "{why}");
    let mut wrong_sig = make(&k, 1, now(), vec![], "x");
    let other = make(&key(), 1, now(), vec![], "x");
    wrong_sig["sig"] = other["sig"].clone();
    assert!(!c.publish(&wrong_sig).await.0);
    let future = make(&k, 1, now() + 7200, vec![], "from the future");
    assert!(c.publish(&future).await.1.contains("future"));
    r.stop().await;
}

#[tokio::test]
async fn live_events_reach_subscribers_by_tag_and_ephemeral_ones_are_never_stored() {
    let (r, url, _d) = server(|_| {}).await;
    let (k, mut a, mut b) = (key(), C::open(&url).await, C::open(&url).await);
    assert!(b
        .req("mb", vec![json!({ "kinds": [21733], "#t": ["room-1"] })])
        .await
        .is_empty());
    let hit = make(&k, 21733, now(), vec![vec!["t", "room-1"]], "signal");
    let miss = make(&k, 21733, now(), vec![vec!["t", "room-2"]], "elsewhere");
    a.publish(&miss).await;
    assert!(a.publish(&hit).await.0);
    let e = b.next().await;
    assert_eq!(
        (e[0].as_str(), e[1].as_str(), e[2]["id"].as_str()),
        (Some("EVENT"), Some("mb"), hit["id"].as_str())
    );
    assert!(b.quiet().await, "the other room's signal must not arrive");
    let mut late = C::open(&url).await;
    assert!(
        late.req("x", vec![json!({ "kinds": [21733] })])
            .await
            .is_empty(),
        "a signal is live only"
    );
    b.send(json!(["CLOSE", "mb"])).await;
    a.publish(&make(
        &k,
        21733,
        now(),
        vec![vec!["t", "room-1"]],
        "after close",
    ))
    .await;
    assert!(b.quiet().await);
    r.stop().await;
}

#[tokio::test]
async fn replaceable_and_addressable_events_keep_only_the_newest() {
    let (r, url, _d) = server(|_| {}).await;
    let (k, mut c) = (key(), C::open(&url).await);
    let t = now() - 100;
    let old = make(&k, 0, t, vec![], "old profile");
    let new = make(&k, 0, t + 10, vec![], "new profile");
    c.publish(&old).await;
    c.publish(&new).await;
    let (ok, why) = c.publish(&old).await;
    assert!(ok && why.starts_with("duplicate"));
    let got = c
        .req("p", vec![json!({ "kinds": [0], "authors": [pk(&k)] })])
        .await;
    assert_eq!(ids(&got), vec![new["id"].as_str().unwrap()]);
    let a1 = make(&k, 30000, t, vec![vec!["d", "one"]], "a1");
    let a2 = make(&k, 30000, t, vec![vec!["d", "two"]], "a2");
    let a1b = make(&k, 30000, t + 5, vec![vec!["d", "one"]], "a1 again");
    for e in [&a1, &a2, &a1b] {
        c.publish(e).await;
    }
    let got = c.req("q", vec![json!({ "kinds": [30000] })]).await;
    let mut want = vec![a2["id"].as_str().unwrap(), a1b["id"].as_str().unwrap()];
    want.sort_unstable();
    let mut have = ids(&got);
    have.sort_unstable();
    assert_eq!(have, want);
    r.stop().await;
}

#[tokio::test]
async fn deletion_removes_only_your_own_events_and_they_stay_deleted() {
    let (r, url, _d) = server(|_| {}).await;
    let (k, stranger, mut c) = (key(), key(), C::open(&url).await);
    let ev = make(&k, 1, now(), vec![], "regret");
    c.publish(&ev).await;
    let by_stranger = make(
        &stranger,
        5,
        now(),
        vec![vec!["e", ev["id"].as_str().unwrap()]],
        "",
    );
    c.publish(&by_stranger).await;
    assert_eq!(
        c.req("a", vec![json!({ "kinds": [1] })]).await.len(),
        1,
        "a stranger cannot delete it"
    );
    let by_author = make(
        &k,
        5,
        now(),
        vec![vec!["e", ev["id"].as_str().unwrap()]],
        "",
    );
    assert!(c.publish(&by_author).await.0);
    assert!(c.req("b", vec![json!({ "kinds": [1] })]).await.is_empty());
    let (ok, why) = c.publish(&ev).await;
    assert!(!ok && why.starts_with("blocked"), "{why}");
    r.stop().await;
}

#[tokio::test]
async fn expiring_events_are_refused_when_late_and_vanish_when_due() {
    let (r, url, _d) = server(|_| {}).await;
    let (k, mut c) = (key(), C::open(&url).await);
    let late = make(
        &k,
        1,
        now(),
        vec![vec!["expiration", &(now() - 5).to_string()]],
        "too late",
    );
    assert!(c.publish(&late).await.1.contains("expired"));
    let brief = make(
        &k,
        1,
        now(),
        vec![vec!["expiration", &(now() + 3).to_string()]],
        "brief",
    );
    assert!(c.publish(&brief).await.0);
    assert_eq!(
        c.req("a", vec![json!({ "authors": [pk(&k)] })]).await.len(),
        1
    );
    tokio::time::sleep(Duration::from_millis(4300)).await;
    assert!(c
        .req("b", vec![json!({ "authors": [pk(&k)] })])
        .await
        .is_empty());
    r.stop().await;
}

#[tokio::test]
async fn filters_pick_by_kind_time_id_author_and_tag_newest_first_with_a_limit() {
    let (r, url, _d) = server(|_| {}).await;
    let (k1, k2, mut c) = (key(), key(), C::open(&url).await);
    let t = now() - 1000;
    let e: Vec<Value> = (0..6u64)
        .map(|i| {
            make(
                if i % 2 == 0 { &k1 } else { &k2 },
                1 + i % 2,
                t + i as i64 * 10,
                vec![vec!["t", if i < 3 { "red" } else { "blue" }]],
                &format!("n{i}"),
            )
        })
        .collect();
    for ev in &e {
        c.publish(ev).await;
    }
    let id = |i: usize| e[i]["id"].as_str().unwrap();
    assert_eq!(
        ids(&c.req("a", vec![json!({ "kinds": [1, 2] })]).await),
        vec![id(5), id(4), id(3), id(2), id(1), id(0)]
    );
    assert_eq!(
        ids(&c
            .req("b", vec![json!({ "kinds": [1, 2], "limit": 2 })])
            .await),
        vec![id(5), id(4)]
    );
    assert_eq!(
        ids(&c
            .req("c", vec![json!({ "since": t + 20, "until": t + 40 })])
            .await),
        vec![id(4), id(3), id(2)]
    );
    assert_eq!(
        ids(&c.req("d", vec![json!({ "#t": ["blue"] })]).await),
        vec![id(5), id(4), id(3)]
    );
    assert_eq!(
        ids(&c
            .req("e", vec![json!({ "authors": [pk(&k1)], "#t": ["red"] })])
            .await),
        vec![id(2), id(0)]
    );
    assert_eq!(
        ids(&c.req("f", vec![json!({ "ids": [&id(3)[..12]] })]).await),
        vec![id(3)]
    );
    assert_eq!(
        ids(&c.req("g", vec![json!({ "ids": [id(1)] })]).await),
        vec![id(1)]
    );
    let two = c
        .req(
            "h",
            vec![
                json!({ "#t": ["red"] }),
                json!({ "#t": ["blue"], "limit": 1 }),
            ],
        )
        .await;
    assert_eq!(
        two.len(),
        4,
        "filters are joined, and an event matching two is sent once"
    );
    c.send(json!(["COUNT", "n", { "kinds": [1] }])).await;
    assert_eq!(c.next().await, json!(["COUNT", "n", { "count": 3 }]));
    r.stop().await;
}

#[tokio::test]
async fn limits_and_bad_requests_are_answered_not_fatal() {
    let (r, url, _d) = server(|c| {
        c.nostr.max_subs_per_conn = 2;
        c.nostr.max_event_bytes = 2000;
    })
    .await;
    let (k, mut c) = (key(), C::open(&url).await);
    for s in ["a", "b"] {
        c.req(s, vec![json!({ "kinds": [1] })]).await;
    }
    c.send(json!(["REQ", "c", { "kinds": [1] }])).await;
    let m = c.next().await;
    assert_eq!(
        (m[0].as_str(), m[2].as_str().unwrap().starts_with("error")),
        (Some("CLOSED"), true)
    );
    c.req("a", vec![json!({ "kinds": [2] })]).await; // replacing a subscription is allowed
    c.send(json!(["REQ", "x".repeat(257), {}])).await;
    assert!(c.next().await[2].as_str().unwrap().starts_with("invalid"));
    c.send(json!(["REQ", "a", { "kinds": "no" }])).await;
    assert!(c.next().await[2].as_str().unwrap().starts_with("invalid"));
    c.0.send(Message::Text("[not json".into())).await.unwrap();
    assert_eq!(c.next().await[0], "NOTICE");
    let big = make(&k, 1, now(), vec![], &"x".repeat(3000));
    assert!(c.publish(&big).await.1.contains("too large"));
    c.send(json!(["WHAT"])).await;
    assert_eq!(c.next().await[0], "NOTICE");
    r.stop().await;
}

#[tokio::test]
async fn the_relay_information_document_is_served() {
    let (r, _url, _d) = server(|c| c.nostr.name = "Test relay".into()).await;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(r.addr).await.unwrap();
    s.write_all(
        b"GET / HTTP/1.1\r\nHost: x\r\nAccept: application/nostr+json\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    let low = out.to_lowercase();
    assert!(
        low.contains("application/nostr+json") && low.contains("access-control-allow-origin: *")
    );
    let doc: Value = serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(doc["name"], "Test relay");
    assert!(doc["supported_nips"]
        .as_array()
        .unwrap()
        .contains(&json!(1)));
    r.stop().await;
}

#[tokio::test]
async fn the_administrators_rules_are_enforced() {
    let (allowed, blocked) = (key(), key());
    let (r, url, _d) = server(|c| {
        c.nostr.allowed_pubkeys = vec![pk(&allowed), pk(&blocked)];
        c.nostr.blocked_pubkeys = vec![pk(&blocked)];
        c.nostr.blocked_kinds = vec![7];
        c.nostr.min_pow = 8;
    })
    .await;
    let mut c = C::open(&url).await;
    let mine = |k: &SigningKey, kind: u64| {
        // mine a few bits of proof of work
        for n in 0u64.. {
            let e = make(
                k,
                kind,
                now(),
                vec![vec!["nonce", &n.to_string(), "8"]],
                "work",
            );
            let id: [u8; 32] = unhex(e["id"].as_str().unwrap());
            if id[0] == 0 {
                return e;
            }
        }
        unreachable!()
    };
    assert!(c.publish(&mine(&allowed, 1)).await.0);
    assert!(c
        .publish(&make(&allowed, 1, now(), vec![], "no work"))
        .await
        .1
        .starts_with("pow"));
    assert!(c.publish(&mine(&allowed, 7)).await.1.starts_with("blocked"));
    assert!(c.publish(&mine(&blocked, 1)).await.1.starts_with("blocked"));
    assert!(c
        .publish(&mine(&key(), 1))
        .await
        .1
        .starts_with("restricted"));
    r.stop().await;
}

fn unhex(s: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
    }
    out
}

#[tokio::test]
async fn a_relay_that_wants_authentication_asks_and_checks() {
    let (r, url, _d) = server(|c| {
        c.nostr.auth_required = true;
        c.nostr.relay_url = "ws://x".into();
    })
    .await;
    let (k, mut c) = (key(), C::open(&url).await);
    c.send(json!(["REQ", "s", { "kinds": [1] }])).await;
    let challenge = c.next().await;
    assert_eq!(challenge[0], "AUTH");
    let m = c.next().await;
    assert_eq!(
        (
            m[0].as_str(),
            m[2].as_str().unwrap().starts_with("auth-required")
        ),
        (Some("CLOSED"), true)
    );
    assert!(c
        .publish(&make(&k, 1, now(), vec![], "x"))
        .await
        .1
        .starts_with("auth-required"));
    let wrong = make(
        &k,
        22242,
        now(),
        vec![vec!["relay", "ws://x"], vec!["challenge", "not-it"]],
        "",
    );
    c.send(json!(["AUTH", wrong])).await;
    assert_eq!(c.next().await[2], false);
    let right = make(
        &k,
        22242,
        now(),
        vec![
            vec!["relay", "ws://x"],
            vec!["challenge", challenge[1].as_str().unwrap()],
        ],
        "",
    );
    c.send(json!(["AUTH", right])).await;
    assert_eq!(c.next().await[2], true);
    assert!(
        c.publish(&make(&k, 1, now(), vec![], "now it works"))
            .await
            .0
    );
    r.stop().await;
}

#[tokio::test]
async fn events_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let k = key();
    let ev = make(&k, 1, now(), vec![], "kept");
    let gone = make(&k, 1, now(), vec![], "deleted");
    for round in 0..2 {
        let mut c = Config::default();
        c.server.listen = "127.0.0.1:0".into();
        c.server.data_dir = dir.path().into();
        c.nostr.enabled = true;
        c.nostr.events_per_minute_per_ip = 0;
        let r = start(c).await.unwrap();
        let mut cl = C::open(&format!("ws://{}", r.addr)).await;
        if round == 0 {
            cl.publish(&ev).await;
            cl.publish(&gone).await;
            cl.publish(&make(
                &k,
                5,
                now(),
                vec![vec!["e", gone["id"].as_str().unwrap()]],
                "",
            ))
            .await;
        } else {
            let got = cl.req("a", vec![json!({ "kinds": [1] })]).await;
            assert_eq!(
                ids(&got),
                vec![ev["id"].as_str().unwrap()],
                "the deletion must survive the restart too"
            );
        }
        r.stop().await;
    }
}

#[tokio::test]
async fn both_protocols_work_on_one_socket_and_publishing_can_be_rate_limited() {
    let (r, url, _d) = server(|c| c.nostr.events_per_minute_per_ip = 3).await;
    let (k, mut c) = (key(), C::open(&url).await);
    c.send(json!({ "op": "sub", "mailbox": "mb" })).await;
    assert_eq!(c.next().await["op"], "ok");
    assert!(c.publish(&make(&k, 1, now(), vec![], "one")).await.0);
    c.send(
        json!({ "op": "put", "mailbox": "mb", "id": "m1", "blob": "B", "ttl": 60, "kind": "sig" }),
    )
    .await;
    let mut seen = Vec::new();
    for _ in 0..2 {
        seen.push(c.next().await["op"].as_str().unwrap().to_owned());
    }
    seen.sort();
    assert_eq!(seen, vec!["event", "ok"]);
    for i in 0..4 {
        let (ok, why) = c
            .publish(&make(&k, 1, now(), vec![], &format!("burst {i}")))
            .await;
        if i >= 2 {
            assert!(!ok && why.starts_with("rate-limited"), "{i}: {why}");
        }
    }
    r.stop().await;
}

// ---- what was learned from nostr-rs-relay ----

#[tokio::test]
async fn a_deletion_that_arrives_before_its_event_still_wins_and_only_for_the_author() {
    let (r, url, _d) = server(|_| {}).await;
    let (k, other, mut c) = (key(), key(), C::open(&url).await);
    let doomed = make(&k, 1, now(), vec![], "published after its deletion");
    let del = make(
        &k,
        5,
        now(),
        vec![vec!["e", doomed["id"].as_str().unwrap()]],
        "",
    );
    assert!(c.publish(&del).await.0);
    let (ok, why) = c.publish(&doomed).await;
    assert!(!ok && why.starts_with("blocked"), "{why}");
    // somebody else's deletion of an event that has not arrived yet must not block its real author
    let mine = make(&k, 1, now(), vec![], "mine");
    c.publish(&make(
        &other,
        5,
        now(),
        vec![vec!["e", mine["id"].as_str().unwrap()]],
        "",
    ))
    .await;
    assert!(
        c.publish(&mine).await.0,
        "a stranger's deletion must not stop the author"
    );
    r.stop().await;
}

#[tokio::test]
async fn deleting_an_address_also_refuses_older_versions_that_arrive_later() {
    let (r, url, _d) = server(|_| {}).await;
    let (k, mut c) = (key(), C::open(&url).await);
    let t = now() - 50;
    let addr = format!("30000:{}:slot", pk(&k));
    assert!(
        c.publish(&make(&k, 5, t, vec![vec!["a", &addr]], ""))
            .await
            .0
    );
    let old = make(
        &k,
        30000,
        t - 10,
        vec![vec!["d", "slot"]],
        "older than the deletion",
    );
    assert!(c.publish(&old).await.1.starts_with("blocked"));
    let newer = make(
        &k,
        30000,
        t + 10,
        vec![vec!["d", "slot"]],
        "newer than the deletion",
    );
    assert!(
        c.publish(&newer).await.0,
        "a version after the deletion is fine"
    );
    r.stop().await;
}

#[tokio::test]
async fn in_durable_mode_the_confirmation_comes_after_the_event_is_on_disk() {
    let (r, url, dir) = server(|c| c.nostr.durable = true).await;
    let (k, mut sub, mut c) = (key(), C::open(&url).await, C::open(&url).await);
    sub.req("s", vec![json!({ "kinds": [1] })]).await;
    let ev = make(&k, 1, now(), vec![], "durable");
    assert_eq!(c.publish(&ev).await, (true, String::new()));
    let log = std::fs::read_to_string(dir.path().join("nostr-events.jsonl")).unwrap();
    assert!(
        log.contains(ev["id"].as_str().unwrap()),
        "confirmed before it was written"
    );
    assert_eq!(
        sub.next().await[2]["id"],
        ev["id"],
        "subscribers get it too"
    );
    // many at once all get confirmed (group commit)
    let evs: Vec<Value> = (0..50)
        .map(|i| make(&k, 1, now(), vec![], &format!("n{i}")))
        .collect();
    for e in &evs {
        c.send(json!(["EVENT", e])).await;
    }
    let mut oks = 0;
    while oks < 50 {
        if c.next().await[0] == "OK" {
            oks += 1;
        }
    }
    r.stop().await;
}

#[tokio::test]
async fn direct_messages_go_only_to_their_sender_and_recipient_when_asked() {
    let (alice, bob, eve) = (key(), key(), key());
    let (r, url, _d) = server(|c| {
        c.nostr.auth_dms = true;
        c.nostr.relay_url = "ws://x".into();
    })
    .await;
    let dm = make(&alice, 4, now(), vec![vec!["p", &pk(&bob)]], "secret");
    let mut a = C::open(&url).await;
    // the AUTH challenge comes first on a relay that cares
    let challenge = a.next().await;
    assert_eq!(challenge[0], "AUTH");
    let auth = |k: &SigningKey, ch: &Value| {
        make(
            k,
            22242,
            now(),
            vec![
                vec!["relay", "ws://x"],
                vec!["challenge", ch[1].as_str().unwrap()],
            ],
            "",
        )
    };
    a.send(json!(["AUTH", auth(&alice, &challenge)])).await;
    assert_eq!(a.next().await[2], true);
    assert!(a.publish(&dm).await.0);
    // an unauthenticated stranger sees nothing, and cannot count it either
    let mut stranger = C::open(&url).await;
    stranger.next().await; // challenge
    assert!(stranger
        .req("x", vec![json!({ "kinds": [4] })])
        .await
        .is_empty());
    stranger.send(json!(["COUNT", "c", { "kinds": [4] }])).await;
    assert_eq!(stranger.next().await, json!(["COUNT", "c", { "count": 0 }]));
    // a stranger who authenticates as somebody else sees nothing
    let mut e = C::open(&url).await;
    let ch = e.next().await;
    e.send(json!(["AUTH", auth(&eve, &ch)])).await;
    e.next().await;
    assert!(e.req("x", vec![json!({ "kinds": [4] })]).await.is_empty());
    // the recipient sees it, stored and live
    let mut b = C::open(&url).await;
    let ch = b.next().await;
    b.send(json!(["AUTH", auth(&bob, &ch)])).await;
    b.next().await;
    assert_eq!(b.req("x", vec![json!({ "kinds": [4] })]).await.len(), 1);
    let live = make(&alice, 4, now(), vec![vec!["p", &pk(&bob)]], "live");
    a.publish(&live).await;
    assert_eq!(b.next().await[2]["id"], live["id"]);
    assert!(
        e.quiet().await,
        "eve must not receive live direct messages either"
    );
    r.stop().await;
}

#[tokio::test]
async fn a_connection_cannot_open_subscriptions_faster_than_allowed_and_the_relay_caps_events_per_second(
) {
    let (r, url, _d) = server(|c| {
        c.nostr.reqs_per_minute_per_conn = 3;
        c.nostr.events_per_second_global = 5;
    })
    .await;
    let mut c = C::open(&url).await;
    for i in 0..3 {
        c.req(&format!("s{i}"), vec![json!({ "kinds": [1] })]).await;
    }
    c.send(json!(["REQ", "s3", { "kinds": [1] }])).await;
    let m = c.next().await;
    assert_eq!(m[0], "CLOSED");
    assert!(m[2].as_str().unwrap().starts_with("rate-limited"));
    // events: no more than 5 in one second across everybody
    let k = key();
    let mut refused = 0;
    for i in 0..12 {
        if !c
            .publish(&make(&k, 1, now(), vec![], &format!("e{i}")))
            .await
            .0
        {
            refused += 1;
        }
    }
    assert!(refused >= 4, "refused only {refused}");
    r.stop().await;
}

#[tokio::test]
async fn memory_only_mode_keeps_nothing_on_disk() {
    let (r, url, dir) = server(|c| c.nostr.persist = false).await;
    let (k, mut c) = (key(), C::open(&url).await);
    assert!(c.publish(&make(&k, 1, now(), vec![], "in memory")).await.0);
    assert_eq!(c.req("a", vec![json!({ "kinds": [1] })]).await.len(), 1);
    assert!(!dir.path().join("nostr-events.jsonl").exists());
    r.stop().await;
}

#[tokio::test]
async fn the_information_document_carries_the_admin_key() {
    let (r, _url, _d) = server(|c| c.nostr.admin_pubkey = "ab".repeat(32)).await;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(r.addr).await.unwrap();
    s.write_all(
        b"GET / HTTP/1.1\r\nHost: x\r\nAccept: application/nostr+json\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let doc: Value = serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(doc["pubkey"], "ab".repeat(32));
    r.stop().await;
}

#[tokio::test]
async fn somebody_else_naming_a_deleted_event_cannot_bring_it_back() {
    // Found by the randomised store test: a second deletion naming the same id used to displace the author's own record,
    // so the (still validly signed) original could be published again.
    let (r, url, _d) = server(|_| {}).await;
    let (author, attacker, mut c) = (key(), key(), C::open(&url).await);
    let ev = make(&author, 1, now(), vec![], "deleted by its author");
    assert!(c.publish(&ev).await.0);
    assert!(
        c.publish(&make(
            &author,
            5,
            now(),
            vec![vec!["e", ev["id"].as_str().unwrap()]],
            ""
        ))
        .await
        .0
    );
    // the attacker publishes a deletion that merely names the same id
    c.publish(&make(
        &attacker,
        5,
        now(),
        vec![vec!["e", ev["id"].as_str().unwrap()]],
        "",
    ))
    .await;
    let (ok, why) = c.publish(&ev).await;
    assert!(
        !ok && why.starts_with("blocked"),
        "the deleted event came back: {why}"
    );
    assert!(c.req("x", vec![json!({ "kinds": [1] })]).await.is_empty());
    r.stop().await;
}

#[tokio::test]
async fn an_authentication_signed_for_another_relay_is_refused() {
    // A malicious relay can hand a client our challenge and replay what the client signs. The relay tag must be this relay.
    let (r, url, _d) = server(|c| {
        c.nostr.auth_required = true;
        c.nostr.relay_url = "wss://relay.example.org".into();
    })
    .await;
    let (k, mut c) = (key(), C::open(&url).await);
    let challenge = c.next().await;
    let sign = |relay: &str| {
        make(
            &k,
            22242,
            now(),
            vec![
                vec!["relay", relay],
                vec!["challenge", challenge[1].as_str().unwrap()],
            ],
            "",
        )
    };
    for wrong in [
        "wss://evil.example",
        "wss://relay.example.org:8443",
        "ws://relay.example.org:8080",
        "not a url",
        "",
    ] {
        c.send(json!(["AUTH", sign(wrong)])).await;
        assert_eq!(c.next().await[2], false, "{wrong}");
    }
    // the right name, in the ways a client might write it
    c.send(json!(["AUTH", sign("wss://RELAY.example.org/")]))
        .await;
    assert_eq!(c.next().await[2], true);
    r.stop().await;
}

#[tokio::test]
async fn authentication_without_a_relay_address_in_the_settings_is_an_error_in_words() {
    let mut c = Config::default();
    c.server.listen = "127.0.0.1:0".into();
    c.nostr.enabled = true;
    c.nostr.auth_dms = true;
    let e = start(c).await.unwrap_err();
    assert!(e.contains("nostr.relay_url"), "{e}");
}

#[tokio::test]
async fn filters_with_too_many_values_and_counts_and_requests_beyond_the_rate_are_refused() {
    let (r, url, _d) = server(|c| {
        c.nostr.max_filter_values = 10;
        c.nostr.reqs_per_minute_per_conn = 4;
    })
    .await;
    let mut c = C::open(&url).await;
    let many: Vec<String> = (0..11).map(|i| format!("{i:064x}")).collect();
    c.send(json!(["REQ", "a", { "authors": many }])).await;
    let m = c.next().await;
    assert_eq!(m[0], "CLOSED");
    assert!(m[2].as_str().unwrap().contains("too many values"), "{m}");
    c.send(json!(["COUNT", "c", { "authors": many }])).await;
    assert_eq!(c.next().await[0], "CLOSED");
    // that was three requests (two refused for size count too); the fourth is the last allowed, the fifth is limited
    c.req("b", vec![json!({ "kinds": [1] })]).await;
    c.send(json!(["COUNT", "d", { "kinds": [1] }])).await;
    assert_eq!(
        c.next().await[0],
        "COUNT",
        "the fourth request is still within the limit"
    );
    c.send(json!(["COUNT", "e", { "kinds": [1] }])).await;
    let m = c.next().await;
    assert!(m[2].as_str().unwrap().starts_with("rate-limited"), "{m}");
    r.stop().await;
}

#[tokio::test]
async fn the_store_keeps_to_its_size_limit_by_dropping_the_oldest_and_an_oversized_event_is_refused(
) {
    let (r, url, _d) = server(|c| c.nostr.max_store_bytes = 3000).await;
    let (k, mut c) = (key(), C::open(&url).await);
    let base = now() - 100;
    let mut ids = Vec::new();
    for i in 0..10u64 {
        let ev = make(&k, 1, base + i as i64, vec![], &"x".repeat(400));
        ids.push(ev["id"].as_str().unwrap().to_owned());
        assert!(c.publish(&ev).await.0);
    }
    let got = c.req("a", vec![json!({ "kinds": [1] })]).await;
    assert!(got.len() < 10 && !got.is_empty(), "kept {}", got.len());
    // the newest are the ones kept
    assert_eq!(got[0]["id"], ids[9]);
    assert!(
        !ids[..2]
            .iter()
            .any(|old| got.iter().any(|e| e["id"] == *old)),
        "the oldest should have gone"
    );
    let huge = make(&k, 1, now(), vec![], &"y".repeat(5000));
    let (ok, why) = c.publish(&huge).await;
    assert!(!ok && why.contains("storage is full"), "{why}");
    r.stop().await;
}

#[tokio::test]
async fn an_admin_ban_survives_a_crash_and_a_restart_and_nothing_accepts_odd_hex() {
    let dir = tempfile::tempdir().unwrap();
    let k = key();
    let ev = make(&k, 1, now(), vec![], "to be banned");
    let id = ev["id"].as_str().unwrap().to_owned();
    {
        let mut c = Config::default();
        c.server.listen = "127.0.0.1:0".into();
        c.server.data_dir = dir.path().into();
        c.nostr.enabled = true;
        c.nostr.events_per_minute_per_ip = 0;
        let r = start(c).await.unwrap();
        let mut cl = C::open(&format!("ws://{}", r.addr)).await;
        assert!(cl.publish(&ev).await.0);
        r.relay().unwrap().nostr().unwrap().ban_event(&id).unwrap();
        // no graceful stop: the log must already say so
        let log = std::fs::read_to_string(dir.path().join("nostr-events.jsonl")).unwrap();
        assert!(
            log.contains(&format!("\"ban\":\"{id}\"")),
            "the ban was not written down"
        );
        std::mem::forget(r); // as after a crash: no compaction at shutdown
    }
    let mut c = Config::default();
    c.server.listen = "127.0.0.1:0".into();
    c.server.data_dir = dir.path().into();
    c.nostr.enabled = true;
    c.nostr.events_per_minute_per_ip = 0;
    let r = start(c).await.unwrap();
    let mut cl = C::open(&format!("ws://{}", r.addr)).await;
    assert!(
        cl.req("a", vec![json!({ "ids": [id] })]).await.is_empty(),
        "a banned event must not come back after a restart"
    );
    let (ok, why) = cl.publish(&ev).await;
    assert!(!ok && why.starts_with("blocked"), "{why}");
    // a leading + (which `from_str_radix` accepts) is not hex
    let mut odd = make(&key(), 1, now(), vec![], "odd");
    let id_odd = odd["id"].as_str().unwrap().to_owned();
    odd["id"] = json!(format!("+{}", &id_odd[1..]));
    assert!(!cl.publish(&odd).await.0);
    r.stop().await;
}

#[tokio::test]
async fn search_finds_words_in_any_case_and_ignores_extensions() {
    let (r, url, _d) = server(|_| {}).await;
    let (k, mut c) = (key(), C::open(&url).await);
    for (i, text) in ["The quick brown fox", "a lazy dog", "QUICK thinking"]
        .iter()
        .enumerate()
    {
        assert!(
            c.publish(&make(&k, 1, now() - i as i64, vec![], text))
                .await
                .0
        );
    }
    let found = |v: Vec<Value>| -> Vec<String> {
        v.iter()
            .map(|e| e["content"].as_str().unwrap().to_owned())
            .collect()
    };
    let one = found(c.req("s1", vec![json!({ "search": "quick" })]).await);
    assert_eq!(
        one,
        ["The quick brown fox", "QUICK thinking"],
        "newest first, any case"
    );
    let two = found(c.req("s2", vec![json!({ "search": "QUICK fox" })]).await);
    assert_eq!(two, ["The quick brown fox"], "every word must appear");
    // `key:value` extensions (NIP-50) are not words
    let ext = found(
        c.req("s3", vec![json!({ "search": "lazy include:spam" })])
            .await,
    );
    assert_eq!(ext, ["a lazy dog"]);
    assert!(found(c.req("s4", vec![json!({ "search": "nothing here" })]).await).is_empty());
    // the information document says so
    assert!(r.relay().is_some());
    r.stop().await;
}

#[tokio::test]
async fn a_protected_event_is_only_taken_from_its_signed_in_author() {
    let (r, url, _d) = server(|c| c.nostr.relay_url = "ws://x".into()).await;
    let (author, other, mut c) = (key(), key(), C::open(&url).await);
    let challenge = c.next().await; // offered at connect, so a client can sign in when it needs to
    assert_eq!(challenge[0], "AUTH");
    let protected = make(&author, 1, now(), vec![vec!["-"]], "mine only");
    assert!(c.publish(&protected).await.1.starts_with("auth-required"));
    let sign_in = |k: &SigningKey| {
        make(
            k,
            22242,
            now(),
            vec![
                vec!["relay", "ws://x"],
                vec!["challenge", challenge[1].as_str().unwrap()],
            ],
            "",
        )
    };
    // signed in as someone else: still refused
    c.send(json!(["AUTH", sign_in(&other)])).await;
    assert_eq!(c.next().await[2], true);
    assert!(c.publish(&protected).await.1.starts_with("auth-required"));
    // a fresh connection signed in as the author: taken
    let mut c2 = C::open(&url).await;
    let ch2 = c2.next().await;
    let proof = make(
        &author,
        22242,
        now(),
        vec![
            vec!["relay", "ws://x"],
            vec!["challenge", ch2[1].as_str().unwrap()],
        ],
        "",
    );
    c2.send(json!(["AUTH", proof])).await;
    assert_eq!(c2.next().await[2], true);
    assert!(c2.publish(&protected).await.0);
    // an ordinary event needs none of this
    assert!(c.publish(&make(&other, 1, now(), vec![], "plain")).await.0);
    r.stop().await;
}

#[tokio::test]
async fn a_request_to_vanish_removes_the_authors_events_and_refuses_older_ones_even_after_a_restart(
) {
    let dir = tempfile::tempdir().unwrap();
    let config = |c: &mut Config| {
        c.server.data_dir = dir.path().into();
        c.nostr.relay_url = "ws://x".into();
    };
    let (r, url, _d) = server(config).await;
    let (alice, bob, mut c) = (key(), key(), C::open(&url).await);
    let (a1, a2) = (
        make(&alice, 1, now() - 100, vec![], "alice one"),
        make(&alice, 1, now() - 50, vec![], "alice two"),
    );
    let b1 = make(&bob, 1, now() - 80, vec![], "bob");
    for e in [&a1, &a2, &b1] {
        assert!(c.publish(e).await.0);
    }
    // addressed to another relay: refused
    let elsewhere = make(
        &alice,
        62,
        now(),
        vec![vec!["relay", "wss://other.example"]],
        "",
    );
    assert!(c.publish(&elsewhere).await.1.starts_with("invalid"));
    assert_eq!(
        c.req("s", vec![json!({ "authors": [pk(&alice)] })])
            .await
            .len(),
        2
    );
    // addressed to this relay
    let vanish = make(&alice, 62, now(), vec![vec!["relay", "ws://x"]], "");
    assert!(c.publish(&vanish).await.0);
    let left = c
        .req("s2", vec![json!({ "authors": [pk(&alice)], "kinds": [1] })])
        .await;
    assert!(left.is_empty(), "alice's events are gone");
    assert_eq!(
        c.req("s3", vec![json!({ "authors": [pk(&bob)] })])
            .await
            .len(),
        1,
        "bob's stay"
    );
    // an older event of hers is refused
    assert!(c
        .publish(&make(&alice, 1, now() - 10, vec![], "late"))
        .await
        .1
        .starts_with("blocked"));
    r.stop().await;
    // still true after a restart (the request is in the log)
    let (r2, url2, _d2) = server(config).await;
    let mut c2 = C::open(&url2).await;
    assert!(c2
        .req("s", vec![json!({ "authors": [pk(&alice)], "kinds": [1] })])
        .await
        .is_empty());
    assert!(c2
        .publish(&make(&alice, 1, now() - 20, vec![], "again"))
        .await
        .1
        .starts_with("blocked"));
    assert_eq!(
        c2.req("s", vec![json!({ "authors": [pk(&bob)] })])
            .await
            .len(),
        1
    );
    r2.stop().await;
}
