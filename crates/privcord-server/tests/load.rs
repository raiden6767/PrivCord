//! Load check (run on purpose): `cargo test -p privcord-server --release --test load -- --ignored --nocapture`.
//! Many clients each listen on their own mailbox while as many others post to them; every message must arrive.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use futures_util::{SinkExt, StreamExt};
use privcord_server::{config::Config, start};
use serde_json::json;
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

#[tokio::test(flavor = "multi_thread")]
#[ignore = "load check; run on purpose"]
async fn thousands_of_clients_all_get_their_messages() {
    let pairs: usize = std::env::var("LOAD_PAIRS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);
    let per: usize = std::env::var("LOAD_MSGS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let dir = tempfile::tempdir().unwrap();
    let mut c = Config::default();
    c.server.listen = "127.0.0.1:0".into();
    c.server.data_dir = dir.path().into();
    c.relay.rate_limit = 1_000_000;
    c.relay.max_conns_per_ip = 100_000;
    c.relay.max_per_mailbox_per_ip = 100_000;
    c.relay.max_queued = 10_000_000;
    // LOAD_URL points the same load at another relay (for comparing), otherwise the Rust one is started here.
    let external = std::env::var("LOAD_URL").ok();
    let r = if external.is_none() {
        Some(start(c).await.unwrap())
    } else {
        None
    };
    let url = external.unwrap_or_else(|| format!("ws://{}", r.as_ref().unwrap().addr));
    let blob = "A".repeat(2048);

    let mut listeners = Vec::new();
    for i in 0..pairs {
        let (mut ws, _) = connect_async(&url).await.unwrap();
        ws.send(Message::Text(
            json!({ "op": "sub", "mailbox": format!("mb{i}") })
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        listeners.push(tokio::spawn(async move {
            let mut got = 0usize;
            let mut lat: Vec<u64> = Vec::new();
            while got < per {
                let Ok(Some(Ok(Message::Text(t)))) =
                    tokio::time::timeout(Duration::from_secs(30), ws.next()).await
                else {
                    break;
                };
                if t.contains("\"event\"") {
                    got += 1;
                    let sent: u128 = t
                        .split("\"id\":\"")
                        .nth(1)
                        .and_then(|s| s.split('"').next())
                        .and_then(|s| s.rsplit('-').next())
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_micros();
                    lat.push(now.saturating_sub(sent) as u64);
                }
            }
            (got, lat)
        }));
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    let start_at = Instant::now();
    let mut senders = Vec::new();
    for i in 0..pairs {
        let (url, blob) = (url.clone(), blob.clone());
        senders.push(tokio::spawn(async move {
            let (mut ws, _) = connect_async(&url).await.unwrap();
            for m in 0..per {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros();
                let id = format!("m{m}-{now}");
                ws.send(Message::Text(json!({ "op": "put", "mailbox": format!("mb{i}"), "id": id, "blob": blob, "ttl": 60, "kind": "sig" }).to_string().into())).await.unwrap();
            }
            // Keep the socket open until the relay has taken everything.
            let _ = tokio::time::timeout(Duration::from_secs(5), async { for _ in 0..per { ws.next().await; } }).await;
        }));
    }
    for s in senders {
        s.await.unwrap();
    }
    let (mut total, mut lat) = (0usize, Vec::<u64>::new());
    for l in listeners {
        let (g, w) = l.await.unwrap();
        total += g;
        lat.extend(w);
    }
    let took = start_at.elapsed();
    lat.sort_unstable();
    let at = |q: f64| {
        Duration::from_micros(
            lat.get(((lat.len() as f64 * q) as usize).min(lat.len().saturating_sub(1)))
                .copied()
                .unwrap_or(0),
        )
    };
    let want = pairs * per;
    println!("{pairs} listeners + {pairs} senders, {want} messages of 2 KiB: {total} delivered in {took:?} ({:.0} msg/s), delivery p50 {:?} p99 {:?} max {:?}", want as f64 / took.as_secs_f64(), at(0.5), at(0.99), at(1.0));
    assert_eq!(total, want, "every message must arrive");
    if let Some(r) = r {
        r.stop().await;
    }
}
