//! Randomised checks (what the fuzz targets check, run on every test run): hostile input is refused without panicking, and
//! the event store keeps its promises however events and deletions arrive.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use privcord_server::config::Config;
use privcord_server::nostr::event::{hex, Event};
use privcord_server::nostr::filter::Filter;
use privcord_server::nostr::store::{Put, Store};
use privcord_server::nostr::Nostr;
use privcord_server::relay::{Conn, Out, Relay, Session};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A small deterministic generator, so a failure can be replayed from its seed.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len() as u64) as usize]
    }
}

fn id32(n: u64) -> String {
    hex(&{
        let mut b = [0u8; 32];
        b[..8].copy_from_slice(&n.to_be_bytes());
        b
    })
}

fn event_json(rng: &mut Rng) -> Value {
    // Few authors, ids and slots, so replacements, deletions and clashes actually happen.
    let author_n = 1 + rng.below(3);
    let author = id32(author_n);
    let kind = *rng.pick(&[0u64, 1, 3, 5, 10002, 20001, 30000, 30001]);
    let mut tags: Vec<Vec<String>> = Vec::new();
    for _ in 0..rng.below(4) {
        match rng.below(5) {
            0 => tags.push(vec![
                "e".into(),
                id32(1000 * (1 + rng.below(3)) + 100 + rng.below(6)),
            ]),
            1 => tags.push(vec![
                "a".into(),
                format!(
                    "{}:{}:{}",
                    rng.pick(&[30000, 30001]),
                    id32(1 + rng.below(3)),
                    rng.pick(&["x", "y", ""])
                ),
            ]),
            2 => tags.push(vec!["d".into(), (*rng.pick(&["x", "y", ""])).into()]),
            3 => tags.push(vec!["t".into(), (*rng.pick(&["room1", "room2"])).into()]),
            _ => tags.push(vec![
                "expiration".into(),
                (1_700_000_000 + rng.below(200) as i64 - 100).to_string(),
            ]),
        }
    }
    json!({
        // an id belongs to one author, as a hash of the event would
        "id": id32(1000 * author_n + 100 + rng.below(6)), "pubkey": author, "created_at": 1_699_999_000 + rng.below(2000) as i64,
        "kind": kind, "tags": tags, "content": "x", "sig": "00".repeat(64),
    })
}

#[test]
fn the_event_store_keeps_its_promises_under_random_events_deletions_and_queries() {
    for seed in 1..=300u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let store = Store::open(None, 0).unwrap();
        // deletions that were stored: (author, target id) must never be visible afterwards
        let mut deleted: Vec<(String, String)> = Vec::new();
        let mut history: Vec<String> = Vec::new();
        for step in 0..120 {
            let v = event_json(&mut rng);
            let e = Event::parse_trusted(&v).unwrap();
            history.push(format!("step {step}: {v}"));
            let is_deletion = e.kind == 5;
            let tags = e.tags.clone();
            let (author, created) = (hex(&e.pubkey), e.created_at);
            if store.insert(Arc::new(e)) == Put::Stored && is_deletion {
                for t in &tags {
                    if t.first().map(String::as_str) == Some("e") {
                        deleted.push((author.clone(), t[1].clone()));
                    }
                }
            }
            let _ = created;
            if step % 17 == 0 {
                store.sweep(1_700_000_000 + step, 0, 0);
            }
            // 1. a deleted event never comes back, whoever published it second
            for (by, target) in &deleted {
                let f = Filter {
                    ids: vec![target.clone()],
                    authors: vec![by.clone()],
                    ..Filter::default()
                };
                for e in store.query(&f, 0, 100) {
                    if e.kind != 5 {
                        let related: Vec<&String> = history
                            .iter()
                            .filter(|h| h.contains(target.as_str()))
                            .collect();
                        panic!("seed {seed} step {step}: {target} (deleted by {by}) is back: {}\nhistory mentioning it:\n{}", e.json, related.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"));
                    }
                }
            }
        }
        store.sweep(0, 0, 0);
        let all = Filter {
            since: Some(i64::MIN),
            ..Filter::default()
        };
        let events = store.query(&all, 0, usize::MAX);
        // 2. the indexes and the table agree
        assert_eq!(store.len(), events.len(), "seed {seed}: indexes disagree");
        // 3. a replaceable or addressable slot holds one event
        let mut slots = std::collections::HashSet::new();
        for e in &events {
            if let Some(slot) = e.replace_key() {
                assert!(
                    slots.insert(slot),
                    "seed {seed}: two events in one replaceable slot"
                );
            }
        }
        // 4. every query result honours its filter, newest first
        for kind in [1u64, 30000] {
            let f = Filter {
                kinds: vec![kind],
                limit: Some(5),
                ..Filter::default()
            };
            let got = store.query(&f, 0, 100);
            assert!(got.len() <= 5 && got.iter().all(|e| e.kind == kind));
            assert!(got.windows(2).all(|w| w[0].created_at >= w[1].created_at));
        }
    }
}

