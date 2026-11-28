//! A diagnostic log kept on this device, for finding out why something did not work.
//!
//! What the engine and the screens did, in order: links going up and down, connection setup, relay posts and what came
//! back, retries, requests, each action a person took and how it ended. Nothing leaves the device: the person saves it
//! as a Markdown file and sends it if they choose to. It is on by default and can be turned off (which also empties it).
//!
//! What it never holds: message text, names, keys, recovery words, addresses. Every long identifier (a friend's id, a
//! mailbox name) is replaced by a short alias (`id-3`), the same one throughout one log, and anything that looks like a
//! network address is replaced by `<ip>`. The log lives in the encrypted database, is capped in size, and is erased with
//! everything else ("Erase everything", the panic wipe).

use super::{Core, StorageOp};
use std::collections::{BTreeMap, VecDeque};

const KV_META: &str = "diag.meta.v1";
const KV_CHUNK: &str = "diag.c.";
const KV_ON: &str = "diag.enabled";
/// Entries kept; older ones are dropped (and counted). A long test is the point, so this is generous: the log is saved in
/// chunks, and only the newest chunk is rewritten as it grows.
const MAX_LINES: usize = 20_000;
const CHUNK: u64 = 400;
/// The same event again within this many milliseconds is counted on the last entry instead of added.
const REPEAT_MS: i64 = 2_000;
const MAX_DETAIL_CHARS: usize = 220;
const SAVE_EVERY_MS: i64 = 10_000;
/// Aliases handed out in one log.
const MAX_ALIASES: usize = 400;
/// Where an entry may come from. The platform says which; anything else is filed under "app".
const CATEGORIES: [&str; 14] = [
    "app", "ui", "nav", "net", "perm", "ble", "relay", "link", "send", "invite", "sync", "error",
    "engine", "call",
];

#[derive(Clone)]
struct Line {
    at: i64,
    cat: String,
    event: String,
    detail: String,
    /// How many times in a row (1 = once).
    n: u32,
    /// When it last happened (not kept; `at` is when it first did).
    last_at: i64,
}

#[derive(Default)]
pub(super) struct DiagState {
    loaded: bool,
    /// `None` until read from storage.
    on: Option<bool>,
    lines: VecDeque<Line>,
    aliases: BTreeMap<String, usize>,
    dropped: u64,
    /// The position (over the whole life of the log) of `lines[0]`.
    base: u64,
    /// The lowest position changed since the last save.
    dirty_from: Option<u64>,
    /// Chunks below this number have been emptied in storage.
    emptied_to: u64,
    saved_at: i64,
    since: i64,
}

/// What the alias table is keyed by: a hash of the identifier, never the identifier. The table is saved with the log, and a
/// phone's saved log must not hold a friend's address or a mailbox (a friend who was removed would still be listed).
fn alias_key(id: &str) -> String {
    format!(
        "h:{}",
        privcord_crypto::kdf::sha256_hex(format!("privcord-diag-alias|{id}").as_bytes())
            .chars()
            .take(16)
            .collect::<String>()
    )
}

/// A text with every identifier and address made harmless: long hex runs become aliases, anything long and unbroken
/// becomes `<long>`, IP-looking pieces become `<ip>`.
fn redact(text: &str, aliases: &mut BTreeMap<String, usize>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String, aliases: &mut BTreeMap<String, usize>| {
        if word.is_empty() {
            return;
        }
        let w = std::mem::take(word);
        if w.len() >= 16 && w.bytes().all(|b| b.is_ascii_hexdigit()) {
            let next = aliases.len() + 1;
            let w = alias_key(&w);
            let n = if aliases.contains_key(&w) || aliases.len() < MAX_ALIASES {
                *aliases.entry(w).or_insert(next)
            } else {
                0
            };
            out.push_str(&format!("id-{n}"));
        } else if w.len() > 40 {
            out.push_str("<long>");
        } else {
            out.push_str(&w);
        }
    };
    for piece in text.split_inclusive(|c: char| c.is_whitespace() || "[](){}<>\"'=,;|".contains(c))
    {
        let (body, tail) = match piece.char_indices().last() {
            Some((i, c)) if c.is_whitespace() || "[](){}<>\"'=,;|".contains(c) => {
                (&piece[..i], &piece[i..])
            }
            _ => (piece, ""),
        };
        if looks_like_address(body) {
            out.push_str("<ip>");
        } else {
            for c in body.chars() {
                if c.is_ascii_alphanumeric() {
                    word.push(c);
                } else {
                    flush(&mut word, &mut out, aliases);
                    out.push(c);
                }
            }
            flush(&mut word, &mut out, aliases);
        }
        out.push_str(tail);
    }
    out
}

