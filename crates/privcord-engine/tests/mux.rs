//! The multi-mailbox signaling layer driven the way an adapter drives it: one connection per relay however many mailboxes,
//! routing by mailbox, one copy of each blob, one answer per put, and a link that goes quiet is replaced.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use privcord_crypto::{
    identity::Identity,
    nostr::make_event,
    rng::SeededEntropy,
    seal::{mailbox_for, seal},
};
use privcord_domain::{blob::SealedBlob, signaling::PutKind};
use privcord_engine::signaling::{
    mux::{MuxConfig, MuxEvent, SignalingMux},
    Action, PutRequest, SignalingStatus,
};
use serde_json::{json, Value};

const NOSTR: &str = "wss://nostr.example";
const RELAY: &str = "ws://relay.example";

fn mux() -> SignalingMux {
    SignalingMux::new(&MuxConfig {
        nostr: vec![NOSTR.into()],
        relays: vec![RELAY.into()],
    })
}

fn blob(seed: &str) -> SealedBlob {
    let mut rng = SeededEntropy::new(seed);
    let (a, b) = (
        Identity::generate(&mut rng).unwrap(),
        Identity::generate(&mut rng).unwrap(),
    );
    seal(
        &a,
        &b.uid().unwrap(),
        &b.dh_public(),
        &json!({ "t": "ready", "n": seed }),
        &mut rng,
    )
    .unwrap()
}

/// An anonymous envelope: nothing names a sender or a recipient, and the signature is empty.
fn envelope(iv: &str) -> SealedBlob {
    let mut b = blob(iv);
    b.v = 3;
    b.to = String::new();
    b.from = String::new();
    b.sig = String::new();
    b.iv = iv.into();
    b.kem = Some("kem".into());
    b.key = Some("key".into());
    b
}

fn sends(actions: &[Action], url: &str) -> Vec<String> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Send { url: u, text } if u == url => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn connects(actions: &[Action]) -> Vec<&str> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Connect { url } => Some(url.as_str()),
            _ => None,
        })
        .collect()
}

fn open_both(m: &mut SignalingMux, now: i64) -> Vec<Action> {
    let mut out = m.socket_opened(RELAY, now).actions;
    out.extend(m.socket_opened(NOSTR, now).actions);
    out
}

