//! A relay (a mailbox for sealed messages) and an optional TURN server, in one program you run yourself.
//!
//! The server carries sealed messages and cannot read them. Whoever runs it is responsible for it.

pub mod config;
pub mod nat;
pub mod net;
pub mod nostr;
pub mod relay;
pub mod store;
pub mod tls;
pub mod turn_role;
#[cfg(feature = "turn")]
pub mod turn_stream;

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
    turn_ports: Option<(Option<u16>, Option<u16>)>,
    tls_addr: Option<SocketAddr>,
    tls: Option<Arc<tls::Tls>>,
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
    /// Where the HTTPS side listens, if it does.
    pub fn tls_addr(&self) -> Option<SocketAddr> {
        self.tls_addr
    }

    /// The certificate in use (tests use it to trust a self-signed one).
    pub fn tls(&self) -> Option<&Arc<tls::Tls>> {
        self.tls.as_ref()
    }

    /// The TCP and TLS ports of the TURN role, when they are on.
    pub fn turn_stream_ports(&self) -> (Option<u16>, Option<u16>) {
        self.turn_ports.unwrap_or((None, None))
    }

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
    let tls = if cfg.wants_tls() {
        Some(tls::Tls::load(
            &cfg.tls,
            &cfg.server.data_dir,
            &cfg.turn.public_ip,
        )?)
    } else {
        None
    };
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
        let server =
            turn_role::server::start(&cfg.turn, secret.clone(), ip, tls.as_deref()).await?;
        // What phones are told: UDP first, then the ways through a network that blocks it.
        let mut urls = vec![format!("turn:{ip}:{}", server.addr.port())];
        if let Some(a) = server.tcp_addr {
            urls.push(format!("turn:{ip}:{}?transport=tcp", a.port()));
        }
        if let Some(a) = server.tls_addr {
            let host = if cfg.tls.domain.is_empty() {
                ip.to_string()
            } else {
                cfg.tls.domain.clone()
            };
            urls.push(format!("turns:{host}:{}?transport=tcp", a.port()));
        }
        Some((server, secret, urls))
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
    #[cfg(feature = "turn")]
    let turn_ports = turn_server
        .as_ref()
        .map(|t| (t.tcp_addr.map(|a| a.port()), t.tls_addr.map(|a| a.port())));
    #[cfg(not(feature = "turn"))]
    let turn_ports: Option<(Option<u16>, Option<u16>)> = None;
    #[cfg(not(feature = "turn"))]
    let turn_port = None;
    #[cfg(not(feature = "turn"))]
    let turn_info = None;
    let https_listen = cfg.tls.https_listen.clone();
    let app = Arc::new(net::App::new(cfg, relay.clone(), turn_info));
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let router = net::router(app.clone()).into_make_service_with_connect_info::<SocketAddr>();
    let mut for_server = stopped.clone();
    // HTTPS: the same web side, over TLS, on its own port.
    let (tls_addr, tls_server) = match (&tls, https_listen.is_empty()) {
        (Some(t), false) => {
            let tcp = tokio::net::TcpListener::bind(&https_listen)
                .await
                .map_err(|e| format!("could not listen for HTTPS on {https_listen}: {e}"))?;
            let listener =
                tls::TlsListener::new(tcp, t.https.clone()).map_err(|e| e.to_string())?;
            let addr = axum::serve::Listener::local_addr(&listener).map_err(|e| e.to_string())?;
            // (The no-op tap is what lets axum read the peer address from this listener.)
            let listener = listener.tap_io(|_| {});
            let routes = net::router(app).into_make_service_with_connect_info::<SocketAddr>();
            let mut stop_tls = stopped.clone();
            let task = tokio::spawn(async move {
                let _ = axum::serve(listener, routes)
                    .with_graceful_shutdown(async move {
                        let _ = stop_tls.wait_for(|s| *s).await;
                    })
                    .await;
            });
            (Some(addr), Some(task))
        }
        _ => (None, None),
    };
    // A renewed certificate file is picked up without a restart.
    let cert_watch = tls.clone().map(|t| {
        let mut stop_watch = stopped.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        match t.reload_if_changed() {
                            Ok(true) => eprintln!("TLS certificate reloaded"),
                            Ok(false) => {}
                            Err(e) => eprintln!("TLS certificate not reloaded (the old one stays): {e}"),
                        }
                    }
                    _ = stop_watch.wait_for(|s| *s) => break,
                }
            }
        })
    });
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
        if let Some(t) = tls_server {
            let _ = t.await;
        }
        if let Some(w) = cert_watch {
            let _ = w.await;
        }
        #[cfg(feature = "turn")]
        if let Some(t) = turn_server {
            t.close().await;
            t.finish().await;
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
        turn_ports,
        tls_addr,
        tls,
    })
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Running")
    }
}
