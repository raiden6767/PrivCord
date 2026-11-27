//! Subscription filters (NIP-01).

use super::event::{Event, Id};
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Lowercase hex, whole or a prefix.
    pub ids: Vec<String>,
    pub authors: Vec<String>,
    pub kinds: Vec<u64>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub limit: Option<usize>,
    /// `#x` filters: the letter and the accepted values.
    pub tags: Vec<(char, Vec<String>)>,
}

fn strings(v: &Value) -> Result<Vec<String>, &'static str> {
    v.as_array()
        .ok_or("invalid: filter list is not an array")?
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_owned)
                .ok_or("invalid: filter list holds a non-string")
        })
        .collect()
}

fn is_hex_prefix(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Filter {
    pub fn parse(v: &Value) -> Result<Self, &'static str> {
        let o = v.as_object().ok_or("invalid: filter is not an object")?;
        let mut f = Self::default();
        for (k, val) in o {
            match k.as_str() {
                "ids" => f.ids = strings(val)?,
                "authors" => f.authors = strings(val)?,
                "kinds" => {
                    f.kinds = val
                        .as_array()
                        .ok_or("invalid: kinds is not an array")?
                        .iter()
                        .map(|x| x.as_u64().ok_or("invalid: a kind is not a number"))
                        .collect::<Result<_, _>>()?;
                }
                "since" => f.since = Some(val.as_i64().ok_or("invalid: bad since")?),
                "until" => f.until = Some(val.as_i64().ok_or("invalid: bad until")?),
                "limit" => {
                    f.limit = Some(
                        usize::try_from(val.as_u64().ok_or("invalid: bad limit")?)
                            .unwrap_or(usize::MAX),
                    )
                }
                other => {
                    let mut chars = other.chars();
                    if let (Some('#'), Some(c), None) = (chars.next(), chars.next(), chars.next()) {
                        if c.is_ascii_alphabetic() {
                            f.tags.push((c, strings(val)?));
                        }
                    }
                    // Anything else (search, unknown extensions) is ignored, as most relays do.
                }
            }
        }
        if f.ids.iter().chain(&f.authors).any(|s| !is_hex_prefix(s)) {
            return Err("invalid: ids and authors must be lowercase hex");
        }
        Ok(f)
    }

    /// Whether `e` satisfies everything but `limit`.
    pub fn matches(&self, e: &Event) -> bool {
        if self.since.is_some_and(|s| e.created_at < s)
            || self.until.is_some_and(|u| e.created_at > u)
        {
            return false;
        }
        if !self.kinds.is_empty() && !self.kinds.contains(&e.kind) {
            return false;
        }
        if !self.ids.is_empty() {
            let id = super::event::hex(&e.id);
            if !self.ids.iter().any(|p| id.starts_with(p.as_str())) {
                return false;
            }
        }
        if !self.authors.is_empty() {
            let pk = super::event::hex(&e.pubkey);
            if !self.authors.iter().any(|p| pk.starts_with(p.as_str())) {
                return false;
            }
        }
        self.tags.iter().all(|(name, values)| {
            e.tags.iter().any(|t| {
                t.first()
                    .is_some_and(|n| n.len() == 1 && n.starts_with(*name))
                    && t.get(1).is_some_and(|v| values.contains(v))
            })
        })
    }

    /// An id when the filter names exactly one whole event.
    pub fn exact_ids(&self) -> Option<Vec<Id>> {
        if self.ids.is_empty() || self.ids.iter().any(|s| s.len() != 64) {
            return None;
        }
        self.ids.iter().map(|s| super::event::unhex(s)).collect()
    }
}

pub fn any_match(filters: &[Filter], e: &Event) -> bool {
    filters.iter().any(|f| f.matches(e))
}
