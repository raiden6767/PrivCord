//! A relay (a mailbox for sealed messages) and an optional TURN server, in one program you run yourself.
//!
//! The server carries sealed messages and cannot read them. Whoever runs it is responsible for it.

pub mod config;
pub mod nat;
pub mod net;
pub mod nostr;
pub mod relay;
pub mod store;
pub mod turn_role;

use axum::serve::ListenerExt;
use config::Config;
use relay::Relay;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// A server that is running: where it listens, and how to stop it.
pub struct Running {
    pub addr: SocketAddr,
    stop: tokio::sync::watch::Sender<bool>,
    done: tokio::task::JoinHandle<()>,
    relay: Option<Arc<Relay>>,
    turn_port: Option<u16>,
}

impl Running {
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        let _ = self.done.await;
        if let Some(r) = &self.relay {
            r.compact(net::now_ms());
        }
    }

    /// The UDP port the TURN role listens on, if it is running.
    pub fn turn_port(&self) -> Option<u16> {
        self.turn_port
    }

    pub fn relay(&self) -> Option<&Arc<Relay>> {
        self.relay.as_ref()
    }
}

/// Starts what `cfg` asks for. The returned address is the one the web side listens on.
pub async fn start(mut cfg: Config) -> Result<Running, String> {
    if cfg.turn.enabled && matches!(cfg.turn.public_ip.trim(), "" | "auto") {
        cfg.turn.public_ip = nat::public_ip().await?.to_string();
    }
    let problems = cfg.problems();
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    let relay = if cfg.relay.enabled || cfg.nostr.enabled {
        let store = if cfg.relay.enabled && cfg.relay.store_and_forward {
            Some(store::Store::open(&cfg.server.data_dir)?)
        } else {
            None
        };
        let mut r = Relay::new(cfg.relay.clone(), store, net::now_ms());
        if cfg.nostr.enabled {
            r = r.with_nostr(Arc::new(nostr::Nostr::new(
                cfg.nostr.clone(),
                &cfg.server.data_dir,
            )?));
        }
        Some(Arc::new(r))
    } else {
        None
    };
    #[cfg(feature = "turn")]
    let turn = if cfg.turn.enabled {
        let secret = turn_role::load_or_make_secret(&cfg.server.data_dir)?;
        let ip: std::net::IpAddr = cfg
            .turn
            .public_ip
            .parse()
            .map_err(|_| "bad turn.public_ip".to_owned())?;
        let server = turn_role::server::start(&cfg.turn, secret.clone(), ip).await?;
        let url = format!("turn:{ip}:{}", server.addr.port());
        Some((server, secret, vec![url]))
    } else {
        None
    };
    #[cfg(not(feature = "turn"))]
    if cfg.turn.enabled {
        return Err("this build has no TURN support".into());
    }
    let listener = tokio::net::TcpListener::bind(&cfg.server.listen)
        .await
        .map_err(|e| format!("could not listen on {}: {e}", cfg.server.listen))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    #[cfg(feature = "turn")]
    let (turn_server, turn_info) = match turn {
        Some((s, secret, urls)) => (Some(s), Some((secret, urls))),
        None => (None, None),
    };
    #[cfg(feature = "turn")]
    let turn_port = turn_server.as_ref().map(|t| t.addr.port());
    #[cfg(not(feature = "turn"))]
    let turn_port = None;
    #[cfg(not(feature = "turn"))]
    let turn_info = None;
    let app = Arc::new(net::App::new(cfg, relay.clone(), turn_info));
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let router = net::router(app).into_make_service_with_connect_info::<SocketAddr>();
    let mut for_server = stopped.clone();
    let server = tokio::spawn(async move {
        // No Nagle: small replies must not wait for the other side's delayed ACK.
        let listener = listener.tap_io(|tcp| {
            let _ = tcp.set_nodelay(true);
        });
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = for_server.wait_for(|s| *s).await;
            })
            .await;
    });
    // With `durable`, events are confirmed after a batched disk sync; this task does the batching.
    let committer = relay
        .as_ref()
        .and_then(|r| r.nostr().cloned())
        .filter(|n| n.cfg.durable && n.cfg.persist)
        .map(|n| tokio::spawn(n.run_committer(stopped.clone())));
    let sweeper = relay.clone().map(|r| {
        tokio::spawn(async move {
            let mut sweep = tokio::time::interval(Duration::from_millis(500));
            let mut compact = tokio::time::interval(Duration::from_secs(600));
            compact.tick().await;
            loop {
                tokio::select! {
                    _ = sweep.tick() => r.sweep(net::now_ms()),
                    _ = compact.tick() => r.compact(net::now_ms()),
                    _ = stopped.wait_for(|s| *s) => break,
                }
            }
        })
    });
    let done = tokio::spawn(async move {
        let _ = server.await;
        #[cfg(feature = "turn")]
        if let Some(t) = turn_server {
            t.close().await;
        }
        if let Some(s) = sweeper {
            let _ = s.await;
        }
        if let Some(c) = committer {
            // Confirms what was still waiting for the disk.
            let _ = c.await;
        }
    });
    Ok(Running {
        addr,
        stop,
        done,
        relay,
        turn_port,
    })
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Running")
    }
}
