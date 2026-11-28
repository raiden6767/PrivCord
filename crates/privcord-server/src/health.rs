//! The server's condition in plain words (for the admin panel and a report to download), and the live numbers the panel shows.
//!
//! Every check says what it found, what that means for the people using the server, and what to do about it. Nothing here
//! changes anything: it only looks.

use crate::net::App;
use serde_json::{json, Value};
use std::path::Path;

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Nothing to do.
    Good,
    /// Worth knowing; nothing is broken.
    Watch,
    /// Something is wrong or at risk now.
    Fix,
}

impl Level {
    pub fn word(self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Watch => "watch",
            Self::Fix => "fix",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Check {
    pub level: Level,
    /// A short name of what was looked at.
    pub title: String,
    /// What was found and what it means, in plain words.
    pub found: String,
    /// What to do (empty when nothing).
    pub todo: String,
}

fn num(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}

fn check(level: Level, title: &str, found: impl Into<String>, todo: &str) -> Check {
    Check {
        level,
        title: title.to_owned(),
        found: found.into(),
        todo: todo.to_owned(),
    }
}

/// Resident memory of this process, in kilobytes (Linux).
pub fn memory_kb() -> Option<u64> {
    status_kb("VmRSS:")
}

fn status_kb(key: &str) -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix(key))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|n| n.parse().ok())
}

/// Memory the machine could still give out, in kilobytes (Linux).
fn memory_available_kb() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("MemAvailable:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|n| n.parse().ok())
}

/// CPU seconds this process has used (Linux): the panel turns two readings into a percentage.
fn cpu_secs() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/self/stat").ok()?;
    // The command name may hold spaces: fields count from after its closing bracket.
    let rest = text.get(text.rfind(')')? + 2..)?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    let (u, s): (f64, f64) = (f.get(11)?.parse().ok()?, f.get(12)?.parse().ok()?);
    Some((u + s) / 100.0)
}

/// Free and total bytes of the disk holding `path`, from the system's `df` (no unsafe code, and no extra library). It is asked at
/// most every half minute, however often the panel refreshes.
pub fn disk(path: &Path) -> Option<(u64, u64)> {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    type Reading = Option<(Instant, Option<(u64, u64)>)>;
    static CACHE: OnceLock<Mutex<Reading>> = OnceLock::new();
    let cell = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cell
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((at, v)) = *guard {
        if at.elapsed() < Duration::from_secs(30) {
            return v;
        }
    }
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok());
    // POSIX output: a header, then "Filesystem 1024-blocks Used Available Capacity Mounted".
    let value = out.and_then(|text| {
        let f: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
        let total = f.get(1)?.parse::<u64>().ok()?.checked_mul(1024)?;
        let free = f.get(3)?.parse::<u64>().ok()?.checked_mul(1024)?;
        Some((free, total))
    });
    *guard = Some((Instant::now(), value));
    value
}

/// The size of everything in the data folder (it is flat, with a few small folders).
pub fn data_bytes(dir: &Path) -> u64 {
    fn walk(dir: &Path, depth: u8) -> u64 {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return 0;
        };
        rd.filter_map(Result::ok)
            .map(|e| match e.metadata() {
                Ok(m) if m.is_dir() && depth < 3 => walk(&e.path(), depth + 1),
                Ok(m) => m.len(),
                Err(_) => 0,
            })
            .sum()
    }
    walk(dir, 0)
}

