//! What any client can send to the relay: PrivCord frames and Nostr frames, one per line, on one connection, in any order.
//! The server may refuse them; it must not panic, and what it queues for the client must stay bounded.
#![no_main]
use libfuzzer_sys::fuzz_target;
use privcord_server::config::Config;
use privcord_server::nostr::Nostr;
use privcord_server::relay::{Conn, Out, Relay, Session};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

fuzz_target!(|data: &[u8]| {
    let mut cfg = Config::default();
    cfg.nostr.enabled = true;
    cfg.nostr.persist = false;
    cfg.nostr.events_per_minute_per_ip = 0;
    cfg.relay.rate_limit = 1_000_000;
    let Ok(nostr) = Nostr::new(cfg.nostr.clone(), std::path::Path::new("/nonexistent")) else {
        return;
    };
    let relay = Relay::new(cfg.relay.clone(), None, 1_700_000_000_000).with_nostr(Arc::new(nostr));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Out>();
    let conn = Conn { id: 1, tx, pending: Arc::new(AtomicUsize::new(0)), total: Arc::new(AtomicUsize::new(0)), limit: 1 << 20 };
    let mut session = Session::default();
    let mut now = 1_700_000_000_000i64;
    for line in data.split(|b| *b == b'\n').take(200) {
        let text = String::from_utf8_lossy(line);
        now += 7;
        relay.frame(&conn, &mut session, "203.0.113.7", &text, now);
        while rx.try_recv().is_ok() {}
    }
    relay.leave("203.0.113.7", &conn, &mut session);
    relay.sweep(now + 10_000_000);
});
