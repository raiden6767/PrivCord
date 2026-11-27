//! How a record is written to a log file: one line each, with a check that catches damage in the middle of a file, and,
//! with a key, encryption at rest.
//!
//! Three shapes of line are read:
//! - `{...}`: plain JSON, as every earlier version wrote it (no check). Still accepted, and rewritten in the new shape the next
//!   time the log is compacted.
//! - `c1 <16 hex> <json>`: the JSON and the first 8 bytes of its SHA-256 (with the log's name mixed in). A flipped byte
//!   anywhere in the line is noticed.
//! - `e1 <base64>`: AES-256-GCM of the JSON with a fresh random nonce; the log's name is the associated data, so a line cannot
//!   be moved from one log to another. The GCM tag is the check: a flipped byte, a wrong key or a tampered line all fail.
//!
//! The key is 32 random bytes (64 hex characters), kept outside `data_dir` (a file on another volume, a systemd credential, or
//! an environment variable), so a stolen disk or a backup of the data folder alone reads as noise.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::config::StorageConfig;

pub struct Codec {
    key: Option<Aes256Gcm>,
    checksums: bool,
}

impl std::fmt::Debug for Codec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Codec")
            .field("encrypted", &self.key.is_some())
            .field("checksums", &self.checksums)
            .finish()
    }
}

/// What reading one line gave.
#[derive(Debug, PartialEq, Eq)]
pub enum Decoded {
    /// The record's JSON.
    Payload(String),
    /// The line is damaged (or was written with another key): it cannot be trusted.
    Corrupt,
    /// The line is encrypted and no key was given.
    NeedsKey,
}

fn check(label: &str, payload: &str) -> String {
    let mut h = Sha256::new();
    h.update(label.as_bytes());
    h.update([0]);
    h.update(payload.as_bytes());
    crate::nostr::event::hex(h.finalize().get(..8).unwrap_or(&[]))
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD_NO_PAD
}

/// Reads a key written as 64 hex characters (whitespace around it is ignored).
pub fn parse_key(text: &str) -> Result<[u8; 32], String> {
    let bad = || "the storage key must be 64 hex characters (32 random bytes)".to_owned();
    let t = text.trim();
    if t.len() != 64 || !t.is_ascii() {
        return Err(bad());
    }
    // Read pair by pair from the text, so the key is only ever made of what was given (no buffer made up in advance).
    let bytes: Vec<u8> = (0..64)
        .step_by(2)
        .map(|i| t.get(i..i + 2).and_then(|p| u8::from_str_radix(p, 16).ok()))
        .collect::<Option<_>>()
        .ok_or_else(bad)?;
    <[u8; 32]>::try_from(bytes).map_err(|_| bad())
}

impl Codec {
    /// Plain lines, no check: the format of the earlier versions (what the tests of those use).
    pub fn plain() -> Self {
        Self {
            key: None,
            checksums: false,
        }
    }

    pub fn with_key(key: &[u8; 32], checksums: bool) -> Result<Self, String> {
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "bad storage key".to_owned())?;
        Ok(Self {
            key: Some(cipher),
            checksums,
        })
    }

    /// From the settings: the key is read from `key_file` or from the environment variable `key_env`.
    pub fn from_config(c: &StorageConfig) -> Result<Self, String> {
        if !c.encrypt {
            return Ok(Self {
                key: None,
                checksums: c.checksums,
            });
        }
        let text = if c.key_file.trim().is_empty() {
            std::env::var(&c.key_env).map_err(|_| {
                format!(
                    "storage.encrypt is on but there is no key: set storage.key_file, or the environment variable {}",
                    c.key_env
                )
            })?
        } else {
            std::fs::read_to_string(&c.key_file)
                .map_err(|e| format!("could not read storage.key_file {}: {e}", c.key_file))?
        };
        let key = parse_key(&text)?;
        Self::with_key(&key, c.checksums)
    }

    pub fn encrypts(&self) -> bool {
        self.key.is_some()
    }

    /// Whether this codec writes a shape other than plain JSON lines (so an older plain log is worth rewriting).
    pub fn frames(&self) -> bool {
        self.key.is_some() || self.checksums
    }

    /// Whether `line` is already in the shape this codec writes (a log with older lines is rewritten at its next compaction).
    pub fn is_current(&self, line: &str) -> bool {
        if self.key.is_some() {
            line.starts_with("e1 ")
        } else if self.checksums {
            line.starts_with("c1 ")
        } else {
            line.starts_with('{')
        }
    }

    /// One line (no trailing newline) holding `payload`, which belongs to the log called `label`.
    pub fn encode(&self, label: &str, payload: &str) -> String {
        if let Some(cipher) = &self.key {
            let mut nonce = [0u8; 12];
            let _ = getrandom::fill(&mut nonce);
            if let Ok(n) = Nonce::try_from(&nonce[..]) {
                if let Ok(ct) = cipher.encrypt(
                    &n,
                    Payload {
                        msg: payload.as_bytes(),
                        aad: label.as_bytes(),
                    },
                ) {
                    let mut raw = nonce.to_vec();
                    raw.extend_from_slice(&ct);
                    return format!("e1 {}", b64().encode(raw));
                }
            }
            // Encryption failing is not a reason to write the record in the clear: an empty line is skipped when read.
            return String::new();
        }
        if self.checksums {
            return format!("c1 {} {payload}", check(label, payload));
        }
        payload.to_owned()
    }

    pub fn decode(&self, label: &str, line: &str) -> Decoded {
        if let Some(rest) = line.strip_prefix("e1 ") {
            let Some(cipher) = &self.key else {
                return Decoded::NeedsKey;
            };
            let Ok(raw) = b64().decode(rest.trim()) else {
                return Decoded::Corrupt;
            };
            let (Some(nonce), Some(ct)) = (raw.get(..12), raw.get(12..)) else {
                return Decoded::Corrupt;
            };
            let Ok(n) = Nonce::try_from(nonce) else {
                return Decoded::Corrupt;
            };
            return cipher
                .decrypt(
                    &n,
                    Payload {
                        msg: ct,
                        aad: label.as_bytes(),
                    },
                )
                .ok()
                .and_then(|p| String::from_utf8(p).ok())
                .map_or(Decoded::Corrupt, Decoded::Payload);
        }
        if let Some(rest) = line.strip_prefix("c1 ") {
            return match rest.split_once(' ') {
                Some((sum, payload)) if sum == check(label, payload) => {
                    Decoded::Payload(payload.to_owned())
                }
                _ => Decoded::Corrupt,
            };
        }
        if line.starts_with('{') {
            return Decoded::Payload(line.to_owned());
        }
        Decoded::Corrupt
    }
}

