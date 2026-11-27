#![cfg(feature = "turn")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! The TURN role, driven by a real TURN client over UDP on this machine.

use privcord_server::{config::Config, turn_role};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use turn::client::{Client, ClientConfig};
use webrtc_util::Conn;

async fn server(
    allow_private: bool,
    users: Vec<String>,
) -> (privcord_server::Running, u16, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.server.listen = "127.0.0.1:0".into();
    cfg.server.data_dir = dir.path().into();
    cfg.relay.store_and_forward = false;
    cfg.turn.enabled = true;
    cfg.turn.listen = "127.0.0.1:0".into();
    cfg.turn.public_ip = "127.0.0.1".into();
    cfg.turn.min_port = 50000;
    cfg.turn.max_port = 50100;
    cfg.turn.allow_private_peers = allow_private;
    cfg.turn.users = users;
    let secret = turn_role::load_or_make_secret(dir.path()).unwrap();
    let _ = secret;
    let running = privcord_server::start(cfg).await.unwrap();
    let port = running.turn_port().unwrap();
    (running, port, dir)
}

async fn client(port: u16, user: &str, pass: &str) -> Client {
    let conn = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let c = Client::new(ClientConfig {
        stun_serv_addr: String::new(),
        turn_serv_addr: format!("127.0.0.1:{port}"),
        username: user.into(),
        password: pass.into(),
        realm: "privcord".into(),
        software: String::new(),
        rto_in_ms: 0,
        conn,
        vnet: None,
    })
    .await
    .unwrap();
    c.listen().await.unwrap();
    c
}

#[tokio::test]
async fn a_phone_with_a_handed_out_credential_can_relay_to_a_peer() {
    let (running, port, dir) = server(true, vec![]).await;
    let secret = turn_role::load_or_make_secret(dir.path()).unwrap();
    let (user, pass) = turn_role::issue(&secret, now(), 3600);
    let c = client(port, &user, &pass).await;
    let relay = c
        .allocate()
        .await
        .expect("allocation with a good credential");
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    relay
        .send_to(b"hello through the relay", peer.local_addr().unwrap())
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    let (n, from) = tokio::time::timeout(Duration::from_secs(3), peer.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], b"hello through the relay");
    assert_eq!(from, relay.local_addr().unwrap());
    c.close().await.unwrap();
    running.stop().await;
}

#[tokio::test]
async fn expired_and_forged_credentials_are_refused() {
    let (running, port, dir) = server(true, vec![]).await;
    let secret = turn_role::load_or_make_secret(dir.path()).unwrap();
    let (user, pass) = turn_role::issue(&secret, now() - 7200, 3600);
    assert!(
        client(port, &user, &pass).await.allocate().await.is_err(),
        "expired"
    );
    let (user, _) = turn_role::issue(&secret, now(), 3600);
    assert!(
        client(port, &user, "not-the-password")
            .await
            .allocate()
            .await
            .is_err(),
        "forged"
    );
    running.stop().await;
}

#[tokio::test]
async fn a_fixed_account_from_the_settings_works() {
    let (running, port, _dir) = server(true, vec!["alice:s3cret".into()]).await;
    assert!(client(port, "alice", "s3cret")
        .await
        .allocate()
        .await
        .is_ok());
    assert!(client(port, "alice", "wrong")
        .await
        .allocate()
        .await
        .is_err());
    running.stop().await;
}

#[tokio::test]
async fn relaying_to_this_machine_is_refused_unless_allowed() {
    let (running, port, dir) = server(false, vec![]).await;
    let secret = turn_role::load_or_make_secret(dir.path()).unwrap();
    let (user, pass) = turn_role::issue(&secret, now(), 3600);
    let c = client(port, &user, &pass).await;
    let relay = c.allocate().await.unwrap();
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    relay
        .send_to(b"should not arrive", peer.local_addr().unwrap())
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    assert!(
        tokio::time::timeout(Duration::from_millis(700), peer.recv_from(&mut buf))
            .await
            .is_err()
    );
    running.stop().await;
}

#[tokio::test]
async fn the_credentials_page_hands_out_working_credentials() {
    let (running, port, _dir) = server(true, vec![]).await;
    let body = http_get(running.addr, "/turn").await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let c = client(
        port,
        v["username"].as_str().unwrap(),
        v["credential"].as_str().unwrap(),
    )
    .await;
    assert!(c.allocate().await.is_ok());
    let found = http_get(running.addr, "/.well-known/privcord-server").await;
    assert!(found.contains("\"turn\""), "{found}");
    running.stop().await;
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn http_get(addr: std::net::SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out.split("\r\n\r\n").nth(1).unwrap_or("").to_owned()
}
