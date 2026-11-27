//! A phone, for the NAT lab (`scripts/nat-lab.sh`): connects to a TURN server over UDP, TCP or TLS, gets a relayed address,
//! and exchanges a packet with the other phone's relayed address.
//!
//!   turn_probe --transport udp|tcp|tls --server IP:PORT --user U --pass P --peer-ip SERVER_IP \
//!              [--ca cert.pem --sni NAME] (listen | send --to RELAYED_ADDR)
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

#[path = "../tests/common/stream_conn.rs"]
mod stream_conn;

use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use turn::client::{Client, ClientConfig};
use webrtc_util::Conn;

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter()
        .position(|x| x == name)
        .and_then(|i| a.get(i + 1).cloned())
}

#[tokio::main]
async fn main() {
    let transport = arg("--transport").unwrap_or_else(|| "udp".into());
    let server = arg("--server").expect("--server IP:PORT");
    let (user, pass) = (
        arg("--user").expect("--user"),
        arg("--pass").expect("--pass"),
    );
    let peer_ip: std::net::IpAddr = arg("--peer-ip").expect("--peer-ip").parse().unwrap();
    let server_addr: SocketAddr = server.parse().unwrap();

    let conn: Arc<dyn Conn + Send + Sync> = match transport.as_str() {
        "udp" => Arc::new(tokio::net::UdpSocket::bind("0.0.0.0:0").await.unwrap()),
        "tcp" | "tls" => {
            let tcp = match tokio::time::timeout(
                Duration::from_secs(6),
                tokio::net::TcpStream::connect(server_addr),
            )
            .await
            {
                Ok(Ok(s)) => s,
                other => {
                    println!("FAIL could not connect over TCP: {other:?}");
                    std::process::exit(2);
                }
            };
            let (local, remote) = (tcp.local_addr().unwrap(), tcp.peer_addr().unwrap());
            if transport == "tcp" {
                Arc::new(stream_conn::StreamConn::new(tcp, local, remote))
            } else {
                let mut roots = rustls::RootCertStore::empty();
                for c in CertificateDer::pem_file_iter(arg("--ca").expect("--ca for tls")).unwrap()
                {
                    roots.add(c.unwrap()).unwrap();
                }
                let cc = rustls::ClientConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth();
                let name = ServerName::try_from(arg("--sni").unwrap_or_else(|| "localhost".into()))
                    .unwrap();
                let tls = tokio_rustls::TlsConnector::from(Arc::new(cc))
                    .connect(name, tcp)
                    .await
                    .unwrap();
                Arc::new(stream_conn::StreamConn::new(tls, local, remote))
            }
        }
        other => panic!("unknown transport {other}"),
    };
    let client = Client::new(ClientConfig {
        stun_serv_addr: String::new(),
        turn_serv_addr: server,
        username: user,
        password: pass,
        realm: "privcord".into(),
        software: String::new(),
        rto_in_ms: 300,
        conn,
        vnet: None,
    })
    .await
    .unwrap();
    client.listen().await.unwrap();
    let started = Instant::now();
    let relay = match tokio::time::timeout(Duration::from_secs(8), client.allocate()).await {
        Ok(Ok(r)) => r,
        other => {
            println!("FAIL no relay: {:?}", other.map(|r| r.map(|_| ())));
            std::process::exit(2);
        }
    };
    println!(
        "RELAY {} (allocated in {:?}, over {transport})",
        relay.local_addr().unwrap(),
        started.elapsed()
    );
    // Permission for the other phone's relayed address, which lives on this same server: one for its IP is enough.
    let perm = relay.send_to(b"", SocketAddr::new(peer_ip, 9)).await;
    println!("PERMISSION for {peer_ip}: {:?}", perm.map(|_| ()));

    let mut buf = [0u8; 1500];
    if std::env::args().any(|a| a == "listen") {
        match tokio::time::timeout(Duration::from_secs(30), relay.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) => {
                println!("RECV {:?} from {from}", String::from_utf8_lossy(&buf[..n]));
                let _ = relay.send_to(b"pong", from).await;
                tokio::time::sleep(Duration::from_millis(500)).await;
                std::process::exit(0);
            }
            other => {
                println!("FAIL nothing arrived: {:?}", other.map(|r| r.map(|_| ())));
                std::process::exit(1);
            }
        }
    }
    let to: SocketAddr = arg("--to").expect("--to RELAYED_ADDR").parse().unwrap();
    for _ in 0..16 {
        let t = Instant::now();
        let _ = relay.send_to(b"ping", to).await;
        if let Ok(Ok((n, from))) =
            tokio::time::timeout(Duration::from_millis(1200), relay.recv_from(&mut buf)).await
        {
            if &buf[..n] == b"pong" {
                println!("OK pong from {from} in {:?}", t.elapsed());
                std::process::exit(0);
            }
        }
    }
    println!("FAIL no answer from {to}");
    std::process::exit(1);
}