/// An IPv4 address (with or without a port), or something with two or more colons that is hex (an IPv6 address).
fn looks_like_address(piece: &str) -> bool {
    let p = piece.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == ':'));
    let v4 = |s: &str| {
        let parts: Vec<&str> = s.split('.').collect();
        parts.len() == 4
            && parts
                .iter()
                .all(|x| !x.is_empty() && x.len() <= 3 && x.bytes().all(|b| b.is_ascii_digit()))
    };
    let host = p
        .rsplit_once(':')
        .filter(|(_, port)| port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(p, |(h, _)| h);
    if v4(host) {
        return true;
    }
    p.matches(':').count() >= 2
        && p.bytes()
            .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
}

fn clip(text: &str, max: usize) -> String {
    let one: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if one.chars().count() <= max {
        one
    } else {
        let mut s: String = one.chars().take(max).collect();
        s.push('…');
        s
    }
}

impl Core {
    /// Whether the log is being kept (it is, until the person turns it off).
    pub fn diag_enabled(&mut self) -> bool {
        if self.diag.on.is_none() {
            let off = matches!(self.ports.storage.kv(KV_ON), Ok(Some(v)) if v == b"0");
            self.diag.on = Some(!off);
        }
        self.diag.on.unwrap_or(true)
    }

    /// Turns the log on or off. Turning it off empties it.
    pub fn set_diag_enabled(&mut self, on: bool) {
        if !on {
            self.diag_clear();
        }
        self.diag.on = Some(on);
        self.hub.set_tap(on);
        let _ = self.ports.storage.apply(&[StorageOp::SetKv {
            key: KV_ON.into(),
            value: vec![u8::from(on) + b'0'],
        }]);
        if on {
            self.diag("app", "log on", "");
        }
    }

    /// Empties the log.
    pub fn diag_clear(&mut self) {
        self.diag_load();
        let end = self.diag.base + self.diag.lines.len() as u64;
        let mut ops = vec![StorageOp::SetKv {
            key: KV_META.into(),
            value: Vec::new(),
        }];
        for c in (self.diag.base / CHUNK)..=(end / CHUNK) {
            ops.push(StorageOp::SetKv {
                key: format!("{KV_CHUNK}{c}"),
                value: Vec::new(),
            });
        }
        let _ = self.ports.storage.apply(&ops);
        self.diag.base = end;
        self.diag.emptied_to = end / CHUNK + 1;
        self.diag.lines.clear();
        self.diag.aliases.clear();
        self.diag.dropped = 0;
        self.diag.dirty_from = None;
    }

    /// How many entries are in the log.
    pub fn diag_len(&mut self) -> usize {
        self.diag_load();
        self.diag.lines.len()
    }

    fn diag_load(&mut self) {
        if self.diag.loaded {
            return;
        }
        self.diag.loaded = true;
        self.diag.since = self.now();
        let Some(meta) = self
            .ports
            .storage
            .kv(KV_META)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        else {
            return;
        };
        let base = meta
            .get("base")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let end = meta
            .get("end")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(base);
        self.diag.dropped = meta
            .get("dropped")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if let Some(o) = meta.get("ids").and_then(serde_json::Value::as_object) {
            for (k, n) in o {
                if let Some(n) = n.as_u64().and_then(|n| usize::try_from(n).ok()) {
                    // A table saved by an older version is keyed by the identifiers themselves: re-keyed here.
                    let key = if k.starts_with("h:") {
                        k.clone()
                    } else {
                        alias_key(k)
                    };
                    self.diag.aliases.insert(key, n);
                }
            }
        }
        self.diag.base = base;
        self.diag.emptied_to = base / CHUNK;
        let mut pos = base / CHUNK * CHUNK;
        for c in (base / CHUNK)..=(end.saturating_sub(1) / CHUNK) {
            let rows = self
                .ports
                .storage
                .kv(&format!("{KV_CHUNK}{c}"))
                .ok()
                .flatten()
                .and_then(|b| serde_json::from_slice::<Vec<serde_json::Value>>(&b).ok())
                .unwrap_or_default();
            for l in rows {
                if pos >= base && pos < end {
                    if let (Some(at), Some(cat), Some(event), Some(detail)) = (
                        l.get(0).and_then(serde_json::Value::as_i64),
                        l.get(1).and_then(serde_json::Value::as_str),
                        l.get(2).and_then(serde_json::Value::as_str),
                        l.get(3).and_then(serde_json::Value::as_str),
                    ) {
                        let n = l.get(4).and_then(serde_json::Value::as_u64).unwrap_or(1);
                        self.diag.lines.push_back(Line {
                            at,
                            cat: cat.into(),
                            event: event.into(),
                            detail: detail.into(),
                            n: u32::try_from(n).unwrap_or(1),
                            last_at: at,
                        });
                    }
                }
                pos += 1;
            }
            pos = (c + 1) * CHUNK;
        }
    }

