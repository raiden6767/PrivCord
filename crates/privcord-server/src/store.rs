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
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        let path = dir.join("queue.jsonl");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
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
        let line = json!({ "op": "put", "mailbox": mailbox, "id": id, "blob": blob, "ts": ts, "exp": exp }).to_string();
        let Some(f) = self.file.as_mut() else {
            return false;
        };
        f.write_all(line.as_bytes())
            .and_then(|()| f.write_all(b"\n"))
            .is_ok()
    }

    /// Rewrites the log to hold exactly `live`.
    pub fn rewrite(&mut self, live: &[Held]) {
        let tmp = self.path.with_extension("jsonl.tmp");
        let mut body = String::new();
        for (mailbox, id, blob, ts, exp) in live {
            body.push_str(&json!({ "op": "put", "mailbox": mailbox, "id": id, "blob": blob, "ts": ts, "exp": exp }).to_string());
            body.push('\n');
        }
        if std::fs::write(&tmp, body)
            .and_then(|()| std::fs::rename(&tmp, &self.path))
            .is_ok()
        {
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
