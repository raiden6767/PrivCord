//! Small, pure answers the screens show, computed once here so Windows and Android cannot disagree: the privacy
//! checkup, the list of hosts the app may contact, and a message's delivery state in words. Each takes JSON and gives
//! JSON, so the apps need no per-feature code beyond drawing the result.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const WEEK_MS: i64 = 7 * 24 * 3600 * 1000;

#[derive(Debug, Deserialize)]
struct CheckupInput {
    app_lock: bool,
    /// When a backup was last made (milliseconds), or 0.
    last_backup_at: i64,
    hide_ip: bool,
    previews_off: bool,
    group_only: bool,
    panic_wipe: bool,
    stale_devices: u32,
    now_ms: i64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct CheckupItem {
    id: &'static str,
    label: &'static str,
    ok: bool,
    hint: String,
}

fn item(id: &'static str, label: &'static str, ok: bool, on: &str, off: &str) -> CheckupItem {
    CheckupItem {
        id,
        label,
        ok,
        hint: if ok { on } else { off }.to_owned(),
    }
}

/// Seven privacy basics and which of them hold.
fn privacy_checkup(i: &CheckupInput) -> Vec<CheckupItem> {
    let recent = i.last_backup_at > 0 && i.now_ms - i.last_backup_at < 4 * WEEK_MS;
    let stale = i.stale_devices;
    let plural = if stale == 1 { "" } else { "s" };
    let it = if stale == 1 { "it" } else { "them" };
    vec![
        item("lock", "App lock", i.app_lock, "Opening the app asks for your own sign-in.", "Anyone holding your unlocked device can read your chats. Turn on the lock."),
        item("backup", "Backup", recent, "A recent backup exists.", "No recent backup. Without one, a lost device means lost chats (your 12 words only restore your identity)."),
        item("ip", "IP address hidden from friends", i.hide_ip, "Connections go through a relay or Tor, so friends do not see your address.", "Friends can see your IP address on a direct connection. Optional, slower."),
        item("previews", "Notifications hide message text", i.previews_off, "Notifications only say a message arrived.", "Message text can appear on a locked screen. Turn off message previews."),
        item("grouponly", "Group-only with people met in groups", i.group_only, "Strangers from a group cannot message you directly.", "People you only know from a group can message you directly."),
        item("panic", "Panic wipe ready", i.panic_wipe, "One gesture erases this device.", "Optional: a one-gesture wipe for emergencies."),
        CheckupItem {
            id: "devices",
            label: "No forgotten devices",
            ok: stale == 0,
            hint: if stale == 0 {
                "Every linked device was seen recently.".to_owned()
            } else {
                format!("{stale} linked device{plural} not seen for 90 days. Remove {it} if lost.")
            },
        },
    ]
}

#[derive(Debug, Deserialize)]
struct HostsInput {
    /// "AUTOMATIC", "BLUETOOTH_ONLY" or "INTERNET_ONLY".
    mode: String,
    force_relay: bool,
    nostr: Vec<String>,
    relays: Vec<String>,
    stun: Vec<String>,
    turn: Vec<String>,
    /// Private mode (Tor): the relays are reached through Tor and nothing is contacted directly.
    #[serde(default)]
    tor: bool,
    /// Update checks are on. They contact GitHub directly, so they are off behind a relay or Tor.
    #[serde(default)]
    updates: bool,
}

fn host_of(url: &str) -> String {
    let rest = url
        .split_once("://")
        .map(|(_, r)| r)
        .or_else(|| {
            let lower = url.to_ascii_lowercase();
            ["stuns:", "stun:", "turns:", "turn:"]
                .iter()
                .find(|p| lower.starts_with(**p))
                .map(|p| url.get(p.len()..).unwrap_or(url))
        })
        .unwrap_or(url);
    rest.split(['/', '?']).next().unwrap_or(rest).to_owned()
}

/// Every internet host the settings allow, and why. Nothing else is ever contacted.
fn network_hosts(i: &HostsInput) -> Vec<Value> {
    let mut rows: Vec<(String, &'static str)> = Vec::new();
    let mut add = |host: String, why: &'static str| {
        if !host.is_empty() && !rows.iter().any(|(h, w)| *h == host && *w == why) {
            rows.push((host, why));
        }
    };
    if i.mode != "BLUETOOTH_ONLY" {
        for r in &i.nostr {
            add(
                host_of(r),
                if i.tor {
                    "Finds your friend and passes sealed hellos (Nostr relay, reached through Tor)"
                } else {
                    "Finds your friend and passes sealed hellos (Nostr relay)"
                },
            );
        }
        for r in &i.relays {
            add(
                host_of(r),
                if i.tor {
                    "Finds your friend and passes sealed messages (your own relay, reached through Tor)"
                } else {
                    "Finds your friend and passes sealed messages (your own relay)"
                },
            );
        }
        // Private mode makes no direct connections, so no STUN or TURN server is ever contacted.
        if !i.force_relay && !i.tor {
            for s in &i.stun {
                add(
                    host_of(s),
                    "Tells your device its public address so a direct connection can start (STUN)",
                );
            }
        }
        if !i.tor {
            for t in &i.turn {
                add(
                    host_of(t),
                    "Carries encrypted traffic when a direct connection is not possible (TURN)",
                );
            }
        }
    }
    // Whatever the connection mode: a check for a new version goes to GitHub directly (and to the download servers
    // it points to), unless a relay or Tor hides this address, when the check is not made at all.
    if i.updates && !i.force_relay && !i.tor {
        add(
            "github.com".to_owned(),
            "Looks for a new version and downloads it, while update checks are on. GitHub can see your IP address",
        );
    }
    rows.into_iter()
        .map(|(host, why)| json!({ "host": host, "why": why }))
        .collect()
}

/// What protects a chat, in words, for the little lock under the chat's name. Honest about public channels.
fn chat_security(kind: &str) -> Value {
    match kind {
        "channel" => json!({
            "headline": "Public channel: not private",
            "lines": [
                "Anyone who knows the channel name can read it and post in it.",
                "Posts are signed, so nobody can pretend to be you, but they are not secret.",
            ],
        }),
        "group" => json!({
            "headline": "End-to-end encrypted group",
            "lines": [
                "Each message is sealed on your device for each member; relays and servers see only padded, sealed data.",
                "New keys are agreed with a post-quantum hybrid (ML-KEM-768 with P-256) when everyone is on a current version.",
                "Group changes are signed by the people who made them.",
            ],
        }),
        _ => json!({
            "headline": "End-to-end encrypted",
            "lines": [
                "Only you and your friend can read these messages. Relays and servers see only padded, sealed data.",
                "The key exchange is a post-quantum hybrid (ML-KEM-768 with P-256), and the keys change as you talk.",
                "Compare the safety number with your friend once to be sure it is really them.",
            ],
        }),
    }
}

#[derive(Debug, Deserialize)]
struct HealthInput {
    /// "android" or "windows".
    platform: String,
    /// "online", "connecting", "offline" or "unconfigured" (no relay set up).
    signaling: String,
    notifications_enabled: bool,
    /// The phone's permission to show notifications (null when the platform has no such switch).
    notification_permission: Option<bool>,
    /// Battery restrictions off for this app (null when not applicable).
    battery_unrestricted: Option<bool>,
    /// Allowed to start by itself / keep running in the background (null when not applicable).
    background_allowed: Option<bool>,
    quiet_hours_now: bool,
    muted_chats: u32,
}

#[derive(Debug, Serialize)]
struct HealthItem {
    id: &'static str,
    ok: bool,
    title: &'static str,
    hint: String,
    /// What the app can offer to do: "notification_settings", "battery_settings", "background_settings",
    /// "connection_settings" or "none".
    fix: &'static str,
}

fn health(i: &HealthInput) -> Value {
    let android = i.platform == "android";
    let conn_ok = i.signaling == "online";
    let conn_hint = match i.signaling.as_str() {
        "online" => "Connected to the relays: friends can find you.",
        "connecting" => {
            "Still connecting to the relays. This can take a moment after the network changes."
        }
        "unconfigured" => {
            "No relay is set up, so friends cannot find you unless you swapped QR codes in person."
        }
        _ => "Cannot reach the relays. Check the internet connection, then Settings > Connection.",
    };
    let mut items = vec![HealthItem {
        id: "connection",
        ok: conn_ok,
        title: "Connected",
        hint: conn_hint.to_owned(),
        fix: if conn_ok {
            "none"
        } else {
            "connection_settings"
        },
    }];
    items.push(HealthItem {
        id: "notifications",
        ok: i.notifications_enabled && i.notification_permission != Some(false),
        title: "Notifications allowed",
        hint: if !i.notifications_enabled {
            "Notifications are turned off in the app settings.".to_owned()
        } else if i.notification_permission == Some(false) {
            "The system is blocking notifications for this app. Allow them in the system settings."
                .to_owned()
        } else {
            "New messages will show a notification.".to_owned()
        },
        fix: if i.notifications_enabled && i.notification_permission != Some(false) {
            "none"
        } else if i.notifications_enabled {
            "notification_settings"
        } else {
            "none"
        },
    });
    if let Some(unrestricted) = i.battery_unrestricted {
        items.push(HealthItem {
            id: "battery",
            ok: unrestricted,
            title: "Battery restriction off",
            hint: if unrestricted {
                "The system will not put the app to sleep, so messages arrive with the app closed.".to_owned()
            } else {
                "The system may put the app to sleep and hold messages back until you open it. Set battery use to Unrestricted.".to_owned()
            },
            fix: if unrestricted { "none" } else { "battery_settings" },
        });
    }
    if let Some(allowed) = i.background_allowed {
        items.push(HealthItem {
            id: "background",
            ok: allowed,
            title: if android { "Allowed to run in the background" } else { "Keeps running when the window is closed" },
            hint: if allowed {
                "The app keeps listening in the background.".to_owned()
            } else if android {
                "Turn on Keep running in the background (Settings > Notifications) so messages arrive with the app closed.".to_owned()
            } else {
                "Turn on Keep running in the tray (Settings > Notifications) so messages arrive with the window closed.".to_owned()
            },
            fix: "background_settings",
        });
    }
    items.push(HealthItem {
        id: "quiet",
        ok: !i.quiet_hours_now,
        title: "Not in quiet hours",
        hint: if i.quiet_hours_now {
            "Quiet hours are on right now, so notifications are held back (messages still arrive)."
                .to_owned()
        } else {
            "Quiet hours are not holding anything back.".to_owned()
        },
        fix: "none",
    });
    items.push(HealthItem {
        id: "muted",
        ok: i.muted_chats == 0,
        title: "No muted chats",
        hint: if i.muted_chats == 0 {
            "No chat is muted.".to_owned()
        } else {
            format!(
                "{} chat{} muted. Open it and turn notifications back on if you expected one.",
                i.muted_chats,
                if i.muted_chats == 1 { " is" } else { "s are" }
            )
        },
        fix: "none",
    });
    // "Why am I not receiving?": the first thing that is wrong, in the order that matters most.
    let order = [
        "connection",
        "battery",
        "background",
        "notifications",
        "quiet",
        "muted",
    ];
    let culprit = order
        .iter()
        .find_map(|id| items.iter().find(|it| it.id == *id && !it.ok));
    let verdict = match culprit {
        Some(it) => json!({ "ok": false, "id": it.id, "text": it.hint, "fix": it.fix }),
        None => {
            json!({ "ok": true, "id": "none", "text": "Everything that lets messages reach you looks right. If a friend says they sent something, ask them to check that they are online too.", "fix": "none" })
        }
    };
    json!({ "items": items, "verdict": verdict })
}

#[derive(Debug, Deserialize)]
struct DeviceBrief {
    name: String,
    last_seen: Option<i64>,
    this_device: bool,
    stale: bool,
}

#[derive(Debug, Deserialize)]
struct DevicesInput {
    devices: Vec<DeviceBrief>,
    now_ms: i64,
}

fn ago(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    match s {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// "2 devices, synced a minute ago": one line for the Settings strip.
fn devices_summary(i: &DevicesInput) -> Value {
    let others: Vec<&DeviceBrief> = i.devices.iter().filter(|d| !d.this_device).collect();
    let line = if others.is_empty() {
        "Only this device so far.".to_owned()
    } else {
        let newest = others.iter().filter_map(|d| d.last_seen).max();
        let stale = others.iter().filter(|d| d.stale).count();
        let seen = match newest {
            Some(t) => format!("synced {}", ago(i.now_ms - t)),
            None => "not synced yet".to_owned(),
        };
        let mut line = format!("{} devices, {seen}", i.devices.len());
        if stale > 0 {
            line.push_str(&format!(". {stale} not seen for 90 days"));
        }
        line
    };
    json!({ "line": line, "others": others.iter().map(|d| json!({ "name": d.name, "seen": d.last_seen.map(|t| ago(i.now_ms - t)) })).collect::<Vec<_>>() })
}

/// What is new, per release: one list for every platform. Newest first.
const WHATS_NEW: &[(&str, &[&str])] = &[(
    "0.2.0",
    &[
        "Link a new device by scanning: the new device shows a code, your phone scans it, and you type six digits.",
        "Pin chats, archive them, and mute one for an hour, a day or for good. They follow you to every device.",
        "Undo a message right after sending it.",
        "Quiet hours, send later with ready-made times, and a privacy checkup.",
        "See which servers the app can contact, and why.",
        "PrivCord can update itself: Android downloads in the background, Windows installs a signed update.",
        "Remove a linked device from any of your devices: it waits a day (and can be taken back), or happens at once with your recovery phrase.",
    ],
)];

fn whats_new(version: &str) -> Value {
    let lines = WHATS_NEW
        .iter()
        .find(|(v, _)| *v == version)
        .or_else(|| WHATS_NEW.first())
        .map(|(v, l)| json!({ "version": v, "lines": l }))
        .unwrap_or(Value::Null);
    lines
}

/// One pure answer by name. Unknown names and bad input give an error, never a guess.
pub fn insight(name: &str, input: &str) -> Result<String, String> {
    let bad = |e: serde_json::Error| format!("bad input: {e}");
    match name {
        "checkup" => {
            let i: CheckupInput = serde_json::from_str(input).map_err(bad)?;
            serde_json::to_string(&privacy_checkup(&i)).map_err(bad)
        }
        "hosts" => {
            let i: HostsInput = serde_json::from_str(input).map_err(bad)?;
            serde_json::to_string(&network_hosts(&i)).map_err(bad)
        }
        "chat_security" => {
            let kind: String = serde_json::from_str(input).map_err(bad)?;
            serde_json::to_string(&chat_security(&kind)).map_err(bad)
        }
        "health" => {
            let i: HealthInput = serde_json::from_str(input).map_err(bad)?;
            serde_json::to_string(&health(&i)).map_err(bad)
        }
        "devices_summary" => {
            let i: DevicesInput = serde_json::from_str(input).map_err(bad)?;
            serde_json::to_string(&devices_summary(&i)).map_err(bad)
        }
        "whats_new" => {
            let v: String = serde_json::from_str(input).map_err(bad)?;
            serde_json::to_string(&whats_new(&v)).map_err(bad)
        }
        _ => Err(format!("unknown insight `{name}`")),
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn run(name: &str, input: Value) -> Value {
        serde_json::from_str(&insight(name, &input.to_string()).unwrap()).unwrap()
    }

    #[test]
    fn the_checkup_says_what_holds_and_what_to_fix() {
        let none = run(
            "checkup",
            json!({"app_lock": false, "last_backup_at": 0, "hide_ip": false, "previews_off": false, "group_only": false, "panic_wipe": false, "stale_devices": 0, "now_ms": 1_000_000_000_000_i64}),
        );
        let ok = none
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["ok"] == true)
            .count();
        assert_eq!(ok, 1, "only 'no forgotten devices' holds");
        let all = run(
            "checkup",
            json!({"app_lock": true, "last_backup_at": 999_999_999_000_i64, "hide_ip": true, "previews_off": true, "group_only": true, "panic_wipe": true, "stale_devices": 0, "now_ms": 1_000_000_000_000_i64}),
        );
        assert!(all.as_array().unwrap().iter().all(|i| i["ok"] == true));
        let old = run(
            "checkup",
            json!({"app_lock": true, "last_backup_at": 1, "hide_ip": true, "previews_off": true, "group_only": true, "panic_wipe": true, "stale_devices": 2, "now_ms": 1_000_000_000_000_i64}),
        );
        assert_eq!(old[1]["ok"], false, "an old backup counts as none");
        assert!(old[6]["hint"]
            .as_str()
            .unwrap()
            .contains("2 linked devices"));
    }

    #[test]
    fn hosts_follow_the_mode_and_drop_stun_behind_a_relay() {
        let base = json!({"mode": "AUTOMATIC", "force_relay": false, "nostr": ["wss://relay.damus.io"], "relays": ["wss://my.example.org/path"], "stun": ["stun:stun.example.net:3478"], "turn": []});
        let rows = run("hosts", base.clone());
        let hosts: Vec<&str> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["host"].as_str().unwrap())
            .collect();
        assert_eq!(
            hosts,
            ["relay.damus.io", "my.example.org", "stun.example.net:3478"]
        );
        let mut bt = base.clone();
        bt["mode"] = json!("BLUETOOTH_ONLY");
        assert_eq!(run("hosts", bt), json!([]));
        let mut relayed = base;
        relayed["force_relay"] = json!(true);
        relayed["turn"] = json!(["turn:turn.example.org:3478"]);
        let rows = run("hosts", relayed);
        assert!(!rows
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["why"].as_str().unwrap().contains("STUN")));
        assert_eq!(
            rows.as_array().unwrap().last().unwrap()["host"],
            "turn.example.org:3478"
        );
    }

    #[test]
    fn private_mode_lists_no_direct_hosts_and_updates_list_github() {
        let base = json!({"mode": "AUTOMATIC", "force_relay": false, "nostr": ["wss://relay.damus.io"], "relays": [], "stun": ["stun:stun.example.net:3478"], "turn": ["turn:turn.example.org:3478"], "tor": true, "updates": true});
        let hosts = |v: Value| -> Vec<String> {
            run("hosts", v)
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["host"].as_str().unwrap().to_owned())
                .collect()
        };
        // Tor: only the relays (through Tor); no STUN, no TURN, and no update check (it would show the address).
        assert_eq!(hosts(base.clone()), ["relay.damus.io"]);
        let mut open = base.clone();
        open["tor"] = json!(false);
        assert_eq!(
            hosts(open.clone()),
            [
                "relay.damus.io",
                "stun.example.net:3478",
                "turn.example.org:3478",
                "github.com"
            ]
        );
        // Update checks are listed even when no other host is, and not when they are off or behind a relay.
        let mut bt = open.clone();
        bt["mode"] = json!("BLUETOOTH_ONLY");
        assert_eq!(hosts(bt), ["github.com"]);
        let mut off = open.clone();
        off["updates"] = json!(false);
        assert!(!hosts(off).contains(&"github.com".to_owned()));
        let mut relayed = open;
        relayed["force_relay"] = json!(true);
        assert!(!hosts(relayed).contains(&"github.com".to_owned()));
    }

