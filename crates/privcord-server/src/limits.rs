//! What the server is allowed to use: a monthly traffic cap, a speed limit per connection, and a disk budget. The numbers
//! people ask about ("how much did we send this month?") are counted here and kept in `data_dir/usage.json`.
//!
//! * **Traffic** is counted where bytes leave and arrive: the relay/Nostr WebSocket (`web_*`) and the TURN relay (`turn_*`).
//!   At the cap, new sockets are refused with 503 and a `Retry-After`, no new relay is handed out, and relayed packets are
//!   dropped, until the month changes.
//! * **Speed** is a per-connection limit (kilobits per second) on the WebSocket, in both directions.
//! * **Disk** is a budget for everything the server stores: it is shared out as the caps the stores already have
//!   (`nostr.max_store_bytes`, `relay.max_queued_bytes`), so nothing here duplicates what they enforce.

use crate::config::{Config, LimitsConfig};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;

/// How long a refused client is told to wait. Long, so a server at its cap is not hammered by reconnects.
pub const RETRY_AFTER_SECS: u64 = 600;

#[derive(Debug)]
pub struct Usage {
    month: std::sync::Mutex<String>,
    web_in: AtomicU64,
    web_out: AtomicU64,
    turn_in: AtomicU64,
    turn_out: AtomicU64,
    cap_bytes: u64,
    path: PathBuf,
}

fn this_month() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    crate::health::civil(secs).0.chars().take(7).collect()
}

impl Usage {
    /// The counters of this month as saved (a new month starts at zero). A missing or damaged file starts at zero too.
    pub fn load(dir: &Path, limits: &LimitsConfig) -> Arc<Self> {
        let path = dir.join("usage.json");
        let now = this_month();
        let saved: Option<serde_json::Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        let same = saved
            .as_ref()
            .and_then(|v| v.get("month"))
            .and_then(|m| m.as_str())
            == Some(now.as_str());
        let n = |k: &str| {
            saved
                .as_ref()
                .filter(|_| same)
                .and_then(|v| v.get(k))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        };
        Arc::new(Self {
            web_in: AtomicU64::new(n("web_in")),
            web_out: AtomicU64::new(n("web_out")),
            turn_in: AtomicU64::new(n("turn_in")),
            turn_out: AtomicU64::new(n("turn_out")),
            month: std::sync::Mutex::new(now),
            cap_bytes: limits.monthly_traffic_gb.saturating_mul(1_000_000_000),
            path,
        })
    }

    /// For tests and servers with no data folder yet: counts, saves nowhere useful, never caps.
    pub fn unlimited() -> Arc<Self> {
        Self::load(
            Path::new("/nonexistent-privcord-usage"),
            &LimitsConfig::default(),
        )
    }

    pub fn web(&self, bytes_in: usize, bytes_out: usize) {
        self.web_in.fetch_add(bytes_in as u64, Relaxed);
        self.web_out.fetch_add(bytes_out as u64, Relaxed);
    }
    pub fn turn(&self, bytes_in: usize, bytes_out: usize) {
        self.turn_in.fetch_add(bytes_in as u64, Relaxed);
        self.turn_out.fetch_add(bytes_out as u64, Relaxed);
    }

    pub fn total(&self) -> u64 {
        self.web_in.load(Relaxed)
            + self.web_out.load(Relaxed)
            + self.turn_in.load(Relaxed)
            + self.turn_out.load(Relaxed)
    }

    /// 0: no cap.
    pub fn cap(&self) -> u64 {
        self.cap_bytes
    }

    pub fn over_cap(&self) -> bool {
        self.cap_bytes > 0 && self.total() >= self.cap_bytes
    }

    /// Starts a new month at zero when the calendar has turned, and saves. Called about once a minute and at stop.
    // ponytail: up to a minute of counts is lost if the process is killed; a write per packet is not worth it.
    pub fn tick(&self) {
        let now = this_month();
        {
            let mut m = self
                .month
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *m != now {
                *m = now;
                for c in [&self.web_in, &self.web_out, &self.turn_in, &self.turn_out] {
                    c.store(0, Relaxed);
                }
            }
        }
        let _ =
            crate::fsutil::write_private_atomic(&self.path, self.snapshot().to_string().as_bytes());
    }