fn blobs(events: &[MuxEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            MuxEvent::Blob { mailbox, blob, .. } => Some((mailbox.clone(), blob.iv.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn ten_mailboxes_use_one_connection_per_relay() {
    let mut m = mux();
    let mut all = Vec::new();
    for i in 0..10 {
        all.extend(
            m.subscribe(&mailbox_for(&format!("user-{i}")), 1_000)
                .actions,
        );
    }
    // Two relays, so two connections, however many mailboxes.
    assert_eq!(connects(&all).len(), 2, "{all:?}");
    assert_eq!(m.link_count(), 2);
    // Once open, each relay is asked about every mailbox (and about no other).
    let asked = open_both(&mut m, 2_000);
    assert_eq!(sends(&asked, RELAY).len(), 10);
    assert_eq!(sends(&asked, NOSTR).len(), 10);
    assert_eq!(m.status(), SignalingStatus::Online);
}

#[test]
fn a_mailbox_added_later_is_asked_for_on_the_open_connections_without_a_new_one() {
    let mut m = mux();
    let first = mailbox_for("first");
    m.subscribe(&first, 0);
    open_both(&mut m, 1);
    let out = m.subscribe(&mailbox_for("second"), 2);
    assert!(connects(&out.actions).is_empty());
    assert_eq!(sends(&out.actions, RELAY).len(), 1);
    assert_eq!(sends(&out.actions, NOSTR).len(), 1);
}

#[test]
fn a_nostr_event_goes_to_its_own_mailbox_and_nowhere_else() {
    let mut m = mux();
    let (a, b) = (mailbox_for("a"), mailbox_for("b"));
    m.subscribe(&a, 0);
    m.subscribe(&b, 0);
    let asked = m.socket_opened(NOSTR, 1).actions;
    // The subscription id of each mailbox is in the request the relay got.
    let req_b: Value = sends(&asked, NOSTR)
        .iter()
        .map(|t| serde_json::from_str::<Value>(t).unwrap())
        .find(|v| v[2]["#t"][0] == b.as_str())
        .unwrap();
    let sub_b = req_b[1].as_str().unwrap().to_owned();
    let mut rng = SeededEntropy::new("ev");
    let event = make_event(&b, &blob("one"), PutKind::Sig, 120, 1, &mut rng).unwrap();
    let frame = json!(["EVENT", sub_b, event.to_value()]).to_string();
    let out = m.socket_message(NOSTR, &frame, 1_500);
    let got = blobs(&out.events);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, b, "it arrives on mailbox b");
    // The same event twice is one blob; an event for a subscription we never made is nothing.
    assert!(blobs(&m.socket_message(NOSTR, &frame, 1_600).events).is_empty());
    let stranger = json!(["EVENT", "pc-unknown", event.to_value()]).to_string();
    assert!(blobs(&m.socket_message(NOSTR, &stranger, 1_700).events).is_empty());
}

#[test]
fn a_relay_event_is_routed_by_the_mailbox_it_names_and_the_catch_up_point_is_kept_per_mailbox() {
    let mut m = mux();
    let (a, b) = (mailbox_for("a"), mailbox_for("b"));
    m.subscribe(&a, 0);
    m.subscribe(&b, 0);
    m.socket_opened(RELAY, 1);
    let ev = |mailbox: &str, id: &str, iv: &str, ts: i64| {
        json!({ "op": "event", "mailbox": mailbox, "id": id, "blob": envelope(iv).encode(), "ts": ts }).to_string()
    };
    let out = m.socket_message(RELAY, &ev(&b, "i1", "iv-1", 500), 2);
    assert_eq!(blobs(&out.events), vec![(b.clone(), "iv-1".into())]);
    // After a reconnect only mailbox b asks "since 500"; a still asks from the start.
    m.socket_closed(RELAY, 3);
    let again = m.tick(40_000).actions;
    assert_eq!(connects(&again), vec![RELAY]);
    let subs = sends(&m.socket_opened(RELAY, 40_001).actions, RELAY);
    let since = |mailbox: &str| {
        subs.iter()
            .map(|t| serde_json::from_str::<Value>(t).unwrap())
            .find(|v| v["mailbox"] == mailbox)
            .map(|v| v["since"].as_i64().unwrap())
            .unwrap()
    };
    assert_eq!(since(&b), 500);
    assert_eq!(since(&a), 0);
}

#[test]
fn every_anonymous_envelope_gets_through_and_only_a_real_copy_is_dropped() {
    // The bug that dropped every envelope after the first: they all have an empty signature.
    let mut m = mux();
    let a = mailbox_for("a");
    m.subscribe(&a, 0);
    m.socket_opened(RELAY, 1);
    let frame = |iv: &str| {
        json!({ "op": "event", "mailbox": a, "id": iv, "blob": envelope(iv).encode(), "ts": 1 })
            .to_string()
    };
    let mut got = Vec::new();
    for iv in ["one", "two", "three", "two"] {
        got.extend(blobs(&m.socket_message(RELAY, &frame(iv), 2).events));
    }
    assert_eq!(
        got.iter().map(|(_, iv)| iv.as_str()).collect::<Vec<_>>(),
        ["one", "two", "three"]
    );
}

#[test]
fn a_put_is_taken_when_any_relay_takes_it_and_failed_only_when_all_refuse() {
    let mut rng = SeededEntropy::new("put");
    let mut m = SignalingMux::new(&MuxConfig {
        nostr: vec![],
        relays: vec![RELAY.into(), "ws://other.example".into()],
    });
    let a = mailbox_for("a");
    m.subscribe(&a, 0);
    m.socket_opened(RELAY, 1);
    m.socket_opened("ws://other.example", 1);
    let out = m
        .put(
            PutRequest {
                mailbox: &a,
                blob: &blob("p"),
                kind: PutKind::Msg,
                ttl_sec: 60,
                put_id: "p1",
            },
            2,
            &mut rng,
        )
        .unwrap();
    assert_eq!(sends(&out.actions, RELAY).len(), 1);
    assert!(out.events.is_empty(), "nothing is known yet");
    // The first refusal is not the answer; the second relay takes it.
    let refused = m.socket_message(
        RELAY,
        &json!({ "op": "err", "id": "p1", "reason": "mailbox full" }).to_string(),
        3,
    );
    assert!(refused.events.is_empty());
    let taken = m.socket_message(
        "ws://other.example",
        &json!({ "op": "ok", "id": "p1" }).to_string(),
        4,
    );
    assert_eq!(
        taken.events,
        vec![MuxEvent::PutOk {
            put_id: "p1".into()
        }]
    );

    // Both refuse: one failure, with the first reason.
    m.put(
        PutRequest {
            mailbox: &a,
            blob: &blob("q"),
            kind: PutKind::Msg,
            ttl_sec: 60,
            put_id: "p2",
        },
        5,
        &mut rng,
    )
    .unwrap();
    m.socket_message(
        RELAY,
        &json!({ "op": "err", "id": "p2", "reason": "relay full" }).to_string(),
        6,
    );
    let failed = m.socket_message(
        "ws://other.example",
        &json!({ "op": "err", "id": "p2", "reason": "rate limited" }).to_string(),
        7,
    );
    assert_eq!(
        failed.events,
        vec![MuxEvent::PutFailed {
            put_id: "p2".into(),
            reason: "relay full".into()
        }]
    );
}

#[test]
fn a_put_nobody_answers_fails_and_with_no_relay_open_fails_at_once() {
    let mut rng = SeededEntropy::new("put2");
    let mut m = mux();
    let a = mailbox_for("a");
    m.subscribe(&a, 0);
    let none = m
        .put(
            PutRequest {
                mailbox: &a,
                blob: &blob("p"),
                kind: PutKind::Sig,
                ttl_sec: 60,
                put_id: "x",
            },
            1,
            &mut rng,
        )
        .unwrap();
    assert!(matches!(none.events[..], [MuxEvent::PutFailed { .. }]));
    m.socket_opened(RELAY, 2);
    // Only the PrivCord relay is open and it never answers.
    m.put(
        PutRequest {
            mailbox: &a,
            blob: &blob("p"),
            kind: PutKind::Sig,
            ttl_sec: 60,
            put_id: "y",
        },
        3,
        &mut rng,
    )
    .unwrap();
    let late = m.tick(3 + 9_000);
    assert!(late
        .events
        .iter()
        .any(|e| matches!(e, MuxEvent::PutFailed { put_id, .. } if put_id == "y")));
}

#[test]
fn a_silent_link_is_pinged_then_replaced_and_a_woken_machine_replaces_all_at_once() {
    let mut m = mux();
    m.subscribe(&mailbox_for("a"), 0);
    open_both(&mut m, 0);
    let ping = m.tick(21_000).actions;
    assert!(ping
        .iter()
        .any(|a| matches!(a, Action::Ping { url } if url == RELAY)));
    // Nothing came back at all: dead after 55 seconds, closed, and connected again after the backoff.
    let dead = m.tick(56_000).actions;
    assert!(dead
        .iter()
        .any(|a| matches!(a, Action::Close { url } if url == RELAY)));
    assert_eq!(m.status(), SignalingStatus::Error);
    // After the backoff it connects again.
    assert_eq!(connects(&m.tick(58_000).actions).len(), 2);
}

#[test]
fn an_answering_link_is_kept_and_waking_from_sleep_reconnects_every_link_at_once() {
    let mut m = mux();
    m.subscribe(&mailbox_for("a"), 0);
    open_both(&mut m, 0);
    for t in [20_000, 40_000, 60_000, 80_000] {
        m.socket_alive(RELAY, t - 1);
        m.socket_alive(NOSTR, t - 1);
        let out = m.tick(t).actions;
        assert!(
            !out.iter().any(|a| matches!(a, Action::Close { .. })),
            "kept at {t}"
        );
    }
    let woke = m.suspect_all(100_000);
    let closed = woke
        .actions
        .iter()
        .filter(|a| matches!(a, Action::Close { .. }))
        .count();
    assert_eq!(closed, 2);
    // Connected again straight away (no backoff, it is not the relay's fault).
    assert_eq!(connects(&m.tick(100_001).actions).len(), 2);
}

#[test]
fn nothing_connects_until_there_is_a_mailbox_and_stop_closes_everything() {
    let mut m = mux();
    assert!(connects(&m.tick(1).actions).is_empty());
    assert_eq!(m.status(), SignalingStatus::Error);
    m.subscribe(&mailbox_for("a"), 2);
    open_both(&mut m, 3);
    let out = m.stop();
    assert_eq!(
        out.actions
            .iter()
            .filter(|a| matches!(a, Action::Close { .. }))
            .count(),
        2
    );
    assert_eq!(m.status(), SignalingStatus::Idle);
}
