#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
//! The limits an operator sets: the monthly traffic cap closes the door politely, traffic is counted, and a speed limit holds.

use futures_util::{SinkExt, StreamExt};
use privcord_server::config::Config;
use std::time::{Duration, Instant};
use tokio_tungstenite::{connect_async, tungstenite::Message};

fn base(dir: &std::path::Path) -> Config {
    let mut cfg = Config::default();
    cfg.server.listen = "127.0.0.1:0".into();
    cfg.server.data_dir = dir.into();
    cfg.relay.store_and_forward = false;
    cfg
}

#[tokio::test]
async fn at_the_monthly_cap_new_connections_get_503_with_a_long_retry_after() {
    let dir = tempfile::tempdir().unwrap();
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let month: String = privcord_server::health::civil(secs)
        .0
        .chars()
        .take(7)
        .collect();
    std::fs::write(
        dir.path().join("usage.json"),
        format!(
            r#"{{"month":"{month}","web_in":2000000000,"web_out":0,"turn_in":0,"turn_out":0}}"#
        ),
    )
    .unwrap();
    let mut cfg = base(dir.path());
    cfg.limits.monthly_traffic_gb = 1;
    let r = privcord_server::start(cfg).await.unwrap();
    assert!(r.usage().over_cap());
    match connect_async(format!("ws://{}", r.addr)).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(resp.status().as_u16(), 503);
            let wait = resp.headers().get("retry-after").unwrap().to_str().unwrap();
            assert!(
                wait.parse::<u64>().unwrap() >= 300,
                "a short wait invites a reconnect storm"
            );
        }
        other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
    }
    // the plain web side still answers: an operator can see why
    r.stop().await;
}

#[tokio::test]
async fn traffic_is_counted_and_saved_and_a_server_without_a_cap_never_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let r = privcord_server::start(base(dir.path())).await.unwrap();
    let (mut ws, _) = connect_async(format!("ws://{}", r.addr)).await.unwrap();
    ws.send(Message::text(r#"["sub","m1",0]"#)).await.unwrap();
    // wait until the server has counted what it read
    let t = Instant::now();
    while r.usage().total() == 0 && t.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(r.usage().total() > 0);
    assert!(!r.usage().over_cap());
    drop(ws);
    r.stop().await;
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("usage.json")).unwrap())
            .unwrap();
    assert!(
        saved["web_in"].as_u64().unwrap() > 0,
        "saved at stop: {saved}"
    );
}

#[tokio::test]
async fn a_connection_speed_limit_slows_a_connection_down() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = base(dir.path());
    cfg.limits.connection_kbps = 160; // 20 kB/s each way
    let r = privcord_server::start(cfg).await.unwrap();
    let (mut ws, _) = connect_async(format!("ws://{}", r.addr)).await.unwrap();
    let filler = "x".repeat(5_000);
    let t = Instant::now();
    // 100 kB sent at 20 kB/s is five seconds of traffic, one second of it free
    for _ in 0..20 {
        ws.send(Message::text(format!(r#"["sub","{filler}",0]"#)))
            .await
            .unwrap();
    }
    while r.usage().total() < 100_000 && t.elapsed() < Duration::from_secs(30) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        t.elapsed() >= Duration::from_secs(3),
        "no slowing: {:?}",
        t.elapsed()
    );
    let _ = ws.close(None).await;
    let _ = ws.next().await;
    r.stop().await;
}
