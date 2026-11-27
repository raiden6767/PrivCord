#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
//! Clients that try to hold a connection open: a head sent one piece at a time, a connection that says nothing, a body
//! that never finishes, and many connections from one address.

use privcord_server::{config::Config, start, Running};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::connect_async;

async fn server(tweak: impl FnOnce(&mut Config)) -> (Running, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Config::default();
    c.server.listen = "127.0.0.1:0".into();
    c.server.data_dir = dir.path().into();
    c.server.header_timeout_secs = 1;
    c.server.request_timeout_secs = 1;
    c.admin.enabled = true;
    c.admin.listen = "127.0.0.1:0".into();
    c.nostr.enabled = false;
    tweak(&mut c);
    (start(c).await.unwrap(), dir)
}

/// How long until the server closes the connection (or says something and closes), up to `max`.
async fn until_closed(s: &mut TcpStream, max: Duration) -> Option<(Duration, String)> {
    let start = Instant::now();
    let mut said = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let left = max.saturating_sub(start.elapsed());
        match tokio::time::timeout(left, s.read(&mut buf)).await {
            Err(_) => return None,
            Ok(Ok(0) | Err(_)) => {
                return Some((start.elapsed(), String::from_utf8_lossy(&said).into_owned()))
            }
            Ok(Ok(n)) => said.extend_from_slice(&buf[..n]),
        }
    }
}

#[tokio::test]
async fn a_head_that_never_finishes_is_cut_off() {
    let (r, _d) = server(|_| {}).await;
    let mut s = TcpStream::connect(r.addr).await.unwrap();
    // The start of a request, then nothing: no blank line ever ends the head.
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\n")
        .await
        .unwrap();
    let (took, _) = until_closed(&mut s, Duration::from_secs(6))
        .await
        .expect("the server held a half-sent request open");
    assert!(took < Duration::from_secs(4), "closed after {took:?}");
    r.stop().await;
}

#[tokio::test]
async fn a_connection_that_says_nothing_is_cut_off_and_a_real_request_still_works() {
    let (r, _d) = server(|_| {}).await;
    let mut silent = TcpStream::connect(r.addr).await.unwrap();
    until_closed(&mut silent, Duration::from_secs(6))
        .await
        .expect("a silent connection was held open");
    let mut ok = TcpStream::connect(r.addr).await.unwrap();
    ok.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let (_, said) = until_closed(&mut ok, Duration::from_secs(3)).await.unwrap();
    assert!(said.starts_with("HTTP/1.1 200"), "{said}");
    r.stop().await;
}

#[tokio::test]
async fn a_body_that_stalls_gets_a_408() {
    let (r, _d) = server(|_| {}).await;
    let mut s = TcpStream::connect(r.admin_addr().unwrap()).await.unwrap();
    s.write_all(
        b"POST /admin/api/setup HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"co",
    )
    .await
    .unwrap();
    let (took, said) = until_closed(&mut s, Duration::from_secs(6))
        .await
        .expect("a stalled body was waited for");
    assert!(said.starts_with("HTTP/1.1 408"), "{said}");
    assert!(took < Duration::from_secs(4), "{took:?}");
    r.stop().await;
}

#[tokio::test]
async fn one_address_cannot_hold_more_connections_than_allowed_and_websockets_are_not_counted() {
    let (r, _d) = server(|c| {
        c.server.max_http_connections_per_ip = 3;
        c.server.header_timeout_secs = 30; // long: the count, not the timeout, is what is tested
    })
    .await;
    let mut held = Vec::new();
    for _ in 0..3 {
        held.push(TcpStream::connect(r.addr).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    // The fourth is accepted by the system and dropped by the server at once.
    let mut fourth = TcpStream::connect(r.addr).await.unwrap();
    let (took, _) = until_closed(&mut fourth, Duration::from_secs(2))
        .await
        .expect("the fourth connection was kept");
    assert!(took < Duration::from_secs(1), "{took:?}");
    // A place frees up when one of the three goes.
    drop(held.pop());
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut again = TcpStream::connect(r.addr).await.unwrap();
    again
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let (_, said) = until_closed(&mut again, Duration::from_secs(3))
        .await
        .unwrap();
    assert!(said.starts_with("HTTP/1.1 200"), "{said}");
    drop(held);
    // WebSockets leave the count once upgraded: many of them from one address are the relay's business, not this limit.
    let mut sockets = Vec::new();
    for _ in 0..8 {
        sockets.push(connect_async(format!("ws://{}", r.addr)).await.unwrap().0);
    }
    assert_eq!(sockets.len(), 8);
    r.stop().await;
}
