//! What the server writes to disk: checked, optionally encrypted, and safe against a wrong key.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use futures_util::{SinkExt, StreamExt};
use k256::schnorr::SigningKey;
use privcord_server::nostr::event::hex;
use privcord_server::{config::Config, start};
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
}

fn key_file(dir: &std::path::Path, byte: u8) -> String {
    let f = dir.join(format!("key-{byte}"));
    std::fs::write(&f, format!("{}\n", format!("{byte:02x}").repeat(32))).unwrap();
    f.display().to_string()
}

fn config(data: &std::path::Path, tweak: impl FnOnce(&mut Config)) -> Config {
    let mut c = Config::default();
    c.server.listen = "127.0.0.1:0".into();
    c.server.data_dir = data.into();
    c.nostr.enabled = true;
    c.nostr.events_per_minute_per_ip = 0;
    c.relay.rate_limit = 1_000_000;
    tweak(&mut c);
    c
}

async fn publish_note(url: &str, text: &str) -> String {
    let mut c = C::open(url).await;
    let ev = make(&key(), 1, now(), vec![], text);
    let id = ev["id"].as_str().unwrap().to_owned();
    c.send(json!(["EVENT", ev])).await;
    loop {
        let m = c.next().await;
        if m[0] == "OK" && m[1] == id.as_str() {
            assert_eq!(m[2], true, "{m}");
            return id;
        }
    }
}

async fn stored(url: &str, ids: &[String]) -> usize {
    let mut c = C::open(url).await;
    c.send(json!(["REQ", "q", { "ids": ids }])).await;
    let mut n = 0;
    loop {
        let m = c.next().await;
        if m[0] == "EVENT" {
            n += 1;
        } else if m[0] == "EOSE" {
            return n;
        }
    }
}

#[tokio::test]
async fn an_encrypted_log_hides_events_survives_a_restart_and_needs_its_key() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let key = key_file(dir.path(), 0x11);
    let enc = |c: &mut Config| {
        c.storage.encrypt = true;
        c.storage.key_file = key.clone();
    };
    let r = start(config(&data, enc)).await.unwrap();
    let url = format!("ws://{}", r.addr);
    let id = publish_note(&url, "a note that must not be readable on disk").await;
    r.stop().await;
    let log = std::fs::read_to_string(data.join("nostr-events.jsonl")).unwrap();
    assert!(log.lines().all(|l| l.starts_with("e1 ")), "{log}");
    assert!(
        !log.contains("must not be readable") && !log.contains(&id),
        "the log is ciphertext"
    );
    // the right key: the event is back
    let r = start(config(&data, enc)).await.unwrap();
    assert_eq!(
        stored(&format!("ws://{}", r.addr), std::slice::from_ref(&id)).await,
        1
    );
    r.stop().await;
    // no key: refused, and nothing changed
    let before = std::fs::read(data.join("nostr-events.jsonl")).unwrap();
    let no_key = start(config(&data, |_| {})).await.unwrap_err();
    assert!(no_key.contains("encrypted"), "{no_key}");
    // another key: refused, and nothing changed (not a log of damage followed by an empty rewrite)
    let other = key_file(dir.path(), 0x22);
    let wrong = start(config(&data, |c| {
        c.storage.encrypt = true;
        c.storage.key_file = other.clone();
    }))
    .await
    .unwrap_err();
    assert!(wrong.contains("storage key"), "{wrong}");
    assert_eq!(
        std::fs::read(data.join("nostr-events.jsonl")).unwrap(),
        before
    );
}