    /// Adds one entry (the engine's own): `cat` is where it happened, `event` what, `detail` the particulars.
    pub(super) fn diag(&mut self, cat: &str, event: &str, detail: &str) {
        if !self.diag_enabled() {
            return;
        }
        self.diag_load();
        let at = self.now();
        let detail = clip(&redact(detail, &mut self.diag.aliases), MAX_DETAIL_CHARS);
        let event = clip(event, 60);
        let end = self.diag.base + self.diag.lines.len() as u64;
        if let Some(last) = self.diag.lines.back_mut() {
            if last.cat == cat
                && last.event == event
                && last.detail == detail
                && at - last.last_at <= REPEAT_MS
            {
                last.n = last.n.saturating_add(1);
                last.last_at = at;
                self.diag.dirty_from.get_or_insert(end - 1);
                return;
            }
        }
        self.diag.lines.push_back(Line {
            at,
            cat: cat.into(),
            event,
            detail,
            n: 1,
            last_at: at,
        });
        self.diag.dirty_from.get_or_insert(end);
        while self.diag.lines.len() > MAX_LINES {
            self.diag.lines.pop_front();
            self.diag.base += 1;
            self.diag.dropped += 1;
        }
        if cat == "error" {
            self.diag_save();
        }
    }

    /// An entry from the screens or the platform ("ui", "nav", "net", "perm", "ble", "call", "error", "app").
    pub fn diag_log(&mut self, cat: &str, event: &str, detail: &str) {
        let cat = if CATEGORIES.contains(&cat) {
            cat
        } else {
            "app"
        };
        self.diag(cat, event, detail);
    }

