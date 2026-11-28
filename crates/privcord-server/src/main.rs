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
    /// Make a storage key (32 random bytes as 64 hex characters) in a new file, for `storage.encrypt = true`. The key is not
    /// printed. Keep the file away from the data folder and back it up: without it, encrypted data cannot be read.
    StoreKey {
        /// The file to create (it must not exist).
        #[arg(long)]
        out: std::path::PathBuf,
    },
    /// Make a one-time setup code for another admin (works while the server runs). They enter it on the panel's login page
    /// and get their own token there; nothing long-lived is printed.
    /// Save everything in the data folder (messages waiting, Nostr events, admins, TURN secret) to one file. Stop the server
    /// first if it uses the disk store. To move to another server: back up, copy the file there, `restore`.
    Backup {
        /// The file to create (it must not exist).
        #[arg(long)]
        out: PathBuf,
        /// Also keep the settings file in the backup.
        #[arg(long)]
        with_config: bool,
        /// Encrypt the backup with the passphrase held in this environment variable.
        #[arg(long, value_name = "NAME")]
        passphrase_env: Option<String>,
    },
    /// Put a backup back into the data folder, after checking all of it. Run it before the server's first start there.
    Restore {
        /// The backup file.
        #[arg(long = "in")]
        input: PathBuf,
        /// The passphrase's environment variable, if the backup is encrypted.
        #[arg(long, value_name = "NAME")]
        passphrase_env: Option<String>,
        /// The data folder already holds data: move it aside (to `<folder>.before-restore`) and restore.
        #[arg(long)]
        replace: bool,
    },
    AdminInvite {
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
# port_mapping = true           # ask the home router (UPnP) to forward this server's ports, and remove them at stop
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
# stun_only = true              # only answer STUN: no relaying, no logins, no port range to open

# [limits]                      # all 0 = no limit
# monthly_traffic_gb = 500
# connection_kbps = 2000
# disk_quota_mb = 20000
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
        Cmd::StoreKey { out } => store_key(&out),
        Cmd::Backup {
            out,
            with_config,
            passphrase_env,
        } => backup(&cli.config, &out, with_config, passphrase_env.as_deref()),
        Cmd::Restore {
            input,
            passphrase_env,
            replace,
        } => restore(&cli.config, &input, passphrase_env.as_deref(), replace),
        Cmd::AdminInvite { name, role } => admin_invite(&cli.config, &name, &role),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

fn passphrase(var: Option<&str>) -> Result<Option<String>, String> {
    var.map(|v| {
        std::env::var(v)
            .ok()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| format!("the environment variable {v} is empty or not set"))
    })
    .transpose()
}

fn backup(
    cfg: &std::path::Path,
    out: &std::path::Path,
    with_config: bool,
    pass: Option<&str>,
) -> Result<(), String> {
    let c = load(cfg)?;
    let pass = passphrase(pass)?;
    let s = privcord_server::backup::create(
        &c.server.data_dir,
        with_config.then_some(cfg),
        out,
        pass.as_deref(),
    )?;
    println!(
        "saved {} files ({} bytes) to {}{}",
        s.files,
        s.bytes,
        out.display(),
        if pass.is_some() { ", encrypted" } else { "" }
    );
    if pass.is_none() {
        println!("this file is not encrypted: it holds your admins' access and the TURN secret. Keep it private, or add --passphrase-env.");
    }
    if c.storage.encrypt {
        println!("storage.encrypt is on: the storage key is NOT in this file. Copy it separately, or the data cannot be read.");
    }
    Ok(())
}

fn restore(
    cfg: &std::path::Path,
    input: &std::path::Path,
    pass: Option<&str>,
    replace: bool,
) -> Result<(), String> {
    let c = load(cfg)?;
    let pass = passphrase(pass)?;
    let (s, conf) =
        privcord_server::backup::restore(input, &c.server.data_dir, pass.as_deref(), replace)?;
    println!(
        "restored {} files ({} bytes) into {}",
        s.files,
        s.bytes,
        c.server.data_dir.display()
    );
    if let Some(p) = conf {
        println!(
            "the old settings are in {} for you to look at; they were not applied.",
            p.display()
        );
    }
    println!("start the server as usual. If its address changed, tell your people the new one.");
    Ok(())
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

fn store_key(out: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| format!("no randomness: {e}"))?;
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o400);
    }
    let mut f = o.open(out).map_err(|e| {
        format!(
            "could not create {} (it must not exist yet): {e}",
            out.display()
        )
    })?;
    f.write_all(format!("{hex}\n").as_bytes())
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("could not write {}: {e}", out.display()))?;
    println!(
        "storage key written to {} (read-only for this user). Set storage.encrypt = true and storage.key_file = \"{}\", \
         and keep a copy somewhere safe: without it, the encrypted data cannot be read.",
        out.display(),
        out.display()
    );
    Ok(())
}

fn admin_invite(path: &std::path::Path, name: &str, role: &str) -> Result<(), String> {
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
    let code = privcord_server::admin::AdminState::add_admin_invite_offline(
        &cfg.server.data_dir,
        name,
        role,
    )?;
    println!(
        "setup code for {name} ({role:?}), works once, for 15 minutes: {code}\n\
         They open the admin panel, enter it under \"Have a setup code?\", and get their own token there."
    );
    Ok(())
}

async fn serve(path: &std::path::Path) -> Result<(), String> {
    let cfg = load(path)?;
    privcord_server::supervise(
        cfg,
        async {
            let _ = tokio::signal::ctrl_c().await;
        },
        |running| {
            // What is on may change while the server runs (the panel can restart it with other choices).
            let c = running.config();
            println!(
                "privcord-server {} listening on {} (relay: {}, nostr: {}, turn: {}). Ctrl-C to stop.",
                env!("CARGO_PKG_VERSION"),
                running.addr,
                c.relay.enabled,
                c.nostr.enabled,
                if !c.turn.enabled { "off" } else if c.turn.stun_only { "stun only" } else { "on" },
            );
            if let Some(a) = running.admin_addr() {
                println!(
                    "admin panel: {}://{a}/admin",
                    if c.admin.tls { "https" } else { "http" }
                );
                if let Some(code) = running.admin_setup_code() {
                    // Not a token: it works once and stops after 15 minutes, so a terminal log that keeps it is worth nothing later.
                    println!("first time here? open the page and enter this setup code (works once, for 15 minutes): {code}");
                }
            }
        },
    )
    .await
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
    if matches!(cfg.turn.public_ip.trim(), "" | "auto") && cfg.turn.enabled && !cfg.turn.stun_only {
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
        if cfg.turn.stun_only {
            println!(
                "note: STUN only: forward just UDP {} to this machine (no relay ports are used).",
                cfg.turn.listen
            );
        } else {
            let n = u32::from(cfg.turn.max_port.saturating_sub(cfg.turn.min_port)) + 1;
            println!("note: forward UDP {} and UDP {}-{} to this machine ({n} simultaneous relayed connections).", cfg.turn.listen, cfg.turn.min_port, cfg.turn.max_port);
        }
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
