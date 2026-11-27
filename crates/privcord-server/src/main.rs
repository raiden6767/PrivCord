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
}

const TEMPLATE: &str = r#"# PrivCord server settings. Whoever runs this server is responsible for it and for what it is used for.
[server]
listen = "0.0.0.0:8787"        # the relay's web address (put a TLS proxy such as Caddy in front for wss://)
data_dir = "./data"
trust_proxy = false             # true only behind a proxy you control that sets X-Forwarded-For
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
# allowed_pubkeys = []          # hex keys; if any are listed, only they may publish
# blocked_pubkeys = []
# allowed_kinds = []
# min_pow = 0                   # NIP-13 difficulty (bits) required to publish
# auth_required = false         # NIP-42: clients must prove who they are first

[turn]
enabled = TURN_ENABLED
listen = "0.0.0.0:3478"         # UDP
public_ip = "auto"              # "auto" asks a public STUN server; or write the address
min_port = 49152                # open this UDP range in your firewall / forward it on your router
max_port = 49400
max_kbps = 0                    # per relayed connection, 0 = no limit
"#;

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return fail(&e.to_string()),
    };
    let result = match cli.cmd.unwrap_or(Cmd::Serve) {
        Cmd::Init { turn } => init(&cli.config, turn),
        Cmd::Serve => rt.block_on(serve(&cli.config)),
        Cmd::Doctor => rt.block_on(doctor(&cli.config)),
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

async fn serve(path: &std::path::Path) -> Result<(), String> {
    let cfg = load(path)?;
    let (relay, turn) = (cfg.relay.enabled, cfg.turn.enabled);
    let running = privcord_server::start(cfg).await?;
    println!(
        "privcord-server {} listening on {} (relay: {relay}, turn: {turn}). Ctrl-C to stop.",
        env!("CARGO_PKG_VERSION"),
        running.addr
    );
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
        println!("note: from inside your own network, test with a phone on mobile data, not Wi-Fi (many routers cannot loop back to their own public address).");
    }
    if bad == 0 {
        println!("all checks passed.");
        Ok(())
    } else {
        Err(format!("{bad} problem(s) found"))
    }
}
