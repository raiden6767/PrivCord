//! What this server runs, as the operator sees it: a list of protocols, each on or off, with the ports it uses. The choices
//! made in the admin panel are kept in `data_dir/panel.json` and laid over the settings file each time the server starts; the
//! panel shows which values come from where, so a line edited in the file is never silently ignored.
//!
//! A protocol here is a description, not a plug-in: the wiring is in `start()`. The next protocols added will show what the
//! description needs to say; until then it holds only what the panel and the health report already use.

use crate::config::Config;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

const FILE: &str = "panel.json";
const ERROR_FILE: &str = "apply-error.txt";

/// The choices made in the panel. `None`: not chosen there, the settings file decides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Panel {
    pub relay: Option<bool>,
    pub nostr: Option<bool>,
    /// "off", "stun" (answer address lookups only) or "turn" (calls and files: relays traffic, includes STUN).
    pub turn: Option<String>,
    pub monthly_traffic_gb: Option<u64>,
    pub connection_kbps: Option<u64>,
    pub disk_quota_mb: Option<u64>,
}

impl Panel {
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::fsutil::write_private_atomic(&dir.join(FILE), text.as_bytes())
    }

    /// The panel's choices, read from what the browser sent (unknown names and wrong types are refused).
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let p: Self =
            serde_json::from_value(v.clone()).map_err(|e| format!("not understood: {e}"))?;
        if let Some(t) = &p.turn {
            if !matches!(t.as_str(), "off" | "stun" | "turn") {
                return Err("turn must be \"off\", \"stun\" or \"turn\"".into());
            }
        }
        Ok(p)
    }

    /// Lays the choices over `cfg`.
    pub fn apply(&self, cfg: &mut Config) {
        if let Some(v) = self.relay {
            cfg.relay.enabled = v;
        }
        if let Some(v) = self.nostr {
            cfg.nostr.enabled = v;
        }
        match self.turn.as_deref() {
            Some("off") => cfg.turn.enabled = false,
            Some(m @ ("stun" | "turn")) => {
                cfg.turn.enabled = true;
                cfg.turn.stun_only = m == "stun";
                if m == "turn" && cfg.turn.public_ip.trim().is_empty() {
                    cfg.turn.public_ip = "auto".into();
                }
            }
            _ => {}
        }
        if let Some(v) = self.monthly_traffic_gb {
            cfg.limits.monthly_traffic_gb = v;
        }
        if let Some(v) = self.connection_kbps {
            cfg.limits.connection_kbps = v;
        }
        if let Some(v) = self.disk_quota_mb {
            cfg.limits.disk_quota_mb = v;
        }
    }
}

/// Why the last change from the panel was not applied (the server went back to what worked), if that happened.
pub fn last_error(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(ERROR_FILE))
        .ok()
        .filter(|t| !t.trim().is_empty())
}

pub fn set_error(dir: &Path, text: &str) {
    let _ = crate::fsutil::write_private_atomic(&dir.join(ERROR_FILE), text.as_bytes());
}

pub fn clear_error(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(ERROR_FILE));
}

/// One line per protocol for the panel: whether it is on, the ports it needs, and where that setting comes from.
pub fn describe(cfg: &Config, panel: &Panel) -> Vec<Value> {
    let from = |set: bool| if set { "panel" } else { "settings file" };
    let web = format!("TCP {}", cfg.server.listen);
    let turn_mode = if !cfg.turn.enabled {
        "off"
    } else if cfg.turn.stun_only {
        "stun"
    } else {
        "turn"
    };
    let mut turn_ports = vec![format!("UDP {}", cfg.turn.listen)];
    if !cfg.turn.tcp_listen.is_empty() {
        turn_ports.push(format!("TCP {}", cfg.turn.tcp_listen));
    }
    if !cfg.turn.tls_listen.is_empty() {
        turn_ports.push(format!("TLS {}", cfg.turn.tls_listen));
    }
    if turn_mode == "turn" {
        turn_ports.push(format!(
            "UDP {}-{} (relayed traffic)",
            cfg.turn.min_port, cfg.turn.max_port
        ));
    }
    vec![
        json!({
            "id": "relay", "name": "PrivCord relay", "on": cfg.relay.enabled, "ports": [web.clone()],
            "about": "Passes the first messages between two phones that are setting up a connection, and holds them for a phone that is offline.",
            "source": from(panel.relay.is_some()),
        }),
        json!({
            "id": "nostr", "name": "Nostr relay", "on": cfg.nostr.enabled, "ports": [web],
            "about": "A Nostr relay for any Nostr app. Shares the PrivCord relay's port.",
            "source": from(panel.nostr.is_some()),
        }),
        json!({
            "id": "turn", "name": "Calls, files and address lookup (TURN and STUN)", "mode": turn_mode,
            "on": cfg.turn.enabled, "ports": turn_ports,
            "about": "STUN tells a phone which address it is seen at and uses almost no bandwidth. TURN also relays calls and files for phones that cannot connect directly, and uses bandwidth.",
            "source": from(panel.turn.is_some()),
        }),
    ]
}

/// The limits in force, and where each comes from.
pub fn limits(cfg: &Config, panel: &Panel) -> Value {
    let from = |set: bool| if set { "panel" } else { "settings file" };
    json!({
        "monthly_traffic_gb": { "value": cfg.limits.monthly_traffic_gb, "source": from(panel.monthly_traffic_gb.is_some()) },
        "connection_kbps": { "value": cfg.limits.connection_kbps, "source": from(panel.connection_kbps.is_some()) },
        "disk_quota_mb": { "value": cfg.limits.disk_quota_mb, "source": from(panel.disk_quota_mb.is_some()) },
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_panels_choices_lay_over_the_file_and_nothing_else_changes() {
        let mut c = Config::default();
        c.relay.enabled = true;
        let p = Panel::from_json(&json!({"nostr": true, "turn": "stun", "monthly_traffic_gb": 50}))
            .unwrap();
        p.apply(&mut c);
        assert!(c.relay.enabled && c.nostr.enabled);
        assert!(c.turn.enabled && c.turn.stun_only);
        assert_eq!(c.limits.monthly_traffic_gb, 50);
        assert_eq!(c.limits.connection_kbps, 0);
        assert!(c.problems().is_empty(), "{:?}", c.problems());
    }

    #[test]
    fn nonsense_from_the_browser_is_refused() {
        assert!(Panel::from_json(&json!({"turn": "everything"})).is_err());
        assert!(Panel::from_json(&json!({"nope": 1})).is_err());
        assert!(Panel::from_json(&json!({"relay": "yes"})).is_err());
        assert!(Panel::from_json(&json!({"monthly_traffic_gb": -1})).is_err());
    }

    #[test]
    fn the_choices_survive_a_restart() {
        let d = tempfile::tempdir().unwrap();
        let p = Panel {
            relay: Some(false),
            turn: Some("turn".into()),
            ..Panel::default()
        };
        p.save(d.path()).unwrap();
        assert_eq!(Panel::load(d.path()), p);
        assert_eq!(Panel::load(&d.path().join("nowhere")), Panel::default());
    }
}
