//! The Nostr event store under any sequence of events and queries. Whatever happens, its indexes must agree with each
//! other: asking for everything must return exactly as many events as the store says it holds.
#![no_main]
use libfuzzer_sys::fuzz_target;
use privcord_server::nostr::event::Event;
use privcord_server::nostr::filter::Filter;
use privcord_server::nostr::store::Store;
use std::sync::Arc;

fuzz_target!(|data: &[u8]| {
    let Ok(store) = Store::open(None, 0) else {
        return;
    };
    let mut filters = Vec::new();
    for (i, line) in data.split(|b| *b == b'\n').take(64).enumerate() {
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("sig").is_some() {
            // A stored event (read the way the server reads its own log: shape and id only).
            if let Some(e) = Event::parse_trusted(&v) {
                let _ = store.insert(Arc::new(e));
            }
        } else if let Ok(f) = Filter::parse(&v) {
            let _ = store.query(&f, 1_700_000_000, 50);
            filters.push(f);
        }
        if i % 16 == 15 {
            store.sweep(1_700_000_000 + i as i64, 3600, 20);
        }
    }
    // Drop what is expired as of time 0, then the table and the indexes must hold exactly the same events.
    store.sweep(0, 0, 0);
    let everything = Filter { since: Some(i64::MIN), ..Filter::default() };
    assert_eq!(store.len(), store.query(&everything, 0, usize::MAX).len(), "indexes disagree with the main table");
});