/// Looks at the start of a log before anything is read from it. A log whose encrypted lines all fail to open is almost
/// certainly read with the wrong key: reading on would skip every record, and the next rewrite of the log would then destroy
/// them. So that is an error at start, not a log of damage.
pub fn check_key(codec: &Codec, label: &str, text: &str, name: &str) -> Result<(), String> {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("e1 "))
        .take(5)
        .collect();
    if !lines.is_empty()
        && codec.key.is_some()
        && lines
            .iter()
            .all(|l| codec.decode(label, l) == Decoded::Corrupt)
    {
        return Err(format!(
            "{name}: none of its encrypted records can be read with this storage key: is it the key they were written with? (nothing was changed)"
        ));
    }
    Ok(())
}

/// What reading a whole log found.
#[derive(Debug, Default, Clone)]
pub struct ReadReport {
    /// Lines that could not be trusted and were skipped.
    pub corrupt: usize,
    /// Lines in an older shape than the codec writes: the log should be rewritten.
    pub old_shape: usize,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A fresh random key: the tests use no key that is written down in the source.
    fn key() -> [u8; 32] {
        let mut k = [0u8; 32];
        getrandom::fill(&mut k).unwrap();
        k
    }

    #[test]
    fn plain_lines_round_trip_and_old_lines_are_accepted_by_every_codec() {
        let p = Codec::plain();
        assert_eq!(
            p.decode("log", &p.encode("log", "{\"a\":1}")),
            Decoded::Payload("{\"a\":1}".into())
        );
        let c = Codec::with_key(&key(), true).unwrap();
        assert_eq!(
            c.decode("log", "{\"old\":true}"),
            Decoded::Payload("{\"old\":true}".into())
        );
        assert!(!c.is_current("{\"old\":true}"));
    }

    #[test]
    fn a_checksummed_line_notices_any_changed_byte_and_a_moved_line() {
        let c = Codec {
            key: None,
            checksums: true,
        };
        let line = c.encode("a", "{\"x\":\"hello\"}");
        assert!(c.is_current(&line));
        assert_eq!(
            c.decode("a", &line),
            Decoded::Payload("{\"x\":\"hello\"}".into())
        );
        for i in 0..line.len() {
            let mut bytes = line.clone().into_bytes();
            bytes[i] ^= 0x01;
            if let Ok(changed) = String::from_utf8(bytes) {
                assert_ne!(
                    c.decode("a", &changed),
                    Decoded::Payload("{\"x\":\"hello\"}".into()),
                    "byte {i}"
                );
            }
        }
        assert_eq!(
            c.decode("b", &line),
            Decoded::Corrupt,
            "another log's line is refused"
        );
    }

    #[test]
    fn an_encrypted_line_hides_the_record_and_fails_for_a_wrong_key_a_changed_byte_or_another_log()
    {
        let c = Codec::with_key(&key(), true).unwrap();
        let line = c.encode("nostr", "{\"secret\":\"words\"}");
        assert!(line.starts_with("e1 ") && !line.contains("secret") && !line.contains("words"));
        assert_ne!(
            line,
            c.encode("nostr", "{\"secret\":\"words\"}"),
            "a fresh nonce every time"
        );
        assert_eq!(
            c.decode("nostr", &line),
            Decoded::Payload("{\"secret\":\"words\"}".into())
        );
        assert_eq!(c.decode("queue", &line), Decoded::Corrupt);
        let other = Codec::with_key(&key(), true).unwrap();
        assert_eq!(other.decode("nostr", &line), Decoded::Corrupt);
        assert_eq!(Codec::plain().decode("nostr", &line), Decoded::NeedsKey);
        let mut bytes = line.into_bytes();
        let last = bytes.len() - 3;
        bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
        assert_eq!(
            c.decode("nostr", &String::from_utf8(bytes).unwrap()),
            Decoded::Corrupt
        );
    }

    #[test]
    fn keys_are_64_hex_characters() {
        assert!(parse_key(&"ab".repeat(32)).is_ok());
        assert!(parse_key(&format!("  {}\n", "AB".repeat(32))).is_ok());
        assert!(parse_key("short").is_err());
        assert!(parse_key(&"zz".repeat(32)).is_err());
    }
}
