//! `privcord-server`: run your own relay and/or TURN server for PrivCord.

use clap::{Parser, Subcommand};
use privcord_server::{config::Config, nat};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "privcord-server",
    version,
    about = "A relay and an optional TURN server for PrivCord. You run it, you are responsible for it."
)]
struct Cli {
    /// The settings file.
    #[arg(
        short,
        long,
        env = "PRIVCORD_CONFIG",
        default_value = "privcord-server.toml",
        global = true
    )]
    config: PathBuf,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the server (the default).
    Serve,
    /// Write a starting settings file.
    Init {
        /// Also run the TURN role.
        #[arg(long)]
        turn: bool,
    },
    /// Check this machine and network for what would stop the server working.
    Doctor,
    /// Make an admin token for the admin panel (works while the server runs).
    AdminToken {
        /// A name for this admin.
        #[arg(long, default_value = "admin")]
        name: String,
        /// owner (everything), moderator (rules, tokens, remove events) or viewer (look only).
        #[arg(long, default_value = "owner")]
        role: String,
    },
}

const TEMPLATE: &str = r#"# PrivCord server settings. Whoever runs this server is responsible for it and for what it is used for.
[server]
listen = "0.0.0.0:8787"        # the relay's web address (put a TLS proxy such as Caddy in front for wss://)
data_dir = "./data"
trust_proxy = false             # true only behind a proxy you control that sets the header below
# real_ip_header = "x-forwarded-for"   # e.g. "cf-connecting-ip" behind Cloudflare
# metrics = false               # Prometheus counters at /metrics (needs a token if tokens are set)
access_tokens = []              # empty: anyone can use it; otherwise clients must send one of these
admin_contact = ""              # shown on the discovery page

[relay]
enabled = true
store_and_forward = true        # keep sealed messages for people who are offline (7 days at most)

[nostr]
enabled = false                 # also speak the Nostr relay protocol on the same port (NIP-1, 9, 11, 13, 40, 42, 45)
store_events = true
name = "My PrivCord server"
description = "A relay run by its administrator."
retention_days = 30
max_events = 1000000
# durable = false               # confirm an event only after it is safely on disk (batched, so still fast)
# persist = true                # false: keep events in memory only
# auth_dms = false              # NIP-42: direct messages only to their sender and recipient
# events_per_second_global = 0  # cap on events per second for the whole relay (0: none)
# reqs_per_minute_per_conn = 0  # cap on subscriptions one connection may open per minute (0: none)
# admin_pubkey = ""             # your Nostr key (hex), shown in the information document
# allowed_pubkeys = []          # hex keys; if any are listed, only they may publish
# blocked_pubkeys = []
# allowed_kinds = []
# min_pow = 0                   # NIP-13 difficulty (bits) required to publish
# auth_required = false         # NIP-42: clients must prove who they are first

[admin]                         # a web page and API to manage the server
enabled = false
listen = "127.0.0.1:8788"       # keep this on localhost or a private network (a VPN such as Tailscale), not the open internet
# tls = false                   # serve it over TLS (needs [tls]); otherwise tokens travel unencrypted

[tls]                           # needed for wss://, the admin panel over https, and TURN over TLS
# cert_file = "/etc/letsencrypt/live/example.org/fullchain.pem"   # read again when it changes (renewals need no restart)
# key_file = "/etc/letsencrypt/live/example.org/privkey.pem"
# self_signed = false           # true: make a certificate for trying things out (apps will not trust it)
# domain = "turn.example.org"   # the name people use; shown in turns: addresses
# https_listen = "0.0.0.0:8443" # serve the relay (and admin) over TLS here, next to the plain listener

[turn]
enabled = TURN_ENABLED
listen = "0.0.0.0:3478"         # UDP
# tcp_listen = "0.0.0.0:3478"   # also over TCP, for networks that block UDP
# tls_listen = "0.0.0.0:443"    # also over TLS, for networks that only let TLS through (needs [tls])
public_ip = "auto"              # "auto" asks a public STUN server; or write the address
min_port = 49152                # open this UDP range in your firewall / forward it on your router
max_port = 49400
max_kbps = 0                    # per relayed connection, 0 = no limit
"#;

/// A minimal logger to stderr. Quiet by default (warnings and errors only); `PRIVCORD_LOG=info|debug|trace` turns it up. At
/// debug level the TURN library logs client addresses, so leave it off in normal use.
struct StderrLog(log::LevelFilter);

impl log::Log for StderrLog {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= self.0
    }
    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) {
            eprintln!(
                "{} {:5} {}: {}",
                privcord_server::net::now_ms() / 1000,
                r.level(),
                r.target(),
                r.args()
            );
        }
    }
    fn flush(&self) {}
}

fn init_logging() {
    let level = std::env::var("PRIVCORD_LOG")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(log::LevelFilter::Warn);
    if log::set_logger(Box::leak(Box::new(StderrLog(level)))).is_ok() {
        log::set_max_level(level);
    }
}

