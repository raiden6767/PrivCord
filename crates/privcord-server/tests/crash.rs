//! The server program itself, killed and started again.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use futures_util::{SinkExt, StreamExt};
use k256::schnorr::SigningKey;
use privcord_server::nostr::event::hex;
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

// Crash safety: the real server program is killed (SIGKILL, no clean shutdown) in the middle of a stream of events, and
// started again. Everything it confirmed must still be there, and a half-written last line must not swallow what comes next.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

struct Proc {
    child: Child,
    url: String,
}

impl Proc {
    fn start(dir: &std::path::Path) -> Self {
        let conf = dir.join("server.toml");
        std::fs::write(
            &conf,
            format!(
                "[server]\nlisten = \"127.0.0.1:0\"\ndata_dir = '{}'\n[relay]\nrate_limit = 1000000\n[nostr]\nenabled = true\ndurable = true\nevents_per_minute_per_ip = 0\n",
                dir.join("data").display()
            ),
        )
        .unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_privcord-server"))
            .args(["--config", conf.to_str().unwrap(), "serve"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .unwrap();
        let addr = line
            .split("listening on ")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .unwrap_or_else(|| panic!("no address in {line:?}"))
            .to_owned();
        Self {
            child,
            url: format!("ws://{addr}"),
        }
    }

    /// Dies at once, with no chance to flush or say goodbye.
    fn kill(mut self) {
        self.child.kill().unwrap();
        let _ = self.child.wait();
    }
}

async fn present(url: &str, ids: &[String]) -> std::collections::HashSet<String> {
    let mut c = C::open(url).await;
    let mut found = std::collections::HashSet::new();
    for (n, chunk) in ids.chunks(200).enumerate() {
        let sub = format!("q{n}");
        c.send(json!(["REQ", sub, { "ids": chunk }])).await;
        loop {
            let m = c.next().await;
            if m[0] == "EVENT" {
                found.insert(m[2]["id"].as_str().unwrap().to_owned());
            } else if m[0] == "EOSE" {
                break;
            }
        }
        c.send(json!(["CLOSE", sub])).await;
    }
    found
}

#[tokio::test]
async fn everything_the_server_confirmed_survives_a_kill_and_a_torn_last_line_is_cut() {
    let dir = tempfile::tempdir().unwrap();
    let k = key();
    // 1. A stream of events, killed in the middle: only what was confirmed counts.
    let p = Proc::start(dir.path());
    let mut c = C::open(&p.url).await;
    let mut sent = Vec::new();
    for i in 0..600 {
        let e = make(&k, 1, now() - i, vec![], &format!("event number {i}"));
        sent.push(e["id"].as_str().unwrap().to_owned());
        c.send(json!(["EVENT", e])).await;
    }
    let mut confirmed = Vec::new();
    while confirmed.len() < 250 {
        let m = c.next().await;
        if m[0] == "OK" && m[2] == true {
            confirmed.push(m[1].as_str().unwrap().to_owned());
        }
    }
    p.kill();
    drop(c);
    // 2. The kill left half a line at the end of the log (as a power cut can).
    let log = dir.path().join("data/nostr-events.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .unwrap()
        .write_all(b"{\"id\":\"deadbeef\",\"pubkey\":\"")
        .unwrap();
    // 3. Started again: every confirmed event is there.
    let p = Proc::start(dir.path());
    let found = present(&p.url, &confirmed).await;
    let lost: Vec<_> = confirmed.iter().filter(|id| !found.contains(*id)).collect();
    assert!(lost.is_empty(), "{} confirmed events lost", lost.len());
    // 4. A new event after the torn line is kept too (it was not glued onto the half line).
    let mut c = C::open(&p.url).await;
    let after = make(&k, 1, now() + 1, vec![], "after the torn line");
    let after_id = after["id"].as_str().unwrap().to_owned();
    c.send(json!(["EVENT", after])).await;
    loop {
        let m = c.next().await;
        if m[0] == "OK" && m[1] == after_id.as_str() {
            assert_eq!(m[2], true, "{m}");
            break;
        }
    }
    p.kill();
    drop(c);
    let p = Proc::start(dir.path());
    assert!(
        present(&p.url, &[after_id]).await.len() == 1,
        "the event after the torn line was lost"
    );
    p.kill();
    let _ = sent;
}
