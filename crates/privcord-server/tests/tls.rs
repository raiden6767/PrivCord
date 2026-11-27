//! The relay over TLS (https and wss), with a self-signed certificate the test client trusts.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use futures_util::{SinkExt, StreamExt};
use privcord_server::{config::Config, start, Running};
use rustls::pki_types::{CertificateDer, ServerName};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

async fn server() -> (Running, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Config::default();
    c.server.listen = "127.0.0.1:0".into();
    c.server.data_dir = dir.path().into();
    c.tls.self_signed = true;
    c.tls.https_listen = "127.0.0.1:0".into();
    c.relay.store_and_forward = false;
    (start(c).await.unwrap(), dir)
}

fn connector(trust: Option<Vec<u8>>) -> tokio_rustls::TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    if let Some(der) = trust {
        roots.add(CertificateDer::from(der)).unwrap();
    }
    let c = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_rustls::TlsConnector::from(Arc::new(c))
}

async fn tls_connect(r: &Running, trust: bool) -> std::io::Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
    let tcp = tokio::net::TcpStream::connect(r.tls_addr().unwrap()).await?;
    let der = trust.then(|| r.tls().unwrap().leaf_der());
    connector(der).connect(ServerName::try_from("localhost").unwrap(), tcp).await
}

#[tokio::test]
async fn https_answers_and_the_plain_port_still_works() {
    let (r, _d) = server().await;
    let mut s = tls_connect(&r, true).await.unwrap();
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    assert!(out.contains("\"ok\":true"));
    // plain still there
    let mut p = tokio::net::TcpStream::connect(r.addr).await.unwrap();
    p.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut out = String::new();
    p.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 200"));
    r.stop().await;
}

#[tokio::test]
async fn the_relay_protocol_works_over_wss() {
    let (r, _d) = server().await;
    let a = tls_connect(&r, true).await.unwrap();
    let (mut a, _) = tokio_tungstenite::client_async("wss://localhost/", a).await.unwrap();
    let b = tls_connect(&r, true).await.unwrap();
    let (mut b, _) = tokio_tungstenite::client_async("wss://localhost/", b).await.unwrap();
    let send = |v: Value| Message::Text(v.to_string().into());
    b.send(send(json!({ "op": "sub", "mailbox": "mb" }))).await.unwrap();
    b.next().await.unwrap().unwrap();
    a.send(send(json!({ "op": "put", "mailbox": "mb", "id": "m1", "blob": "SEALED", "ttl": 60, "kind": "sig" }))).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(5), b.next()).await.unwrap().unwrap().unwrap();
    let v: Value = serde_json::from_str(got.to_text().unwrap()).unwrap();
    assert_eq!((v["op"].as_str(), v["blob"].as_str()), (Some("event"), Some("SEALED")));
    r.stop().await;
}

#[tokio::test]
async fn a_client_that_does_not_trust_the_certificate_is_refused() {
    let (r, _d) = server().await;
    assert!(tls_connect(&r, false).await.is_err());
    r.stop().await;
}

#[tokio::test]
async fn a_connection_that_never_finishes_its_handshake_does_not_hold_up_others() {
    let (r, _d) = server().await;
    // Some stalled connections: connected, silent.
    let _stalled: Vec<_> = futures_util::future::join_all((0..20).map(|_| tokio::net::TcpStream::connect(r.tls_addr().unwrap()))).await;
    let started = std::time::Instant::now();
    let mut s = tls_connect(&r, true).await.unwrap();
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 200"));
    assert!(started.elapsed() < Duration::from_secs(3), "was held up by the silent connections");
    r.stop().await;
}

#[tokio::test]
async fn asking_for_tls_without_a_certificate_is_an_error_in_words() {
    let mut c = Config::default();
    c.server.listen = "127.0.0.1:0".into();
    c.tls.https_listen = "127.0.0.1:0".into();
    let e = start(c).await.unwrap_err();
    assert!(e.contains("no certificate"), "{e}");
}