    pub fn snapshot(&self) -> serde_json::Value {
        let month = self
            .month
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        serde_json::json!({
            "month": month,
            "web_in": self.web_in.load(Relaxed),
            "web_out": self.web_out.load(Relaxed),
            "turn_in": self.turn_in.load(Relaxed),
            "turn_out": self.turn_out.load(Relaxed),
            "total": self.total(),
            "cap": self.cap_bytes,
            "over_cap": self.over_cap(),
        })
    }
}

/// Holds a connection to a speed: after moving `n` bytes, wait until they would have taken that long at the limit.
#[derive(Debug)]
pub struct Pace {
    bytes_per_sec: f64,
    next: tokio::time::Instant,
}

impl Pace {
    /// `None` when there is no limit.
    pub fn new(kbps: u64) -> Option<Self> {
        (kbps > 0).then(|| Self {
            bytes_per_sec: kbps as f64 * 125.0,
            next: tokio::time::Instant::now(),
        })
    }

    pub async fn wait(&mut self, bytes: usize) {
        let now = tokio::time::Instant::now();
        let start = self.next.max(now);
        self.next = start + std::time::Duration::from_secs_f64(bytes as f64 / self.bytes_per_sec);
        // A second's worth may go at once (a burst); beyond that the connection waits for its turn.
        let due = self.next.checked_sub(std::time::Duration::from_secs(1));
        if let Some(due) = due.filter(|d| *d > now) {
            tokio::time::sleep_until(due).await;
        }
    }
}

/// Shares the disk budget out as the caps the stores already enforce (a cap that is already smaller stays).
pub fn apply_disk_budget(cfg: &mut Config) {
    let budget = cfg.limits.disk_quota_mb.saturating_mul(1024 * 1024);
    if budget == 0 {
        return;
    }
    let budget = usize::try_from(budget).unwrap_or(usize::MAX);
    cfg.nostr.max_store_bytes = cfg.nostr.max_store_bytes.min(budget / 10 * 7);
    cfg.relay.max_queued_bytes = cfg.relay.max_queued_bytes.min(budget / 4);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn counts_survive_a_restart_and_the_cap_closes_the_door() {
        let d = tempfile::tempdir().unwrap();
        let lim = LimitsConfig {
            monthly_traffic_gb: 1,
            ..LimitsConfig::default()
        };
        let u = Usage::load(d.path(), &lim);
        u.web(10, 20);
        u.turn(5, 5);
        assert_eq!(u.total(), 40);
        assert!(!u.over_cap());
        u.tick();
        let again = Usage::load(d.path(), &lim);
        assert_eq!(again.total(), 40);
        again.web(1_000_000_000, 0);
        assert!(again.over_cap());
        // a server with no cap is never "over"
        let free = Usage::load(d.path(), &LimitsConfig::default());
        free.web(usize::MAX / 4, 0);
        assert!(!free.over_cap());
    }

    #[test]
    fn a_saved_count_from_another_month_is_not_carried_over() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("usage.json"),
            r#"{"month":"1999-01","web_in":999,"web_out":1,"turn_in":0,"turn_out":0}"#,
        )
        .unwrap();
        assert_eq!(Usage::load(d.path(), &LimitsConfig::default()).total(), 0);
    }

    #[test]
    fn the_disk_budget_becomes_the_stores_caps_and_never_raises_one() {
        let mut c = Config::default();
        c.limits.disk_quota_mb = 100;
        c.relay.max_queued_bytes = 1024;
        apply_disk_budget(&mut c);
        assert_eq!(c.nostr.max_store_bytes, 100 * 1024 * 1024 / 10 * 7);
        assert_eq!(c.relay.max_queued_bytes, 1024);
    }

    #[tokio::test]
    async fn a_paced_connection_cannot_exceed_its_speed_beyond_one_burst() {
        let mut p = Pace::new(400).unwrap(); // 400 kbit/s = 50 kB/s
        let t = tokio::time::Instant::now();
        for _ in 0..10 {
            p.wait(10_000).await; // 100 kB: 2 s of traffic, of which 1 s may go at once
        }
        assert!(
            t.elapsed() >= std::time::Duration::from_millis(900),
            "{:?}",
            t.elapsed()
        );
        assert!(
            t.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            t.elapsed()
        );
        assert!(Pace::new(0).is_none());
    }
}
