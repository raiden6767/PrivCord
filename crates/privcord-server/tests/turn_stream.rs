#![cfg(feature = "turn")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! TURN over TCP and over TLS, driven by a real TURN client.

mod common;
use common::stream_conn::StreamConn;

use privcord_server::{config::Config, turn_role};
use rustls::pki_types::{CertificateDer, ServerName};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpStream, UdpSocket};
use turn::client::{Client, ClientConfig};
use webrtc_util::Conn;

async fn server() -> (
    privcord_server::Running,
    tempfile::TempDir,
    (u64, String, String),
) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.server.listen = "127.0.0.1:0".into();
    cfg.server.data_dir = dir.path().into();
    cfg.relay.store_and_forward = false;
    cfg.tls.self_signed = true;
    cfg.turn.enabled = true;
    cfg.turn.listen = "127.0.0.1:0".into();
    cfg.turn.tcp_listen = "127.0.0.1:0".into();
    cfg.turn.tls_listen = "127.0.0.1:0".into();
    cfg.turn.public_ip = "127.0.0.1".into();
    cfg.turn.min_port = 50200;
    cfg.turn.max_port = 50300;
    cfg.turn.allow_private_peers = true;
    let secret = turn_role::load_or_make_secret(dir.path()).unwrap();
    let r = privcord_server::start(cfg).await.unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let (user, pass) = turn_role::issue(&secret, now, 3600);
    (r, dir, (now, user, pass))
}

fn client_cfg(
    conn: Arc<dyn Conn + Send + Sync>,
    port: u16,
    user: &str,
    pass: &str,
) -> ClientConfig {
    ClientConfig {
        stun_serv_addr: String::new(),
        turn_serv_addr: format!("127.0.0.1:{port}"),
        username: user.into(),
        password: pass.into(),
        realm: "privcord".into(),
        software: String::new(),
        rto_in_ms: 0,
        conn,
        vnet: None,
    }
}

/// Relay a packet out through the client's allocation to a UDP peer, and one back in.
async fn relay_round_trip(client: &Client) {
    client.listen().await.unwrap();
    let relay = client.allocate().await.expect("allocation over a stream");
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    relay
        .send_to(b"out through the relay", peer.local_addr().unwrap())
        .await
        .unwrap();
    let mut buf = [0u8; 128];
    let (n, from) = tokio::time::timeout(Duration::from_secs(5), peer.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], b"out through the relay");
    // and back in: the peer answers to the relayed address
    peer.send_to(b"and back", from).await.unwrap();
    let mut back = [0u8; 128];
    let (n, _) = tokio::time::timeout(Duration::from_secs(5), relay.recv_from(&mut back))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&back[..n], b"and back");
    // a large packet, to be sure frames are cut right on a stream
    let big = vec![7u8; 1100];
    relay
        .send_to(&big, peer.local_addr().unwrap())
        .await
        .unwrap();
    let mut buf = [0u8; 2000];
    let (n, _) = tokio::time::timeout(Duration::from_secs(5), peer.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], &big[..]);
}

#[tokio::test]
async fn a_phone_that_cannot_use_udp_relays_over_tcp() {
    let (r, _d, (_, user, pass)) = server().await;
    let (tcp_port, _) = r.turn_stream_ports();
    let port = tcp_port.unwrap();
    let s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (local, remote) = (s.local_addr().unwrap(), s.peer_addr().unwrap());
    let conn = Arc::new(StreamConn::new(s, local, remote));
    let client = Client::new(client_cfg(conn, port, &user, &pass))
        .await
        .unwrap();
    relay_round_trip(&client).await;
    client.close().await.unwrap();
    r.stop().await;
}

#[tokio::test]
async fn and_over_tls_with_a_certificate_it_trusts() {
    let (r, _d, (_, user, pass)) = server().await;
    let (_, tls_port) = r.turn_stream_ports();
    let port = tls_port.unwrap();
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (local, remote) = (tcp.local_addr().unwrap(), tcp.peer_addr().unwrap());
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(r.tls().unwrap().leaf_der()))
        .unwrap();
    let cc = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let tls = tokio_rustls::TlsConnector::from(Arc::new(cc))
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    let conn = Arc::new(StreamConn::new(tls, local, remote));
    let client = Client::new(client_cfg(conn, port, &user, &pass))
        .await
        .unwrap();
    relay_round_trip(&client).await;
    client.close().await.unwrap();
    r.stop().await;
}

