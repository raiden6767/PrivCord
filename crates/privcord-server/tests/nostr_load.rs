//! Nostr load check (run on purpose): `cargo test -p privcord-server --release --test nostr_load -- --ignored --nocapture`.
//! `NOSTR_URL=ws://host:port` aims the same load at another relay; otherwise this server is started here.
//!   fan-out: many listeners each on their own `#t` room, as many publishers posting live (ephemeral) events to them
//!   store:   publishers post regular events that must be kept, then one client reads them back
//!   latency: one event at a time, publisher to listener, on an otherwise idle relay
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::panic
)]

use futures_util::{SinkExt, StreamExt};
use k256::schnorr::SigningKey;
use privcord_server::nostr::event::hex;
use privcord_server::{config::Config, start};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

/// Connects with Nagle's algorithm off, as the apps' libraries and Node's `ws` do.
async fn connect_async(
    url: &str,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    ),
    tokio_tungstenite::tungstenite::Error,
> {
    tokio_tungstenite::connect_async_with_config(url, None, true).await
}

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

fn make(k: &SigningKey, kind: u64, tags: &[Vec<String>], content: &str) -> String {
    let pubkey = hex(&k.verifying_key().to_bytes());
    let created_at = now();
    let id: [u8; 32] = Sha256::digest(
        json!([0, pubkey, created_at, kind, tags, content])
            .to_string()
            .as_bytes(),
    )
    .into();
    let mut aux = [0u8; 32];
    getrandom::fill(&mut aux).unwrap();
    let sig = k.sign_raw(&id, &aux).unwrap();
    json!({ "id": hex(&id), "pubkey": pubkey, "created_at": created_at, "kind": kind, "tags": tags, "content": content, "sig": hex(&sig.to_bytes()) }).to_string()
}

fn env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn text(v: Value) -> Message {
    Message::Text(v.to_string().into())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "load check; run on purpose"]
