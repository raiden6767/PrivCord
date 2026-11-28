//! A Nostr event (NIP-01): parsing, the id, and the Schnorr signature.

use k256::schnorr::{Signature, VerifyingKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub type Id = [u8; 32];

#[derive(Debug, Clone)]
pub struct Event {
    pub id: Id,
    pub pubkey: [u8; 32],
    pub created_at: i64,
    pub kind: u64,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: [u8; 64],
    /// The event as sent to subscribers, built once.
    pub json: Arc<str>,
}

pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(b & 15), 16).unwrap_or('0'));
    }
    out
}

/// Lowercase hex of exactly `N` bytes. (Not `from_str_radix`, which would also accept a leading `+`.)
pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut out = [0u8; N];
    for (slot, pair) in out.iter_mut().zip(s.as_bytes().chunks(2)) {
        let digit = |c: u8| -> u8 {
            if c.is_ascii_digit() {
                c - b'0'
            } else {
                c - b'a' + 10
            }
        };
        *slot = digit(*pair.first()?) * 16 + digit(*pair.get(1)?);
    }
    Some(out)
}

/// `[0, pubkey, created_at, kind, tags, content]` as compact JSON, hashed.
fn compute_id(pubkey: &str, created_at: i64, kind: u64, tags: &[Vec<String>], content: &str) -> Id {
    let text = json!([0, pubkey, created_at, kind, tags, content]).to_string();
    Sha256::digest(text.as_bytes()).into()
}

impl Event {
    /// Reads an event from JSON and checks everything about it: shape, id and signature. The error is for the client.
    pub fn parse(v: &Value) -> Result<Self, &'static str> {
        let o = v.as_object().ok_or("invalid: event is not an object")?;
        let text = |k: &str| o.get(k).and_then(Value::as_str);
        let id_hex = text("id").ok_or("invalid: no id")?;
        let pk_hex = text("pubkey").ok_or("invalid: no pubkey")?;
        let sig_hex = text("sig").ok_or("invalid: no sig")?;
        let content = text("content").ok_or("invalid: no content")?.to_owned();
        let created_at = o
            .get("created_at")
            .and_then(Value::as_i64)
            .ok_or("invalid: bad created_at")?;
        let kind = o
            .get("kind")
            .and_then(Value::as_u64)
            .ok_or("invalid: bad kind")?;
        if kind > 65535 {
            return Err("invalid: kind out of range");
        }
        let mut tags = Vec::new();
        for t in o
            .get("tags")
            .and_then(Value::as_array)
            .ok_or("invalid: bad tags")?
        {
            let t = t.as_array().ok_or("invalid: a tag is not an array")?;
            let strs: Option<Vec<String>> =
                t.iter().map(|x| x.as_str().map(str::to_owned)).collect();
            tags.push(strs.ok_or("invalid: a tag holds a non-string")?);
        }
        let (Some(id), Some(pubkey), Some(sig)) = (
            unhex::<32>(id_hex),
            unhex::<32>(pk_hex),
            unhex::<64>(sig_hex),
        ) else {
            return Err("invalid: id, pubkey or sig is not lowercase hex of the right length");
        };
        if id_hex.bytes().any(|b| b.is_ascii_uppercase())
            || pk_hex.bytes().any(|b| b.is_ascii_uppercase())
        {
            return Err("invalid: hex must be lowercase");
        }
        if compute_id(pk_hex, created_at, kind, &tags, &content) != id {
            return Err("invalid: id does not match the content");
        }
        let key = VerifyingKey::from_bytes(&k256::FieldBytes::from(pubkey))
            .map_err(|_| "invalid: bad pubkey")?;
        let signature =
            Signature::try_from(sig.as_slice()).map_err(|_| "invalid: bad signature")?;
        key.verify_raw(&id, &signature)
            .map_err(|_| "invalid: bad signature")?;
        let mut e = Self {
            id,
            pubkey,
            created_at,
            kind,
            tags,
            content,
            sig,
            json: Arc::from(""),
        };
        e.json = Arc::from(e.to_value().to_string());
        Ok(e)
    }

    /// Reads an event this server wrote earlier: checked for shape and id only (it was verified when it arrived).
    pub fn parse_trusted(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        let tags: Vec<Vec<String>> = o
            .get("tags")?
            .as_array()?
            .iter()
            .map(|t| {
                t.as_array()?
                    .iter()
                    .map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .collect::<Option<_>>()?;
        let mut e = Self {
            id: unhex(o.get("id")?.as_str()?)?,
            pubkey: unhex(o.get("pubkey")?.as_str()?)?,
            created_at: o.get("created_at")?.as_i64()?,
            kind: o.get("kind")?.as_u64()?,
            tags,
            content: o.get("content")?.as_str()?.to_owned(),
            sig: unhex(o.get("sig")?.as_str()?)?,
            json: Arc::from(""),
        };
        e.json = Arc::from(e.to_value().to_string());
        Some(e)
    }

    pub fn to_value(&self) -> Value {
        json!({
            "id": hex(&self.id), "pubkey": hex(&self.pubkey), "created_at": self.created_at, "kind": self.kind,
            "tags": self.tags, "content": self.content, "sig": hex(&self.sig),
        })
    }

    /// The first value of the first tag called `name`.
    pub fn tag(&self, name: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|t| t.first().map(String::as_str) == Some(name))
            .and_then(|t| t.get(1))
            .map(String::as_str)
    }

    /// NIP-40: the second after which the event is gone, if it says so.
    pub fn expiration(&self) -> Option<i64> {
        self.tag("expiration").and_then(|s| s.parse().ok())
    }

    /// NIP-13: leading zero bits of the id.
    pub fn pow_bits(&self) -> u32 {
        let mut bits = 0;
        for b in self.id {
            if b == 0 {
                bits += 8;
            } else {
                bits += b.leading_zeros();
                break;
            }
        }
        bits
    }

    pub fn is_ephemeral(&self) -> bool {
        (20_000..30_000).contains(&self.kind)
    }
    pub fn is_replaceable(&self) -> bool {
        self.kind == 0 || self.kind == 3 || self.kind == 41 || (10_000..20_000).contains(&self.kind)
    }
    pub fn is_addressable(&self) -> bool {
        (30_000..40_000).contains(&self.kind)
    }

    /// For events that replace each other: who, what kind, and (addressable) which `d` tag.
    pub fn replace_key(&self) -> Option<(Id, u64, String)> {
        if self.is_replaceable() {
            Some((self.pubkey, self.kind, String::new()))
        } else if self.is_addressable() {
            Some((
                self.pubkey,
                self.kind,
                self.tag("d").unwrap_or("").to_owned(),
            ))
        } else {
            None
        }
    }

    pub fn size(&self) -> usize {
        self.json.len() + 160
    }
}
