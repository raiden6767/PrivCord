//! The web side's connection loop, in place of `axum::serve`, so that a slow or greedy client cannot hold what it likes:
//!
//! - a **header timeout**: the whole request head must arrive within `header_timeout_secs` (also while waiting for the next
//!   request on a kept-alive connection), so a client that sends one byte a minute is cut off (the "slow header" attack);
//! - a **request timeout**: a request not answered in `request_timeout_secs` gets a 408 (a body that trickles in, a stuck handler);
//! - a bound on the **head's size**, on the **connections** in the request/response phase in all, and on those from one address.
//!
//! A WebSocket leaves these counts once it has been upgraded (hyper hands the socket over): the relay's own per-address
//! and rate limits apply to it from there.

use axum::{extract::ConnectInfo, Router};
use hyper::server::conn::http1;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::config::ServerConfig;

#[derive(Debug, Clone, Copy)]
pub struct HttpLimits {
    pub header_timeout: Duration,
    pub request_timeout: Duration,
    pub max_connections: usize,
    pub max_per_ip: usize,
}

impl HttpLimits {
    pub fn from_config(c: &ServerConfig) -> Self {
        Self {
            header_timeout: Duration::from_secs(c.header_timeout_secs.max(1)),
            request_timeout: Duration::from_secs(c.request_timeout_secs.max(1)),
            max_connections: c.max_http_connections.max(1),
            // Behind a proxy every visitor has the proxy's address: a per-address count would limit everyone at once.
            max_per_ip: if c.trust_proxy {
                0
            } else {
                c.max_http_connections_per_ip
            },
        }
    }
}

/// Connections now open, per address group (a /64 for IPv6: one machine can use any address of its prefix).
type Counts = Arc<Mutex<HashMap<IpAddr, usize>>>;

fn group(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => {
            let o = v6.octets();
            let mut g = [0u8; 16];
            g[..8].copy_from_slice(&o[..8]);
            IpAddr::V6(Ipv6Addr::from(g))
        }
        v4 => v4,
    }
}

/// Holds one connection's place in the per-address count until it is dropped.
struct Slot {
    counts: Counts,
    ip: IpAddr,
}

impl Slot {
    fn take(counts: &Counts, ip: IpAddr, max: usize) -> Option<Self> {
        if max == 0 {
            return Some(Self {
                counts: counts.clone(),
                ip: group(ip),
            });
        }
        let ip = group(ip);
        let mut c = counts.lock().unwrap_or_else(PoisonError::into_inner);
        let n = c.entry(ip).or_insert(0);
        if *n >= max {
            return None;
        }
        *n += 1;
        Some(Self {
            counts: counts.clone(),
            ip,
        })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut c = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = c.get_mut(&self.ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                c.remove(&self.ip);
            }
        }
    }
}

/// Serves `router` on `listener` until `shutdown` completes, with `limits`.
pub async fn run<L, S>(mut listener: L, router: Router, limits: HttpLimits, shutdown: S)
where
    L: axum::serve::Listener<Addr = SocketAddr>,
    L::Io: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: Future<Output = ()>,
{
    let router = router.layer(axum::middleware::from_fn(
        move |req, next: axum::middleware::Next| async move {
            match tokio::time::timeout(limits.request_timeout, next.run(req)).await {
                Ok(response) => response,
                Err(_) => axum::response::IntoResponse::into_response(
                    axum::http::StatusCode::REQUEST_TIMEOUT,
                ),
            }
        },
    ));
    let counts: Counts = Arc::new(Mutex::new(HashMap::new()));
    let total = Arc::new(tokio::sync::Semaphore::new(limits.max_connections));
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut tasks = tokio::task::JoinSet::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            (io, peer) = listener.accept() => {
                // Over the limits: the connection is dropped before anything is read from it.
                let Ok(permit) = total.clone().try_acquire_owned() else { continue };
                let Some(slot) = Slot::take(&counts, peer.ip(), limits.max_per_ip) else { continue };
                let service = TowerToHyperService::new(
                    router.clone().layer(axum::Extension(ConnectInfo(peer))),
                );
                let mut stop = stop_rx.clone();
                tasks.spawn(async move {
                    let mut http = http1::Builder::new();
                    http.timer(TokioTimer::new())
                        .header_read_timeout(limits.header_timeout)
                        .max_buf_size(32 * 1024);
                    let conn = http.serve_connection(TokioIo::new(io), service).with_upgrades();
                    tokio::pin!(conn);
                    tokio::select! {
                        _ = conn.as_mut() => {}
                        () = async { let _ = stop.wait_for(|s| *s).await; } => {
                            conn.as_mut().graceful_shutdown();
                            let _ = tokio::time::timeout(Duration::from_secs(5), conn.as_mut()).await;
                        }
                    }
                    drop((permit, slot));
                });
                // Finished connections are collected as they go, so the set does not grow with every connection served.
                while tasks.try_join_next().is_some() {}
            }
        }
    }
    let _ = stop_tx.send(true);
    while tasks.join_next().await.is_some() {}
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn addresses_of_one_ipv6_prefix_share_a_count_and_ipv4_stays_itself() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(group(a), group(b));
        assert_ne!(group(a), group(c));
        let v4: IpAddr = "192.0.2.7".parse().unwrap();
        assert_eq!(group(v4), v4);
    }

    #[test]
    fn a_slot_is_given_back_when_the_connection_ends() {
        let counts: Counts = Arc::new(Mutex::new(HashMap::new()));
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        let first = Slot::take(&counts, ip, 2).unwrap();
        let _second = Slot::take(&counts, ip, 2).unwrap();
        assert!(Slot::take(&counts, ip, 2).is_none(), "the third is refused");
        drop(first);
        assert!(
            Slot::take(&counts, ip, 2).is_some(),
            "a place is free again"
        );
        assert!(Slot::take(&counts, ip, 0).is_some(), "0 means no limit");
    }
}