fn harness() -> (
    Relay,
    Conn,
    tokio::sync::mpsc::UnboundedReceiver<Out>,
    Arc<AtomicUsize>,
) {
    let mut cfg = Config::default();
    cfg.nostr.enabled = true;
    cfg.nostr.persist = false;
    cfg.nostr.events_per_minute_per_ip = 0;
    cfg.relay.rate_limit = 10_000_000;
    cfg.relay.max_queued = 1_000;
    let nostr = Nostr::new(cfg.nostr.clone(), std::path::Path::new("/nonexistent")).unwrap();
    let relay = Relay::new(cfg.relay.clone(), None, 1_700_000_000_000).with_nostr(Arc::new(nostr));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let total = Arc::new(AtomicUsize::new(0));
    let conn = Conn {
        id: 1,
        tx,
        pending: Arc::new(AtomicUsize::new(0)),
        total: total.clone(),
        limit: 1 << 24,
    };
    (relay, conn, rx, total)
}

#[test]
fn hostile_frames_in_both_protocols_never_panic_and_never_pile_up() {
    let dictionary: Vec<String> = vec![
        r#"{"op":"sub","mailbox":"mb"}"#.into(),
        r#"{"op":"put","mailbox":"mb","id":"m1","blob":"AAAA","ttl":60,"kind":"sig"}"#.into(),
        r#"{"op":"put","mailbox":"mb","id":"m2","blob":"AAAA","ttl":99999999999,"kind":"msg"}"#
            .into(),
        r#"{"op":"del","mailbox":"mb","id":"m1"}"#.into(),
        r##"["REQ","a",{"kinds":[1],"#t":["x"],"limit":5},{"authors":["ab"],"since":1}]"##.into(),
        r#"["REQ","b",{"ids":["00"],"limit":0}]"#.into(),
        r#"["COUNT","c",{"kinds":[1,2,3]}]"#.into(),
        r#"["CLOSE","a"]"#.into(),
        r#"["AUTH",{"id":"x"}]"#.into(),
        format!(r#"["EVENT",{}]"#, event_json(&mut Rng(7))),
        r#"["EVENT",{"id":1,"kind":"x","tags":[["a"],[1]]}]"#.into(),
        "[".into(),
        "{".into(),
        "null".into(),
        "[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[".into(),
        "{\"op\":\"put\",\"mailbox\":\"\\u0000\",\"id\":\"\\ud800\"}".into(),
    ];
    for seed in 1..=400u64 {
        let mut rng = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) | 1);
        let (relay, conn, mut rx, total) = harness();
        let mut session = Session::default();
        for i in 0..150 {
            let mut frame: Vec<u8> = rng.pick(&dictionary).clone().into_bytes();
            // sometimes damage it: flip, drop, or add bytes
            for _ in 0..rng.below(4) {
                if frame.is_empty() {
                    break;
                }
                let at = rng.below(frame.len() as u64) as usize;
                match rng.below(3) {
                    0 => frame[at] = rng.next() as u8,
                    1 => {
                        frame.remove(at);
                    }
                    _ => frame.insert(at, rng.next() as u8),
                }
            }
            relay.frame(
                &conn,
                &mut session,
                "203.0.113.7",
                &String::from_utf8_lossy(&frame),
                1_700_000_000_000 + i,
            );
            while rx.try_recv().is_ok() {}
        }
        relay.leave("203.0.113.7", &conn, &mut session);
        relay.sweep(1_800_000_000_000);
        // what was queued for the connection is all accounted for once it is read
        assert!(
            total.load(Ordering::Relaxed) < 1 << 24,
            "seed {seed}: queue grew without bound"
        );
    }
}