#[tokio::test]
async fn a_bad_password_is_refused_over_tcp_too_and_garbage_closes_the_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (r, _d, (_, user, _)) = server().await;
    let port = r.turn_stream_ports().0.unwrap();
    let s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (local, remote) = (s.local_addr().unwrap(), s.peer_addr().unwrap());
    let client = Client::new(client_cfg(
        Arc::new(StreamConn::new(s, local, remote)),
        port,
        &user,
        "wrong",
    ))
    .await
    .unwrap();
    client.listen().await.unwrap();
    assert!(client.allocate().await.is_err());
    // not TURN at all: the server hangs up
    let mut g = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    g.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
    let mut b = [0u8; 8];
    let n = tokio::time::timeout(Duration::from_secs(3), g.read(&mut b))
        .await
        .unwrap()
        .unwrap_or(0);
    assert_eq!(n, 0, "the connection should be closed");
    r.stop().await;
}

#[tokio::test]
async fn the_addresses_given_to_phones_list_every_way_in() {
    let (r, _d, _) = server().await;
    let (tcp, tls) = r.turn_stream_ports();
    let mut s = TcpStream::connect(r.addr).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    s.write_all(b"GET /turn HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let urls: Vec<&str> = v["urls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u.as_str().unwrap())
        .collect();
    assert_eq!(urls.len(), 3, "{urls:?}");
    assert!(urls[0].starts_with("turn:127.0.0.1:") && !urls[0].contains("transport"));
    assert_eq!(
        urls[1],
        format!("turn:127.0.0.1:{}?transport=tcp", tcp.unwrap())
    );
    assert_eq!(
        urls[2],
        format!("turns:127.0.0.1:{}?transport=tcp", tls.unwrap())
    );
    r.stop().await;
}

#[tokio::test]
async fn two_phones_on_the_same_server_relay_to_each_other_over_any_mix_of_transports() {
    let (r, _d, (_, user, pass)) = server().await;
    let (udp_port, (tcp_port, _)) = (r.turn_port().unwrap(), r.turn_stream_ports());
    // phone A over UDP
    let a = Client::new(client_cfg(
        Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap()),
        udp_port,
        &user,
        &pass,
    ))
    .await
    .unwrap();
    a.listen().await.unwrap();
    let relay_a = a.allocate().await.unwrap();
    // phone B over TCP
    let tcp_port = tcp_port.unwrap();
    let s = TcpStream::connect(("127.0.0.1", tcp_port)).await.unwrap();
    let (local, remote) = (s.local_addr().unwrap(), s.peer_addr().unwrap());
    let b = Client::new(client_cfg(
        Arc::new(StreamConn::new(s, local, remote)),
        tcp_port,
        &user,
        &pass,
    ))
    .await
    .unwrap();
    b.listen().await.unwrap();
    let relay_b = b.allocate().await.unwrap();
    // each side opens a permission for the other's relayed address (what the apps do after exchanging candidates)
    relay_b
        .send_to(b"", relay_a.local_addr().unwrap())
        .await
        .unwrap();
    relay_a
        .send_to(b"hello B", relay_b.local_addr().unwrap())
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    let (n, from) = tokio::time::timeout(Duration::from_secs(5), relay_b.recv_from(&mut buf))
        .await
        .expect("B got nothing")
        .unwrap();
    assert_eq!(&buf[..n], b"hello B");
    assert_eq!(
        from,
        relay_a.local_addr().unwrap(),
        "it must appear to come from A's relayed address"
    );
    relay_b.send_to(b"hello A", from).await.unwrap();
    // (B's empty permission-opening packet was relayed to A too; skip it)
    let got = loop {
        let (n, _) = tokio::time::timeout(Duration::from_secs(5), relay_a.recv_from(&mut buf))
            .await
            .expect("A got nothing")
            .unwrap();
        if n > 0 {
            break buf[..n].to_vec();
        }
    };
    assert_eq!(got, b"hello A");
    r.stop().await;
}