    fn diag_save(&mut self) {
        let Some(from) = self.diag.dirty_from.take() else {
            return;
        };
        self.diag.saved_at = self.now();
        let base = self.diag.base;
        let end = base + self.diag.lines.len() as u64;
        let from = from.max(base);
        let mut ops: Vec<StorageOp> = Vec::new();
        if end > from {
            for c in (from / CHUNK)..=((end - 1) / CHUNK) {
                let (lo, hi) = (c * CHUNK, (c + 1) * CHUNK);
                let rows: Vec<serde_json::Value> = self
                    .diag
                    .lines
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| {
                        let pos = base + *i as u64;
                        pos >= lo && pos < hi
                    })
                    .map(|(_, l)| serde_json::json!([l.at, l.cat, l.event, l.detail, l.n]))
                    .collect();
                if let Ok(value) = serde_json::to_vec(&rows) {
                    ops.push(StorageOp::SetKv {
                        key: format!("{KV_CHUNK}{c}"),
                        value,
                    });
                }
            }
        }
        // Chunks that fell wholly off the front are emptied.
        while self.diag.emptied_to < base / CHUNK {
            ops.push(StorageOp::SetKv {
                key: format!("{KV_CHUNK}{}", self.diag.emptied_to),
                value: Vec::new(),
            });
            self.diag.emptied_to += 1;
        }
        let meta = serde_json::json!({ "base": base, "end": end, "dropped": self.diag.dropped, "ids": self.diag.aliases });
        if let Ok(value) = serde_json::to_vec(&meta) {
            ops.push(StorageOp::SetKv {
                key: KV_META.into(),
                value,
            });
        }
        let _ = self.ports.storage.apply(&ops);
    }

    /// Every tick: what the event hub noted goes into the log, and the log is saved now and then.
    pub(super) fn tick_diag(&mut self) {
        let noted = self.hub.take_tap();
        for n in noted {
            self.diag("engine", &n.0, &n.1);
        }
        if self.diag.dirty_from.is_some() && self.now() - self.diag.saved_at >= SAVE_EVERY_MS {
            self.diag_save();
        }
    }

    /// The log as a Markdown file. `about` is what the app says about itself (version, platform, screen), one line each.
    pub fn diag_export(&mut self, about: &str) -> String {
        self.diag_load();
        self.diag_save();
        let now = self.now();
        let contacts = self.contacts.len();
        let linked = self.peers.values().filter(|p| p.ready()).count();
        let queued = self.outbox.len();
        let requests = self.invites.requests.len();
        let channels = self.channels.len();
        let relay = if self.ports.transport.signal_available() {
            "configured"
        } else {
            "none configured"
        };
        let store = if self.store_forward() { "on" } else { "off" };
        let mut out = String::new();
        out.push_str("# PrivCord diagnostic log\n\n");
        out.push_str("Kept on this device only. It holds no message text, names, keys or addresses: friends and\n");
        out.push_str("mailboxes appear as short aliases (`id-1`, `id-2`). Send it only if you choose to.\n\n");
        out.push_str(&format!(
            "- Saved: {} (UTC)\n- Entries: {} (older ones dropped: {})\n",
            clock(now),
            self.diag.lines.len(),
            self.diag.dropped
        ));
        out.push_str("\n## This app\n\n");
        for line in about.lines().take(30) {
            let l = clip(&redact(line, &mut self.diag.aliases), 160);
            if !l.trim().is_empty() {
                out.push_str(&format!("- {l}\n"));
            }
        }
        out.push_str("\n## Right now (counts only)\n\n");
        out.push_str(&format!(
            "- Friends: {contacts}, live links: {linked}\n- Messages waiting to send: {queued}\n- Requests waiting for a tap: {requests}\n- Public channels: {channels}\n- Connection service: {relay}\n- Store-and-forward: {store}\n"
        ));
        let errors = self.diag.lines.iter().filter(|l| l.cat == "error").count();
        out.push_str(&format!("- Errors in this log: {errors}\n"));
        out.push_str("\n## What happened, oldest first\n\n```\n");
        for l in &self.diag.lines {
            let d = if l.detail.is_empty() {
                String::new()
            } else {
                format!(" — {}", l.detail)
            };
            let times = if l.n > 1 {
                format!(" ×{}", l.n)
            } else {
                String::new()
            };
            out.push_str(&format!(
                "{} [{}] {}{}{}\n",
                time_of(l.at),
                l.cat,
                l.event,
                d,
                times
            ));
        }
        out.push_str("```\n");
        out
    }
}

/// `2026-11-27 07:11:03` (UTC) from Unix milliseconds.
fn clock(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// `07:11:03.250` from Unix milliseconds.
fn time_of(ms: i64) -> String {
    let rem = ms.div_euclid(1000).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms.rem_euclid(1000)
    )
}

/// Year, month, day from days since 1970-01-01 (the civil-from-days algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_and_addresses_never_get_through() {
        let mut ids = BTreeMap::new();
        let uid = "a".repeat(64);
        let out = redact(
            &format!(
                "peer={uid} other={} at 192.168.1.20:3478 and 2001:db8::1 key={}",
                "b".repeat(64),
                "Zz".repeat(30)
            ),
            &mut ids,
        );
        assert!(!out.contains(&uid), "{out}");
        assert!(!out.contains("192.168"), "{out}");
        assert!(!out.contains("2001:db8"), "{out}");
        assert!(!out.contains("ZzZz"), "{out}");
        assert!(out.contains("id-1") && out.contains("id-2"), "{out}");
        // The same identifier gets the same alias again.
        assert_eq!(redact(&uid, &mut ids), "id-1");
        // Ordinary words and numbers are left alone.
        assert_eq!(
            redact("link up path=relayed in 12 ms", &mut ids),
            "link up path=relayed in 12 ms"
        );
    }

    #[test]
    fn dates_are_right() {
        assert_eq!(clock(0), "1970-01-01 00:00:00");
        assert_eq!(clock(1_700_000_000_000), "2023-11-14 22:13:20");
        assert_eq!(time_of(1_700_000_000_250), "22:13:20.250");
    }
}
