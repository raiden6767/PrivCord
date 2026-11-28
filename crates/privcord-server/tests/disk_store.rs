//! The event database on disk must behave exactly like the in-memory store. A long run of random operations (new events,
//! replaceable and addressable ones, deletions, requests to vanish, bans, expiry, sweeps) is applied to both, and after
//! every step their answers must be the same. Then: what is on disk is still there after the store is closed and opened.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use k256::schnorr::SigningKey;
use privcord_server::nostr::{
    event::{hex, Event},
    filter::Filter,
    store::Store,
};
use privcord_server::record::Codec;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn key() -> SigningKey {
    loop {
        let mut b = [0u8; 32];
        getrandom::fill(&mut b).unwrap();
        if let Ok(k) = SigningKey::from_bytes(&k256::FieldBytes::from(b)) {
            return k;
        }
    }
}

fn make(
    k: &SigningKey,
    kind: u64,
    created_at: i64,
    tags: Vec<Vec<String>>,
    content: &str,
) -> Arc<Event> {
    let pubkey = hex(&k.verifying_key().to_bytes());
    let id: [u8; 32] = Sha256::digest(
        json!([0, pubkey, created_at, kind, tags, content])
            .to_string()
            .as_bytes(),
    )
    .into();
    let mut aux = [0u8; 32];
    getrandom::fill(&mut aux).unwrap();
    let sig = k.sign_raw(&id, &aux).unwrap();
    let v = json!({ "id": hex(&id), "pubkey": pubkey, "created_at": created_at, "kind": kind, "tags": tags, "content": content, "sig": hex(&sig.to_bytes()) });
    Arc::new(Event::parse(&v).unwrap())
}