#[tokio::test]
async fn a_changed_byte_in_the_middle_of_the_log_is_noticed_kept_aside_and_the_rest_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let r = start(config(&data, |_| {})).await.unwrap();
    let url = format!("ws://{}", r.addr);
    let mut ids = Vec::new();
    for i in 0..5 {
        ids.push(publish_note(&url, &format!("note {i}")).await);
    }
    r.stop().await;
    let path = data.join("nostr-events.jsonl");
    let log = std::fs::read_to_string(&path).unwrap();
    assert!(
        log.lines().all(|l| l.starts_with("c1 ")),
        "checked by default: {log}"
    );
    // flip one letter inside the third record
    let mut lines: Vec<String> = log.lines().map(str::to_owned).collect();
    let middle = lines.iter().position(|l| l.contains("note 2")).unwrap();
    lines[middle] = lines[middle].replacen("note 2", "note X", 1);
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let r = start(config(&data, |_| {})).await.unwrap();
    let url = format!("ws://{}", r.addr);
    assert_eq!(
        stored(&url, &ids).await,
        4,
        "the damaged record is skipped, the others are kept"
    );
    let damaged = std::fs::read_dir(&data)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains(".damaged-"));
    assert!(damaged, "a copy of the damaged log is kept for a look");
    assert_eq!(
        r.relay()
            .unwrap()
            .nostr()
            .unwrap()
            .store()
            .unwrap()
            .corrupt_lines(),
        1
    );
    r.stop().await;
}

#[tokio::test]
async fn a_log_written_by_an_earlier_version_is_read_and_rewritten_in_the_new_shape() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let key = key_file(dir.path(), 0x33);
    // as the earlier versions wrote it: plain lines, no check
    let r = start(config(&data, |c| c.storage.checksums = false))
        .await
        .unwrap();
    let id = publish_note(&format!("ws://{}", r.addr), "written long ago").await;
    r.stop().await;
    assert!(std::fs::read_to_string(data.join("nostr-events.jsonl"))
        .unwrap()
        .starts_with('{'));
    // started with encryption: the event is there, and the file is ciphertext from then on
    let r = start(config(&data, |c| {
        c.storage.encrypt = true;
        c.storage.key_file = key.clone();
    }))
    .await
    .unwrap();
    assert_eq!(stored(&format!("ws://{}", r.addr), &[id]).await, 1);
    r.stop().await;
    let log = std::fs::read_to_string(data.join("nostr-events.jsonl")).unwrap();
    assert!(
        log.lines().all(|l| l.starts_with("e1 ")) && !log.contains("written long ago"),
        "{log}"
    );
}

#[tokio::test]
async fn the_turn_secret_and_the_offline_queue_are_encrypted_too() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let key = key_file(dir.path(), 0x44);
    let secret = privcord_server::turn_role::load_or_make_secret(dir.path()).unwrap();
    let codec =
        privcord_server::record::Codec::from_config(&privcord_server::config::StorageConfig {
            encrypt: true,
            key_file: key.clone(),
            ..Default::default()
        })
        .unwrap();
    // an older plain secret is read and written again encrypted, and stays the same secret
    let again = privcord_server::turn_role::load_or_make_secret_with(dir.path(), &codec).unwrap();
    assert_eq!(secret, again);
    let file = std::fs::read_to_string(dir.path().join("turn-secret")).unwrap();
    assert!(file.starts_with("e1 ") && !file.contains(std::str::from_utf8(&secret).unwrap()));
    // the queue: a stored message is ciphertext on disk
    let mut c = config(&data, |c| {
        c.storage.encrypt = true;
        c.storage.key_file = key.clone();
        c.relay.store_and_forward = true;
    });
    c.nostr.enabled = false;
    let blob = "A".repeat(64);
    let mut store =
        privcord_server::store::Store::open_with(&data, std::sync::Arc::new(codec)).unwrap();
    assert!(store.append_put("box", "id1", &blob, 1, i64::MAX / 2));
    drop(store);
    let text = std::fs::read_to_string(data.join("queue.jsonl")).unwrap();
    assert!(
        text.starts_with("e1 ") && !text.contains(&blob) && !text.contains("box"),
        "{text}"
    );
    let _ = c; // (the settings above are what a server would run with)
}
