//! The diagnostic log: what it keeps, what it never lets in, that it survives a restart and can be emptied or turned off.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use privcord_crypto::rng::SeededEntropy;
use privcord_domain::{privacy::ContactTrust, state::ConnectionPath, UserId};
use privcord_engine::{Core, Ports};
use privcord_ports::testing::{
    FakeClock, MemorySecureStore, MemoryStorage, RecordingNotifier, RecordingTransport,
};
use std::sync::Arc;

struct Node {
    core: Core,
    transport: Arc<RecordingTransport>,
    storage: Arc<MemoryStorage>,
    secure: Arc<MemorySecureStore>,
    uid: UserId,
}

fn start(
    seed: &str,
    clock: &Arc<FakeClock>,
    storage: Arc<MemoryStorage>,
    secure: Arc<MemorySecureStore>,
    transport: Arc<RecordingTransport>,
) -> Core {
    Core::new(Ports {
        storage,
        secure,
        transport,
        clock: clock.clone(),
        notifier: RecordingNotifier::new(),
        entropy: Box::new(SeededEntropy::new(seed)),
    })
    .unwrap()
}

fn node(seed: &str, clock: &Arc<FakeClock>) -> Node {
    let (storage, secure, transport) = (
        MemoryStorage::new(),
        MemorySecureStore::new(),
        RecordingTransport::new(),
    );
    let mut core = start(
        seed,
        clock,
        storage.clone(),
        secure.clone(),
        transport.clone(),
    );
    core.create_identity().unwrap();
    let uid = core.own_uid().unwrap().clone();
    Node {
        core,
        transport,
        storage,
        secure,
        uid,
    }
}

fn pair(clock: &Arc<FakeClock>) -> (Node, Node) {
    let (mut a, mut b) = (node("alice", clock), node("bob", clock));
    let (ac, bc) = (
        a.core.own_card(Some("Alice Wonderland")).unwrap(),
        b.core.own_card(Some("Bob Builder")).unwrap(),
    );
    a.core.add_contact(&bc, ContactTrust::Verified).unwrap();
    b.core.add_contact(&ac, ContactTrust::Verified).unwrap();
    (a, b)
}

#[test]
fn the_log_says_what_happened_and_holds_nothing_it_must_not() {
    let clock = FakeClock::at(1_700_000_000_000);
    let (mut a, b) = pair(&clock);
    a.transport.set_signaling(true);
    a.core.tick();
    a.transport.set_connected(&b.uid, true);
    a.core.link_up(&b.uid, ConnectionPath::DirectLan).unwrap();
    a.core
        .send_message(&b.uid, "my secret plan: tuesday", None)
        .unwrap();
    a.core.diag_log("ui", "tapped Send", "chars=23");
    a.core
        .diag_log("nonsense", "from a screen", "path=192.168.4.7:51820");
    clock.advance(11_000);
    a.core.tick();
    a.core.link_down(&b.uid);
    let log = a.core.diag_export("PrivCord 0.1.31\nAndroid 14 · Pixel 8");
    assert!(log.starts_with("# PrivCord diagnostic log"), "{log}");
    assert!(log.contains("link up"), "{log}");
    assert!(log.contains("link down"), "{log}");
    assert!(log.contains("[ui] tapped Send"), "{log}");
    assert!(
        log.contains("[app] from a screen"),
        "an unknown place is filed under app: {log}"
    );
    assert!(log.contains("PrivCord 0.1.31"), "{log}");
    // Never: the text, names, the friend's id, an address.
    assert!(!log.contains("secret plan"), "{log}");
    assert!(
        !log.contains("Wonderland") && !log.contains("Builder"),
        "{log}"
    );
    assert!(
        !log.contains(b_id(&b).as_str()),
        "the friend's id is an alias: {log}"
    );
    assert!(!log.contains("192.168.4.7"), "{log}");
    assert!(log.contains("id-1"), "{log}");
}

fn b_id(b: &Node) -> String {
    b.uid.as_str().to_owned()
}

#[test]
fn the_log_survives_a_restart_and_can_be_emptied_or_turned_off() {
    let clock = FakeClock::at(1_700_000_000_000);
    let mut a = node("alice", &clock);
    a.core.diag_log("ui", "opened Settings", "");
    a.core.diag_log("ui", "tapped Save log", "");
    clock.advance(11_000);
    a.core.tick(); // saved
    let n = a.core.diag_len();
    assert!(n >= 2);
    let Node {
        core,
        transport,
        storage,
        secure,
        ..
    } = a;
    drop(core);
    let mut core = start("alice again", &clock, storage, secure, transport);
    assert_eq!(core.diag_len(), n, "the log is still there after a restart");
    assert!(core.diag_export("").contains("opened Settings"));
    core.diag_clear();
    assert_eq!(core.diag_len(), 0);
    assert!(!core.diag_export("").contains("opened Settings"));
    // Off: nothing is kept, and what was there is gone.
    core.diag_log("ui", "before off", "x");
    core.set_diag_enabled(false);
    assert!(!core.diag_enabled());
    core.diag_log("ui", "while off", "x");
    assert_eq!(core.diag_len(), 0);
    core.set_diag_enabled(true);
    core.diag_log("ui", "after on", "x");
    let log = core.diag_export("");
    assert!(log.contains("after on") && !log.contains("while off") && !log.contains("before off"));
}

#[test]
fn a_long_log_is_kept_in_chunks_and_loads_whole() {
    let clock = FakeClock::at(1_700_000_000_000);
    let mut a = node("alice", &clock);
    for i in 0..1_300 {
        a.core.diag_log("ui", "step", &format!("n={i}"));
        clock.advance(10);
    }
    clock.advance(20_000);
    a.core.tick();
    let before = a.core.diag_len();
    assert!(before >= 1_300);
    let Node {
        core,
        transport,
        storage,
        secure,
        ..
    } = a;
    drop(core);
    let mut core = start("alice again", &clock, storage, secure, transport);
    assert_eq!(core.diag_len(), before);
    let log = core.diag_export("");
    assert!(
        log.contains("n=0\n") && log.contains("n=1299\n"),
        "first and last are both there"
    );
}

#[test]
fn the_same_event_again_and_again_is_counted_not_listed() {
    let clock = FakeClock::at(1_700_000_000_000);
    let mut a = node("alice", &clock);
    let base = a.core.diag_len();
    for _ in 0..50 {
        a.core.diag_log("net", "relay refused", "code=429");
        clock.advance(100);
    }
    assert_eq!(a.core.diag_len(), base + 1);
    assert!(a
        .core
        .diag_export("")
        .contains("relay refused — code=429 ×50"));
}