fn main() -> std::process::ExitCode {
    init_logging();
    let cli = Cli::parse();
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return fail(&e.to_string()),
    };
    let result = match cli.cmd.unwrap_or(Cmd::Serve) {
        Cmd::Init { turn } => init(&cli.config, turn),
        Cmd::Serve => rt.block_on(serve(&cli.config)),
        Cmd::Doctor => rt.block_on(doctor(&cli.config)),
        Cmd::AdminToken { name, role } => admin_token(&cli.config, &name, &role),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

fn fail(msg: &str) -> std::process::ExitCode {
    eprintln!("error: {msg}");
    std::process::ExitCode::FAILURE
}

fn init(path: &std::path::Path, turn: bool) -> Result<(), String> {
    if path.exists() {
        return Err(format!(
            "{} already exists; not overwriting it",
            path.display()
        ));
    }
    std::fs::write(
        path,
        TEMPLATE.replace("TURN_ENABLED", if turn { "true" } else { "false" }),
    )
    .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    println!(
        "wrote {}. Edit it, then run: privcord-server doctor",
        path.display()
    );
    Ok(())
}

fn load(path: &std::path::Path) -> Result<Config, String> {
    if path.exists() {
        Config::load(path)
    } else {
        Ok(Config::default())
    }
}

fn admin_token(path: &std::path::Path, name: &str, role: &str) -> Result<(), String> {
    let cfg = load(path)?;
    let role = match role {
        "owner" => privcord_server::admin::Role::Owner,
        "moderator" => privcord_server::admin::Role::Moderator,
        "viewer" => privcord_server::admin::Role::Viewer,
        other => {
            return Err(format!(
                "role must be owner, moderator or viewer, not \"{other}\""
            ))
        }
    };
    let token =
        privcord_server::admin::AdminState::add_admin_offline(&cfg.server.data_dir, name, role)?;
    println!("admin token for {name} ({role:?}), shown once, keep it safe:\n{token}");
    Ok(())
}

async fn serve(path: &std::path::Path) -> Result<(), String> {
    let cfg = load(path)?;
    let (relay, turn) = (cfg.relay.enabled, cfg.turn.enabled);
    let running = privcord_server::start(cfg).await?;
    println!(
        "privcord-server {} listening on {} (relay: {relay}, turn: {turn}). Ctrl-C to stop.",
        env!("CARGO_PKG_VERSION"),
        running.addr
    );
    if let Some(a) = running.admin_addr() {
        println!("admin panel: http://{a}/admin");
        if let Some(t) = running.admin_first_token() {
            println!("admin token (shown only now, save it): {t}");
        }
    }
    tokio::signal::ctrl_c().await.map_err(|e| e.to_string())?;
    running.stop().await;
    Ok(())
}

async fn doctor(path: &std::path::Path) -> Result<(), String> {
    let mut cfg = load(path)?;
    let mut bad = 0;
    let mut say = |ok: bool, text: String| {
        println!("{} {text}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            bad += 1;
        }
    };
    if matches!(cfg.turn.public_ip.trim(), "" | "auto") && cfg.turn.enabled {
        match nat::public_ip().await {
            Ok(ip) => {
                say(true, format!("public address seen from the internet: {ip}"));
                if !nat::is_public(ip) {
                    say(false, format!("{ip} is not a public address: your provider shares one address between customers (carrier-grade NAT). Phones cannot reach this machine; use a VPS, or a tunnel to one."));
                }
                cfg.turn.public_ip = ip.to_string();
            }
            Err(e) => say(false, e),
        }
    }
    for p in cfg.problems() {
        say(false, p);
    }
    say(
        tokio::net::TcpListener::bind(&cfg.server.listen)
            .await
            .is_ok(),
        format!("relay can listen on {} (TCP)", cfg.server.listen),
    );
    if cfg.turn.enabled {
        say(
            tokio::net::UdpSocket::bind(&cfg.turn.listen).await.is_ok(),
            format!("TURN can listen on {} (UDP)", cfg.turn.listen),
        );
        let n = u32::from(cfg.turn.max_port.saturating_sub(cfg.turn.min_port)) + 1;
        println!("note: forward UDP {} and UDP {}-{} to this machine ({n} simultaneous relayed connections).", cfg.turn.listen, cfg.turn.min_port, cfg.turn.max_port);
        if !cfg.turn.tcp_listen.is_empty() || !cfg.turn.tls_listen.is_empty() {
            println!("note: also forward the TCP port(s) you set for TURN over TCP / TLS; relayed traffic still leaves over UDP.");
        }
        println!("note: from inside your own network, test with a phone on mobile data, not Wi-Fi (many routers cannot loop back to their own public address).");
    }
    if bad == 0 {
        println!("all checks passed.");
        Ok(())
    } else {
        Err(format!("{bad} problem(s) found"))
    }
}
