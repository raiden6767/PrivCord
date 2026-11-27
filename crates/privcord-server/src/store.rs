//! What the relay keeps for people who are offline: an append-only log (`queue.jsonl`), rewritten without what expired now
//! and then. The format is the Node relay's, so either relay can pick up the other's data. A sender's address is never
//! written here.

use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub struct Store {
    path: PathBuf,
    file: Option<File>,
}

/// `(mailbox, id, blob, stored at, expires at)`, in milliseconds.
pub type Held = (String, String, String, i64, i64);

impl Store {
    pub fn open(dir: &std::path::Path) -> Result<Self, String> {
        crate::fsutil::create_private_dir(dir)?;
        let path = dir.join("queue.jsonl");
        // Half a line left by a crash is cut off, or the next message would be glued to it and lost.
        crate::fsutil::trim_torn_tail(&path)
            .map_err(|e| format!("could not check {}: {e}", path.display()))?;
        let file = crate::fsutil::open_private_append(&path)
            .map_err(|e| format!("could not open {}: {e}", path.display()))?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }

    /// Everything in the log that has not expired and was not deleted.
    pub fn load(&mut self, now: i64) -> Vec<Held> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let mut held: Vec<Held> = Vec::new();
        for line in text.lines().filter(|l| !l.is_empty()) {
            let Ok(r) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let (Some(mailbox), Some(id)) = (
                r.get("mailbox").and_then(Value::as_str),
                r.get("id").and_then(Value::as_str),
            ) else {
                continue;
            };
            match r.get("op").and_then(Value::as_str) {
                Some("put") => {
                    let (Some(blob), Some(ts), Some(exp)) = (
                        r.get("blob").and_then(Value::as_str),
                        r.get("ts").and_then(Value::as_i64),
                        r.get("exp").and_then(Value::as_i64),
                    ) else {
                        continue;
                    };
                    if exp > now {
                        held.retain(|h| !(h.0 == mailbox && h.1 == id));
                        held.push((mailbox.to_owned(), id.to_owned(), blob.to_owned(), ts, exp));
                    }
                }
                Some("del") => held.retain(|h| !(h.0 == mailbox && h.1 == id)),
                _ => {}
            }
        }
        held
    }

    /// False when it could not be written (the put is then refused: better than a promise that cannot be kept).
    pub fn append_put(&mut self, mailbox: &str, id: &str, blob: &str, ts: i64, exp: i64) -> bool {
        let mut line = json!({ "op": "put", "mailbox": mailbox, "id": id, "blob": blob, "ts": ts, "exp": exp }).to_string();
        line.push('\n');
        let Some(f) = self.file.as_mut() else {
            return false;
        };
        // One write, so a crash leaves either the whole line or a torn tail that the next start cuts off. A failed write is
        // cut off again here (a part of a line would swallow the next message).
        let before = f.metadata().map(|m| m.len()).ok();
        if f.write_all(line.as_bytes()).is_ok() {
            return true;
        }
        if let Some(len) = before {
            let _ = f.set_len(len);
        }
        false
    }

    /// Asks the disk to keep what has been appended so far (called every second or so: a power cut loses at most that).
    pub fn sync(&mut self) {
        if let Some(f) = self.file.as_mut() {
            let _ = f.sync_data();
        }
    }

    /// Rewrites the log to hold exactly `live`.
    pub fn rewrite(&mut self, live: &[Held]) {
        let mut body = String::new();
        for (mailbox, id, blob, ts, exp) in live {
            body.push_str(&json!({ "op": "put", "mailbox": mailbox, "id": id, "blob": blob, "ts": ts, "exp": exp }).to_string());
            body.push('\n');
        }
        // Synced to disk, renamed over the log, and the folder synced: never a half-written queue.
        if crate::fsutil::write_private_atomic(&self.path, body.as_bytes()).is_ok() {
            self.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .ok();
        } else {
            eprintln!("privcord-server: could not compact {}", self.path.display());
        }
    }
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Store")
    }
}