/// When the last backup was made (seconds since 1970), if one was: the backup command writes the time next to the data.
pub fn last_backup(dir: &Path) -> Option<u64> {
    std::fs::read_to_string(dir.join("last-backup"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

fn mb_of_kb(kb: u64) -> String {
    format!("{:.0} MB", kb as f64 / 1024.0)
}

fn span(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("{d} day{} {h} hour{}", plural(d), plural(h))
    } else if h > 0 {
        format!("{h} hour{} {m} minute{}", plural(h), plural(m))
    } else {
        format!("{m} minute{}", plural(m))
    }
}

fn plural(n: u64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// What was found, worst first.
pub fn checks(app: &App, data_dir: &Path, self_signed: bool) -> Vec<Check> {
    let c = &app.cfg;
    let mut out = Vec::new();

    // Who can connect.
    if c.relay.enabled {
        if app.requires_token() {
            out.push(check(
                Level::Good,
                "Who can connect",
                "Only people with a token can connect.",
                "",
            ));
        } else {
            out.push(check(
                Level::Watch,
                "Who can connect",
                "Anyone who finds this server's address can use it, and take its space and bandwidth.",
                "Make a token for each person under People. It takes a minute and makes the server private.",
            ));
        }
    }

    // Disk space.
    match disk(data_dir) {
        Some((free, total)) if total > 0 => {
            let pct = free as f64 * 100.0 / total as f64;
            let line = format!(
                "{} of {} left on the disk ({pct:.0}%). The server's own files take {}.",
                gb(free),
                gb(total),
                gb(data_bytes(data_dir))
            );
            if free < 500_000_000 || pct < 5.0 {
                out.push(check(Level::Fix, "Disk space", line, "Free some space or move the data folder to a bigger disk. When the disk is full the server cannot keep messages and may stop."));
            } else if pct < 15.0 {
                out.push(check(Level::Watch, "Disk space", line, "Plan more space soon, or lower how long events are kept (Settings > Nostr > retention)."));
            } else {
                out.push(check(Level::Good, "Disk space", line, ""));
            }
        }
        _ => {}
    }

    // Memory.
    if let (Some(rss), Some(avail)) = (memory_kb(), memory_available_kb()) {
        let line = format!(
            "The server uses {}; the machine has {} still free.",
            mb_of_kb(rss),
            mb_of_kb(avail)
        );
        let on_disk = app
            .relay
            .as_ref()
            .and_then(|r| r.nostr())
            .and_then(|n| n.store())
            .is_some_and(|s| s.is_disk());
        if avail < 150 * 1024 {
            out.push(check(Level::Fix, "Memory", line, "The machine is nearly out of memory. Add memory, or lower how many events are kept; with very many events switch nostr.backend to \"disk\"."));
        } else if rss > avail * 2 && !on_disk {
            out.push(check(Level::Watch, "Memory", line, "The server holds a lot compared to what is free. If it keeps growing, set nostr.backend = \"disk\" so memory stops growing with the events."));
        } else {
            out.push(check(Level::Good, "Memory", line, ""));
        }
    }

    // Safe storage.
    let damaged = app.relay.as_ref().map_or(0, |r| {
        r.queue_corrupt()
            + r.nostr()
                .and_then(|n| n.store())
                .map_or(0, |s| s.corrupt_lines())
    });
    if damaged > 0 {
        out.push(check(
            Level::Fix,
            "Stored data",
            format!("{damaged} damaged records were skipped when the server started: the files were changed or corrupted. A copy of each damaged file was kept (named .damaged-…)."),
            "Restore from a backup if messages are missing, and check the disk for faults. Delete the .damaged files once you have looked.",
        ));
    } else if c.storage.checksums || c.storage.encrypt {
        out.push(check(
            Level::Good,
            "Stored data",
            format!(
                "Every record is checked when read{}. Nothing damaged was found.",
                if c.storage.encrypt {
                    " and encrypted on disk"
                } else {
                    ""
                }
            ),
            "",
        ));
    } else {
        out.push(check(
            Level::Watch,
            "Stored data",
            "Records are written as plain lines with no check, so damage would not be noticed.",
            "Turn on storage.checksums.",
        ));
    }
    if !c.storage.encrypt && c.relay.enabled {
        out.push(check(
            Level::Watch,
            "Privacy on disk",
            "Stored messages are sealed by the apps, so this server cannot read them, but who talks to whom and when (mailbox names and times) is readable by anyone who can read the disk.",
            "Make a key with `privcord-server store-key` and turn on storage.encrypt, or encrypt the whole disk.",
        ));
    }

    // Certificates.
    if c.tls.https_listen.is_empty() {
        out.push(check(
            Level::Watch,
            "Encrypted connections",
            "Apps connect without TLS, so addresses and timing are visible on the network.",
            "Set a domain under [tls] with acme = true; the certificate then renews itself.",
        ));
    } else if self_signed {
        out.push(check(
            Level::Fix,
            "Encrypted connections",
            "The certificate is self-signed, so apps and browsers will not trust it.",
            "Use a real certificate: set tls.domain and acme = true, or give certificate files.",
        ));
    } else {
        out.push(check(
            Level::Good,
            "Encrypted connections",
            if c.tls.acme {
                "Connections are encrypted and the certificate renews itself."
            } else {
                "Connections are encrypted."
            },
            "",
        ));
    }

    // Calls.
    if c.turn.enabled && c.turn.stun_only {
        out.push(check(
            Level::Good,
            "Address lookup",
            "The server answers STUN only (it tells a phone which address it is seen at, and relays nothing), so it uses almost no bandwidth.",
            "",
        ));
    } else if c.turn.enabled {
        if c.turn.tcp_listen.is_empty() && c.turn.tls_listen.is_empty() {
            out.push(check(Level::Watch, "Calls and files", "The call server accepts UDP only, so phones on networks that block UDP cannot use it.", "Set turn.tcp_listen and turn.tls_listen."));
        } else {
            out.push(check(
                Level::Good,
                "Calls and files",
                "The call server (TURN) is on and reachable over UDP and TCP.",
                "",
            ));
        }
    }

    // Traffic and disk budgets, when the operator set them.
    let (used, cap) = (app.usage.total(), app.usage.cap());
    if cap > 0 {
        let (level, found, todo) = if used >= cap {
            (Level::Fix, format!("The traffic allowance for this month ({}) is used up: new connections are being refused until next month.", gb(cap)), "Raise the monthly traffic limit under Settings, or wait for the month to change.")
        } else if used * 10 >= cap * 8 {
            (
                Level::Watch,
                format!(
                    "{} of the {} monthly traffic allowance is used.",
                    gb(used),
                    gb(cap)
                ),
                "Raise the limit if you want people to keep connecting until the month ends.",
            )
        } else {
            (
                Level::Good,
                format!(
                    "{} of the {} monthly traffic allowance is used.",
                    gb(used),
                    gb(cap)
                ),
                "",
            )
        };
        out.push(check(level, "Traffic this month", found, todo));
    }
    let quota = c.limits.disk_quota_mb.saturating_mul(1024 * 1024);
    if quota > 0 {
        let held = data_bytes(data_dir);
        let (level, found, todo) = if held >= quota {
            (
                Level::Fix,
                format!(
                    "The disk budget ({}) is full: new messages and events are being refused.",
                    gb(quota)
                ),
                "Raise the disk limit, or clear old stored messages under Settings.",
            )
        } else if held * 10 >= quota * 8 {
            (
                Level::Watch,
                format!("{} of the {} disk budget is used.", gb(held), gb(quota)),
                "Raise the disk limit or clear old data before it fills.",
            )
        } else {
            (
                Level::Good,
                format!("{} of the {} disk budget is used.", gb(held), gb(quota)),
                "",
            )
        };
        out.push(check(level, "Disk budget", found, todo));
    }

    // Backups.
    match last_backup(data_dir) {
        None => out.push(check(Level::Watch, "Backups", "No backup has been made with this server's backup command.", "Run `privcord-server backup --out backup.pcb` regularly, and keep a copy off this machine.")),
        Some(t) if now_secs().saturating_sub(t) > 7 * 86_400 => out.push(check(
            Level::Watch,
            "Backups",
            format!("The last backup was {} ago.", span(now_secs().saturating_sub(t))),
            "Make a new one: `privcord-server backup --out backup.pcb`.",
        )),
        Some(t) => out.push(check(Level::Good, "Backups", format!("The last backup was {} ago.", span(now_secs().saturating_sub(t))), "")),
    }

    // Refused requests.
    if let Some(r) = &app.relay {
        let h = r.health();
        let put = num(&h, "put");
        let rejected = num(&h, "rejected");
        if rejected > 100 && rejected * 5 > put.max(1) {
            out.push(check(
                Level::Watch,
                "Refused requests",
                format!("{rejected} requests were refused, against {put} accepted. That is a lot."),
                "Someone may be hammering the server or using a wrong token. Look at People, and consider blocking an address.",
            ));
        }
    }

    out.sort_by_key(|c| std::cmp::Reverse(c.level));
    out
}

/// Overall: the worst level found.
pub fn overall(checks: &[Check]) -> Level {
    checks.iter().map(|c| c.level).max().unwrap_or(Level::Good)
}

pub fn checks_json(checks: &[Check]) -> Value {
    json!({
        "overall": overall(checks).word(),
        "checks": checks.iter().map(|c| json!({
            "level": c.level.word(), "title": c.title, "found": c.found, "todo": c.todo,
        })).collect::<Vec<_>>(),
    })
}

/// "2026-11-28" and "2026-11-28 05:40 UTC" from seconds since 1970 (no calendar library needed).
pub fn civil(secs: u64) -> (String, String) {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    // Howard Hinnant's days-to-date.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (h, min) = (secs % 86_400 / 3600, secs % 3600 / 60);
    let date = format!("{y:04}-{m:02}-{d:02}");
    (date.clone(), format!("{date} {h:02}:{min:02} UTC"))
}

/// The report to download: plain words, no jargon, nothing secret in it (no tokens, keys or addresses of people).
pub fn report_text(app: &App, data_dir: &Path, self_signed: bool) -> String {
    let checks = checks(app, data_dir, self_signed);
    let (fix, watch): (Vec<_>, Vec<_>) = (
        checks.iter().filter(|c| c.level == Level::Fix).collect(),
        checks.iter().filter(|c| c.level == Level::Watch).collect(),
    );
    let good: Vec<_> = checks.iter().filter(|c| c.level == Level::Good).collect();
    let mut t = String::new();
    t.push_str("PrivCord server: health report\n");
    t.push_str("==============================\n\n");
    t.push_str(&format!(
        "Made:        {}\nVersion:     {}\nRunning for: {}\n\n",
        civil(now_secs()).1,
        env!("CARGO_PKG_VERSION"),
        span(app.started.elapsed().as_secs())
    ));
    t.push_str(&format!(
        "Overall: {}\n\n",
        match overall(&checks) {
            Level::Good => "all good".to_owned(),
            _ => format!("{} to fix, {} to keep an eye on", fix.len(), watch.len()),
        }
    ));
    let mut section = |title: &str, list: &[&Check]| {
        if list.is_empty() {
            return;
        }
        t.push_str(&format!("{title}\n{}\n", "-".repeat(title.len())));
        for (i, c) in list.iter().enumerate() {
            t.push_str(&format!("{}. {}\n   {}\n", i + 1, c.title, c.found));
            if !c.todo.is_empty() {
                t.push_str(&format!("   What to do: {}\n", c.todo));
            }
            t.push('\n');
        }
    };
    section("NEEDS FIXING", &fix);
    section("WORTH WATCHING", &watch);
    section("ALL GOOD", &good);
    let live = live(app, data_dir);
    t.push_str("THE NUMBERS\n-----------\n");
    let relay = live.get("relay").cloned().unwrap_or(Value::Null);
    t.push_str(&format!(
        "Devices connected now:                 {}\n",
        num(&relay, "open_connections")
    ));
    t.push_str(&format!(
        "Messages waiting for offline phones:   {}\n",
        num(&relay, "queued")
    ));
    t.push_str(&format!(
        "Mailboxes holding messages:            {}\n",
        num(&relay, "mailboxes")
    ));
    if let Some(nostr) = live.get("nostr").filter(|v| v.is_object()) {
        t.push_str(&format!(
            "Nostr events stored:                   {}\n",
            num(nostr, "events")
        ));
    }
    if let Some(kb) = live.get("memory_kb").and_then(Value::as_u64) {
        t.push_str(&format!(
            "Memory used by the server:             {}\n",
            mb_of_kb(kb)
        ));
    }
    t.push_str("\nThis report holds no tokens, keys, messages or people's addresses, so it is safe to send to someone who is helping.\n");
    t
}

/// The live numbers the panel shows and refreshes every few seconds. The panel works out rates from two readings.
pub fn live(app: &App, data_dir: &Path) -> Value {
    let relay = app.relay.as_ref().map_or(json!({}), |r| {
        let mut h = r.health();
        if let Some(o) = h.as_object_mut() {
            o.remove("ok");
            o.remove("nostr");
        }
        h
    });
    let nostr = app
        .relay
        .as_ref()
        .and_then(|r| r.nostr())
        .map(|n| {
            let mut v = n.counters();
            if let (Some(o), Some(s)) = (v.as_object_mut(), n.store()) {
                o.insert("on_disk".into(), json!(s.is_disk()));
            }
            v
        })
        .unwrap_or(Value::Null);
    let disk = disk(data_dir);
    json!({
        "at": now_secs(),
        "uptime_secs": app.started.elapsed().as_secs(),
        "memory_kb": memory_kb(),
        "memory_free_kb": memory_available_kb(),
        "threads": status_kb("Threads:"),
        "cpu_secs": cpu_secs(),
        "disk_free": disk.map(|d| d.0),
        "disk_total": disk.map(|d| d.1),
        "data_bytes": data_bytes(data_dir),
        "relay": relay,
        "nostr": nostr,
        "usage": app.usage.snapshot(),
    })
}
