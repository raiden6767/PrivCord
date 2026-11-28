//! NIP-77: set reconciliation with a real negentropy client against the real server.
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

use negentropy::{Id as NegId, Negentropy, NegentropyStorageVector};

/// Runs a whole sync as a client holding `mine` (ids with their times): returns (what I have that the relay lacks, what the
/// relay has that I lack), as hex ids, and how many rounds it took.
async fn sync(
    c: &mut C,
    filter: Value,
    mine: &[(u64, String)],
) -> (Vec<String>, Vec<String>, usize) {
    let mut set = NegentropyStorageVector::new();
    for (t, id) in mine {
        let mut raw = [0u8; 32];
        raw.copy_from_slice(&hex_to_bytes(id));
        set.insert(*t, NegId::from_byte_array(raw)).unwrap();
    }
    set.seal().unwrap();
    let mut ng = Negentropy::owned(set, 0).unwrap();
    let first = ng.initiate().unwrap();
    c.send(json!(["NEG-OPEN", "sync", filter, hex(&first)]))
        .await;
    let (mut have, mut need) = (Vec::new(), Vec::new());
    let mut rounds = 0;
    loop {
        let m = c.next().await;
        if m[0] == "NEG-ERR" {
            panic!("{m}");
        }
        assert_eq!(m[0], "NEG-MSG", "{m}");
        rounds += 1;
        let bytes = hex_to_bytes(m[2].as_str().unwrap());
        match ng.reconcile_with_ids(&bytes, &mut have, &mut need).unwrap() {
            Some(next) => c.send(json!(["NEG-MSG", "sync", hex(&next)])).await,
            None => break,
        }
        assert!(rounds < 50, "the sync does not finish");
    }
    c.send(json!(["NEG-CLOSE", "sync"])).await;
    let name = |v: Vec<NegId>| {
        let mut out: Vec<String> = v.iter().map(|i| hex(&i[..])).collect();
        out.sort();
        out
    };
    (name(have), name(need), rounds)
}

fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[tokio::test]
async fn a_client_finds_exactly_what_differs_and_then_fetches_it() {
    let (r, url, _d) = server(|_| {}).await;
    let (k, mut c) = (key(), C::open(&url).await);
    let mut relay_has = Vec::new();
    for i in 0..30 {
        let e = make(&k, 1, now() - i, vec![], &format!("note {i}"));
        assert!(c.publish(&e).await.0);
        relay_has.push((
            e["created_at"].as_u64().unwrap(),
            e["id"].as_str().unwrap().to_owned(),
        ));
    }
    // The client has the older twenty, plus two events the relay has never seen.
    let mut mine: Vec<(u64, String)> = relay_has[10..].to_vec();
    let extra = [
        make(&k, 1, now() - 500, vec![], "only here"),
        make(&k, 1, now() - 501, vec![], "and here"),
    ];
    for e in &extra {
        mine.push((
            e["created_at"].as_u64().unwrap(),
            e["id"].as_str().unwrap().to_owned(),
        ));
    }
    let (have, need, _) = sync(&mut c, json!({ "kinds": [1] }), &mine).await;
    let mut want_need: Vec<String> = relay_has[..10].iter().map(|x| x.1.clone()).collect();
    want_need.sort();
    let mut want_have: Vec<String> = extra
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    want_have.sort();
    assert_eq!(need, want_need, "what the client lacks");
    assert_eq!(have, want_have, "what the relay lacks");
    // and the missing ones can be fetched with a plain REQ
    let got = c.req("f", vec![json!({ "ids": need })]).await;
    assert_eq!(got.len(), 10);
    r.stop().await;
}

#[tokio::test]
async fn a_big_set_with_a_few_differences_takes_a_few_rounds_not_a_transfer_of_everything() {
    let (r, url, _d) = server(|c| c.nostr.max_limit = 100_000).await;
    let (k, mut c) = (key(), C::open(&url).await);
    let mut all = Vec::new();
    for i in 0..1500u64 {
        let e = make(
            &k,
            1,
            now() - i64::try_from(i).unwrap(),
            vec![],
            &format!("n{i}"),
        );
        assert!(c.publish(&e).await.0);
        all.push((
            e["created_at"].as_u64().unwrap(),
            e["id"].as_str().unwrap().to_owned(),
        ));
    }
    // The client lacks three scattered events.
    let missing = [100usize, 700, 1400];
    let mine: Vec<(u64, String)> = all
        .iter()
        .enumerate()
        .filter(|(i, _)| !missing.contains(i))
        .map(|(_, x)| x.clone())
        .collect();
    let (have, need, rounds) = sync(&mut c, json!({ "kinds": [1] }), &mine).await;
    assert!(have.is_empty());
    let mut want: Vec<String> = missing.iter().map(|i| all[*i].1.clone()).collect();
    want.sort();
    assert_eq!(need, want);
    assert!(rounds <= 6, "{rounds} rounds");
    r.stop().await;
}

#[tokio::test]
async fn the_relay_refuses_what_it_should_in_words() {
    let (r, url, _d) = server(|c| {
        c.nostr.max_negentropy_items = 5;
        c.nostr.max_negentropy_sessions = 1;
        c.nostr.auth_required = false;
    })
    .await;
    let (k, mut c) = (key(), C::open(&url).await);
    for i in 0..8 {
        assert!(c.publish(&make(&k, 1, now() - i, vec![], "x")).await.0);
    }
    // a filter that matches more than the limit
    c.send(json!(["NEG-OPEN", "big", { "kinds": [1] }, "6100"]))
        .await;
    let m = c.next().await;
    assert_eq!(
        (m[0].as_str(), m[2].as_str().unwrap().starts_with("blocked")),
        (Some("NEG-ERR"), true),
        "{m}"
    );
    // a message for a sync that is not open
    c.send(json!(["NEG-MSG", "nope", "6100"])).await;
    assert!(c.next().await[2].as_str().unwrap().starts_with("closed"));
    // not hex
    c.send(json!(["NEG-OPEN", "bad", { "kinds": [7] }, "zz"]))
        .await;
    assert!(c.next().await[2].as_str().unwrap().starts_with("invalid"));
    // one at a time on this server
    c.send(json!(["NEG-OPEN", "one", { "kinds": [7] }, "6100"]))
        .await;
    assert_eq!(c.next().await[0], "NEG-MSG");
    c.send(json!(["NEG-OPEN", "two", { "kinds": [7] }, "6100"]))
        .await;
    assert!(c.next().await[2].as_str().unwrap().starts_with("blocked"));
    // closing frees the place
    c.send(json!(["NEG-CLOSE", "one"])).await;
    c.send(json!(["NEG-OPEN", "two", { "kinds": [7] }, "6100"]))
        .await;
    assert_eq!(c.next().await[0], "NEG-MSG");
    r.stop().await;
}
