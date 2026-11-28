//! NIP-98: HTTP authentication with a signed Nostr event. The request carries `Authorization: Nostr <base64 of the event>`; the
//! event (kind 27235) names the URL and method it is for, and, for a request with a body, the body's SHA-256, so it cannot be
//! replayed for another request. It is only good for a minute.

use super::event::{hex, Event, Id};
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

const HTTP_AUTH_KIND: u64 = 27_235;
/// How far the event's time may be from ours, in seconds.
const WINDOW_SECS: i64 = 60;

/// Remembers the Authorization events already used, until they are too old to be accepted anyway.
#[derive(Debug, Default)]
pub struct Seen(std::sync::Mutex<std::collections::HashMap<Id, i64>>);

impl Seen {
    /// False if this event was used before (the request is a replay). Entries older than the window are dropped.
    pub fn first_use(&self, id: Id, now: i64) -> bool {
        let mut m = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        m.retain(|_, at| now - *at <= 2 * WINDOW_SECS);
        m.insert(id, now).is_none()
    }
}

/// The part of a URL that has to match: `host[:port]` (lower case) and the path with its query. The scheme is left out: a proxy
/// that ends TLS makes the server see `http` where the client signed `https`.
pub(crate) fn host_and_path(url: &str) -> Option<(String, String)> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let (host, path) = match rest.find('/') {
        Some(i) => (rest.get(..i)?, rest.get(i..)?),
        None => (rest, "/"),
    };
    (!host.is_empty()).then(|| (host.to_ascii_lowercase(), path.to_owned()))
}

/// Checks the `Authorization` header of a request. `hosts` are the names this server answers to (its `Host` header and, when
/// set, the host of `nostr.relay_url`); `path` is the request's path and query. Returns who signed it and the event's id (the
/// caller remembers ids for a minute or two, so one signed request cannot be played again within its window).
pub fn verify(
    authorization: &str,
    method: &str,
    hosts: &[String],
    path: &str,
    body: &[u8],
    now: i64,
) -> Result<(Id, Id), &'static str> {
    let b64 = authorization
        .strip_prefix("Nostr ")
        .ok_or("the Authorization header must be: Nostr <base64 event>")?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| "the Authorization event is not base64")?;
    let v: Value =
        serde_json::from_slice(&raw).map_err(|_| "the Authorization event is not JSON")?;
    // The id and the signature are checked here.
    let e = Event::parse(&v).map_err(|_| "the Authorization event is not a valid signed event")?;
    if e.kind != HTTP_AUTH_KIND {
        return Err("the Authorization event must be kind 27235");
    }
    if (e.created_at - now).abs() > WINDOW_SECS {
        return Err("the Authorization event is too old or too new (a minute either way)");
    }
    let tag = |name: &str| {
        e.tags
            .iter()
            .find(|t| t.first().is_some_and(|n| n == name))
            .and_then(|t| t.get(1))
    };
    if !tag("method").is_some_and(|m| m.eq_ignore_ascii_case(method)) {
        return Err("the Authorization event is for another HTTP method");
    }
    let (host, p) = tag("u")
        .and_then(|u| host_and_path(u))
        .ok_or("the Authorization event names no URL")?;
    if !hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)) || p != path {
        return Err("the Authorization event is for another URL");
    }
    if !body.is_empty() {
        let want = hex(&Sha256::digest(body));
        if !tag("payload").is_some_and(|t| t.eq_ignore_ascii_case(&want)) {
            return Err("the Authorization event's payload tag does not match the body");
        }
    }
    Ok((e.pubkey, e.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_compared_by_host_and_path_not_scheme() {
        assert_eq!(
            host_and_path("https://Relay.Example.com:8443/a?b=1"),
            Some(("relay.example.com:8443".into(), "/a?b=1".into()))
        );
        assert_eq!(host_and_path("http://x"), Some(("x".into(), "/".into())));
        assert_eq!(host_and_path("https:///nope"), None);
    }

    #[test]
    fn an_authorization_event_can_be_used_once_within_its_window() {
        let seen = Seen::default();
        assert!(seen.first_use([1; 32], 1_000));
        assert!(!seen.first_use([1; 32], 1_030), "played again");
        assert!(seen.first_use([2; 32], 1_030));
        // long after the window the entry is gone (the event itself would be refused as too old)
        assert!(seen.first_use([1; 32], 1_200));
    }

    #[test]
    fn a_header_that_is_not_an_event_is_refused_with_a_reason() {
        let hosts = vec!["x".to_owned()];
        assert!(verify("Bearer abc", "POST", &hosts, "/", b"", 0).is_err());
        assert!(verify("Nostr !!!", "POST", &hosts, "/", b"", 0).is_err());
        let not_event = base64::engine::general_purpose::STANDARD.encode(b"{}");
        assert!(verify(&format!("Nostr {not_event}"), "POST", &hosts, "/", b"", 0).is_err());
    }
}