/// A small deterministic stream of choices (the events themselves are signed with random keys).
struct Dice(u64);
impl Dice {
    fn next(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

fn tag(a: &str, b: &str) -> Vec<String> {
    vec![a.to_owned(), b.to_owned()]
}

fn filter(v: Value) -> Filter {
    Filter::parse(&v).unwrap()
}

fn ids(s: &Store, f: &Filter, now: i64) -> Vec<String> {
    s.query(f, now, 500).iter().map(|e| hex(&e.id)).collect()
}

#[test]
fn the_disk_store_answers_exactly_like_the_memory_store() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Store::open(None, 0).unwrap();
    let disk = Store::open_disk(dir.path(), 0, Arc::new(Codec::plain())).unwrap();
    let authors: Vec<SigningKey> = (0..4).map(|_| key()).collect();
    let mut dice = Dice(0x9e37_79b9_7f4a_7c15);
    let base = 1_700_000_000_i64;
    let mut made: Vec<Arc<Event>> = Vec::new();
    let filters: Vec<Filter> = vec![
        filter(json!({})),
        filter(json!({ "kinds": [1] })),
        filter(json!({ "kinds": [0, 3, 10002] })),
        filter(json!({ "kinds": [30023] })),
        filter(json!({ "#t": ["a"] })),
        filter(json!({ "#t": ["a", "b"], "limit": 7 })),
        filter(json!({ "#d": ["x"] })),
        filter(json!({ "since": base + 10, "until": base + 30 })),
        filter(json!({ "kinds": [1], "#t": ["c"], "limit": 3 })),
        filter(json!({ "kinds": [1059] })),
    ];
    for step in 0..600 {
        let author = &authors[dice.next(4) as usize];
        let at = base + dice.next(60) as i64;
        let (e, ban): (Option<Arc<Event>>, Option<usize>) = match dice.next(12) {
            0..=4 => {
                let t = ["a", "b", "c"][dice.next(3) as usize];
                (
                    Some(make(
                        author,
                        1,
                        at,
                        vec![tag("t", t)],
                        &format!("note {step}"),
                    )),
                    None,
                )
            }
            5 => (
                Some(make(
                    author,
                    [0, 3, 10002][dice.next(3) as usize],
                    at,
                    vec![],
                    &format!("meta {step}"),
                )),
                None,
            ),
            6 => {
                let d = ["x", "y"][dice.next(2) as usize];
                (
                    Some(make(
                        author,
                        30023,
                        at,
                        vec![tag("d", d), tag("t", "a")],
                        &format!("long {step}"),
                    )),
                    None,
                )
            }
            7 if !made.is_empty() => {
                // A deletion naming an event (maybe somebody else's) and an address.
                let target = &made[dice.next(made.len() as u64) as usize];
                let tags = vec![
                    tag("e", &hex(&target.id)),
                    tag("a", &format!("30023:{}:x", hex(&target.pubkey))),
                ];
                (Some(make(author, 5, at + 5, tags, "")), None)
            }
            8 if dice.next(8) == 0 => (
                Some(make(
                    author,
                    62,
                    at + 10,
                    vec![tag("relay", "ALL_RELAYS")],
                    "",
                )),
                None,
            ),
            9 => {
                let to = hex(&authors[dice.next(4) as usize].verifying_key().to_bytes());
                (
                    Some(make(author, 1059, at, vec![tag("p", &to)], "wrapped")),
                    None,
                )
            }
            10 => {
                let exp = if dice.next(2) == 0 {
                    base + 5
                } else {
                    base + 100_000
                };
                (
                    Some(make(
                        author,
                        1,
                        at,
                        vec![tag("t", "b"), tag("expiration", &exp.to_string())],
                        "short lived",
                    )),
                    None,
                )
            }
            _ if !made.is_empty() => (None, Some(dice.next(made.len() as u64) as usize)),
            _ => (None, None),
        };
        if let Some(e) = e {
            made.push(e.clone());
            assert_eq!(
                mem.insert(e.clone()),
                disk.insert(e),
                "step {step}: the two stores disagree on an event"
            );
        } else if let Some(i) = ban {
            let id = made[i].id;
            if dice.next(3) == 0 {
                assert_eq!(
                    mem.unban_event(&id),
                    disk.unban_event(&id),
                    "step {step}: unban"
                );
            } else {
                assert_eq!(mem.ban_event(&id), disk.ban_event(&id), "step {step}: ban");
            }
        }
        if step % 40 == 39 {
            let now = base + 20;
            assert_eq!(
                mem.sweep(now, 0, 0),
                disk.sweep(now, 0, 0),
                "step {step}: sweep"
            );
        }
        if step % 25 == 24 {
            let now = base + 20;
            assert_eq!(mem.len(), disk.len(), "step {step}: how many events");
            assert_eq!(mem.bytes(), disk.bytes(), "step {step}: how many bytes");
            for (i, f) in filters.iter().enumerate() {
                assert_eq!(
                    ids(&mem, f, now),
                    ids(&disk, f, now),
                    "step {step}: filter {i}"
                );
            }
            assert_eq!(
                mem.banned_events(),
                disk.banned_events(),
                "step {step}: bans"
            );
        }
    }
    assert!(mem.len() > 20, "the run kept some events ({})", mem.len());
}

#[test]
fn the_size_cap_drops_the_oldest_first_on_both() {
    let dir = tempfile::tempdir().unwrap();
    let k = key();
    let one = make(&k, 1, 1_700_000_000, vec![], "x".repeat(300).as_str());
    let cap = one.size() * 5;
    let mem = Store::open(None, cap).unwrap();
    let disk = Store::open_disk(dir.path(), cap, Arc::new(Codec::plain())).unwrap();
    for i in 0..12 {
        let e = make(&k, 1, 1_700_000_000 + i, vec![], "x".repeat(300).as_str());
        assert_eq!(mem.insert(e.clone()), disk.insert(e));
    }
    assert_eq!(mem.len(), disk.len());
    let f = filter(json!({}));
    assert_eq!(ids(&mem, &f, 0), ids(&disk, &f, 0));
    assert!(disk.len() <= 5);
}

#[test]
fn what_is_on_disk_survives_closing_and_opening_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let k = key();
    let a = make(&k, 1, 1_700_000_000, vec![tag("t", "keep")], "kept");
    let doomed = make(&k, 1, 1_700_000_001, vec![], "deleted");
    let banned = make(&k, 1, 1_700_000_002, vec![], "banned");
    {
        let s = Store::open_disk(dir.path(), 0, Arc::new(Codec::plain())).unwrap();
        for e in [&a, &doomed, &banned] {
            s.insert(e.clone());
        }
        s.insert(make(
            &k,
            5,
            1_700_000_003,
            vec![tag("e", &hex(&doomed.id))],
            "",
        ));
        s.ban_event(&banned.id);
        assert!(s.sync());
        s.close();
    }
    let s = Store::open_disk(dir.path(), 0, Arc::new(Codec::plain())).unwrap();
    assert_eq!(
        s.len(),
        2,
        "the kept event and the deletion are still there"
    );
    assert_eq!(
        ids(&s, &filter(json!({ "#t": ["keep"] })), 0),
        vec![hex(&a.id)]
    );
    // What was deleted or banned stays refused after a restart.
    assert_eq!(
        s.insert(doomed),
        privcord_server::nostr::store::Put::Deleted
    );
    assert_eq!(
        s.insert(banned),
        privcord_server::nostr::store::Put::Deleted
    );
}

#[test]
fn a_closed_store_does_nothing_instead_of_failing_loudly() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open_disk(dir.path(), 0, Arc::new(Codec::plain())).unwrap();
    s.close();
    let e = make(&key(), 1, 1_700_000_000, vec![], "late");
    assert_eq!(s.insert(e), privcord_server::nostr::store::Put::Failed);
    assert!(s.query(&filter(json!({})), 0, 10).is_empty());
}