async fn nostr_load() {
    let rooms = env("NL_ROOMS", 200);
    let per = env("NL_MSGS", 100);
    let stored = env("NL_STORED", 20_000);
    let dir = tempfile::tempdir().unwrap();
    let external = std::env::var("NOSTR_URL").ok();
    let server = if external.is_none() {
        let mut c = Config::default();
        c.server.listen = "127.0.0.1:0".into();
        c.server.data_dir = dir.path().into();
        c.relay.enabled = false;
        c.nostr.enabled = true;
        c.nostr.events_per_minute_per_ip = 0;
        c.relay.max_conns_per_ip = 100_000;
        Some(start(c).await.unwrap())
    } else {
        None
    };
    let url = external.unwrap_or_else(|| format!("ws://{}", server.as_ref().unwrap().addr));
    let run = format!("run-{}", now());

    // ---- fan-out ----
    let mut batches: Vec<Vec<String>> = Vec::new();
    let signer = key();
    for r in 0..rooms {
        let tags = vec![vec!["t".to_owned(), format!("{run}-room-{r}")]];
        batches.push(
            (0..per)
                .map(|i| make(&signer, 21733, &tags, &format!("{i}")))
                .collect(),
        );
    }
    let mut listeners = Vec::new();
    for r in 0..rooms {
        let (mut ws, _) = connect_async(&url).await.unwrap();
        ws.send(text(
            json!(["REQ", "s", { "kinds": [21733], "#t": [format!("{run}-room-{r}")] }]),
        ))
        .await
        .unwrap();
        listeners.push(tokio::spawn(async move {
            let mut got = 0;
            while got < per {
                let Ok(Some(Ok(Message::Text(t)))) =
                    tokio::time::timeout(Duration::from_secs(20), ws.next()).await
                else {
                    break;
                };
                got += usize::from(t.starts_with("[\"EVENT\""));
            }
            got
        }));
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let t0 = Instant::now();
    let mut senders = Vec::new();
    for batch in batches {
        let url = url.clone();
        senders.push(tokio::spawn(async move {
            let (mut ws, _) = connect_async(&url).await.unwrap();
            for ev in &batch {
                ws.send(Message::Text(format!("[\"EVENT\",{ev}]").into()))
                    .await
                    .unwrap();
            }
            let mut oks = 0;
            while oks < batch.len() {
                let Ok(Some(Ok(Message::Text(t)))) =
                    tokio::time::timeout(Duration::from_secs(20), ws.next()).await
                else {
                    break;
                };
                oks += usize::from(t.starts_with("[\"OK\""));
            }
        }));
    }
    // Timed until every listener has everything: some relays do not answer live events with OK, so waiting for the
    // publishers' OKs would measure their timeout instead.
    let mut total = 0;
    for l in listeners {
        total += l.await.unwrap();
    }
    let took = t0.elapsed();
    for s in senders {
        s.await.unwrap();
    }
    println!("fan-out: {rooms} listeners + {rooms} publishers, {} live events: {total} delivered in {took:?} ({:.0} deliveries/s)", rooms * per, total as f64 / took.as_secs_f64());

    // ---- store ----
    let writers = 20;
    let mut jobs = Vec::new();
    for _ in 0..writers {
        let k = key();
        let tags = vec![vec!["t".to_owned(), format!("{run}-store")]];
        jobs.push(
            (0..stored / writers)
                .map(|i| make(&k, 1, &tags, &format!("note {i} {}", "x".repeat(300))))
                .collect::<Vec<_>>(),
        );
    }
    let t0 = Instant::now();
    let mut tasks = Vec::new();
    for batch in jobs {
        let url = url.clone();
        tasks.push(tokio::spawn(async move {
            let (mut ws, _) = connect_async(&url).await.unwrap();
            for ev in &batch {
                ws.send(Message::Text(format!("[\"EVENT\",{ev}]").into()))
                    .await
                    .unwrap();
            }
            let mut accepted = 0;
            let mut seen = 0;
            while seen < batch.len() {
                let Ok(Some(Ok(Message::Text(t)))) =
                    tokio::time::timeout(Duration::from_secs(30), ws.next()).await
                else {
                    break;
                };
                if t.starts_with("[\"OK\"") {
                    seen += 1;
                    accepted += usize::from(t.contains(",true"));
                }
            }
            accepted
        }));
    }
    let mut accepted = 0;
    for t in tasks {
        accepted += t.await.unwrap();
    }
    let took = t0.elapsed();
    println!("store: {} events from {writers} connections: {accepted} accepted in {took:?} ({:.0} events/s)", stored / writers * writers, accepted as f64 / took.as_secs_f64());

    // read them back, and run many small queries
    let (mut ws, _) = connect_async(&url).await.unwrap();
    let t0 = Instant::now();
    let queries = 2000;
    for q in 0..queries {
        ws.send(text(
            json!(["REQ", format!("q{q}"), { "#t": [format!("{run}-store")], "limit": 50 }]),
        ))
        .await
        .unwrap();
        let mut n = 0;
        loop {
            let m = tokio::time::timeout(Duration::from_secs(20), ws.next()).await;
            let t = match m {
                Ok(Some(Ok(Message::Text(t)))) => t,
                Ok(Some(Ok(_))) => continue, // a keep-alive ping
                other => panic!("no answer: {other:?}"),
            };
            if t.starts_with("[\"EOSE\"") {
                break;
            }
            n += usize::from(t.starts_with("[\"EVENT\""));
        }
        assert!(n > 0 || accepted == 0, "a stored event must come back");
        ws.send(text(json!(["CLOSE", format!("q{q}")])))
            .await
            .unwrap();
    }
    let took = t0.elapsed();
    println!("query: {queries} sequential REQs of 50 events from {accepted} stored: {took:?} ({:.0} queries/s)", queries as f64 / took.as_secs_f64());

    // ---- latency ----
    let (mut sub, mut publ) = (
        connect_async(&url).await.unwrap().0,
        connect_async(&url).await.unwrap().0,
    );
    sub.send(text(
        json!(["REQ", "l", { "kinds": [21733], "#t": [format!("{run}-lat")] }]),
    ))
    .await
    .unwrap();
    loop {
        if let Some(Ok(Message::Text(t))) = sub.next().await {
            if t.starts_with("[\"EOSE\"") {
                break;
            }
        }
    }
    let k = key();
    let tags = vec![vec!["t".to_owned(), format!("{run}-lat")]];
    let events: Vec<String> = (0..300)
        .map(|i| make(&k, 21733, &tags, &format!("{i}")))
        .collect();
    let mut lat = Vec::new();
    for ev in events {
        let t0 = Instant::now();
        publ.send(Message::Text(format!("[\"EVENT\",{ev}]").into()))
            .await
            .unwrap();
        loop {
            if let Ok(Some(Ok(Message::Text(t)))) =
                tokio::time::timeout(Duration::from_secs(5), sub.next()).await
            {
                if t.starts_with("[\"EVENT\"") {
                    break;
                }
            } else {
                panic!("no delivery");
            }
        }
        lat.push(t0.elapsed().as_micros());
        while let Ok(Some(Ok(_))) =
            tokio::time::timeout(Duration::from_millis(0), publ.next()).await
        {}
    }
    lat.sort_unstable();
    println!("latency: one event at a time, publisher to listener: p50 {:.2} ms, p99 {:.2} ms, max {:.2} ms", lat[150] as f64 / 1000.0, lat[297] as f64 / 1000.0, lat[299] as f64 / 1000.0);
    if let Some(s) = server {
        s.stop().await;
    }
}
