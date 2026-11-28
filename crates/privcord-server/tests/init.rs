//! `privcord-server init` writes a settings file the server accepts: private by default, open only on request.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use privcord_server::config::Config;
use std::process::Command;

fn init(args: &[&str]) -> Config {
    let dir = std::env::temp_dir().join(format!("pc-init-{}-{}", std::process::id(), args.len()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("s.toml");
    let _ = std::fs::remove_file(&file);
    let out = Command::new(env!("CARGO_BIN_EXE_privcord-server"))
        .arg("-c")
        .arg(&file)
        .arg("init")
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let c = Config::load(&file).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    c
}

#[test]
fn private_by_default_open_on_request() {
    let c = init(&[]);
    assert_eq!(c.server.access_tokens.len(), 1);
    assert_eq!(c.server.access_tokens[0].len(), 32);
    assert!(c.limits.disk_quota_mb > 0 && c.limits.monthly_traffic_gb > 0);
    assert!(c.problems().iter().all(|p| !p.contains("token")));
    let o = init(&["--open"]);
    assert!(o.server.access_tokens.is_empty());
}