    fn health_input(over: Value) -> Value {
        let mut base = json!({"platform": "android", "signaling": "online", "notifications_enabled": true, "notification_permission": true, "battery_unrestricted": true, "background_allowed": true, "quiet_hours_now": false, "muted_chats": 0});
        for (k, v) in over.as_object().unwrap() {
            base[k] = v.clone();
        }
        base
    }

    #[test]
    fn the_verdict_names_the_first_thing_that_stops_messages() {
        let ok = run("health", health_input(json!({})));
        assert_eq!(ok["verdict"]["ok"], true);
        let off = run(
            "health",
            health_input(json!({"signaling": "offline", "battery_unrestricted": false})),
        );
        assert_eq!(
            off["verdict"]["id"], "connection",
            "no connection beats battery"
        );
        let battery = run(
            "health",
            health_input(json!({"battery_unrestricted": false, "quiet_hours_now": true})),
        );
        assert_eq!(battery["verdict"]["id"], "battery");
        assert_eq!(battery["verdict"]["fix"], "battery_settings");
        let blocked = run(
            "health",
            health_input(json!({"notification_permission": false})),
        );
        assert_eq!(blocked["verdict"]["fix"], "notification_settings");
        let quiet = run(
            "health",
            health_input(json!({"quiet_hours_now": true, "muted_chats": 2})),
        );
        assert_eq!(quiet["verdict"]["id"], "quiet");
        // Windows has no battery or notification-permission switch: those rows are left out.
        let win = run(
            "health",
            health_input(
                json!({"platform": "windows", "battery_unrestricted": null, "notification_permission": null}),
            ),
        );
        assert!(!win["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == "battery"));
    }

    #[test]
    fn the_security_text_is_honest_about_public_channels() {
        let c = run("chat_security", json!("channel"));
        assert!(c["headline"].as_str().unwrap().contains("not private"));
        assert!(run("chat_security", json!("direct"))["headline"]
            .as_str()
            .unwrap()
            .contains("End-to-end"));
        assert!(
            run("chat_security", json!("group"))["lines"]
                .as_array()
                .unwrap()
                .len()
                >= 2
        );
    }

    #[test]
    fn the_devices_line_says_when_the_others_last_synced() {
        let now = 1_000_000_000_000_i64;
        let one = run(
            "devices_summary",
            json!({"devices": [{"name": "Phone", "last_seen": null, "this_device": true, "stale": false}], "now_ms": now}),
        );
        assert_eq!(one["line"], "Only this device so far.");
        let two = run(
            "devices_summary",
            json!({"devices": [
            {"name": "Phone", "last_seen": null, "this_device": true, "stale": false},
            {"name": "Laptop", "last_seen": now - 3 * 60_000, "this_device": false, "stale": false}], "now_ms": now}),
        );
        assert_eq!(two["line"], "2 devices, synced 3 min ago");
        let old = run(
            "devices_summary",
            json!({"devices": [
            {"name": "Phone", "last_seen": null, "this_device": true, "stale": false},
            {"name": "Old", "last_seen": now - 100 * 86_400_000, "this_device": false, "stale": true}], "now_ms": now}),
        );
        assert!(old["line"]
            .as_str()
            .unwrap()
            .contains("1 not seen for 90 days"));
    }

    #[test]
    fn whats_new_has_lines_and_falls_back_to_the_newest() {
        assert!(!run("whats_new", json!("0.2.0"))["lines"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(run("whats_new", json!("9.9.9"))["version"], "0.2.0");
    }

    #[test]
    fn bad_calls_are_errors_not_guesses() {
        assert!(insight("nope", "{}").is_err());
        assert!(insight("checkup", "{").is_err());
    }
}
