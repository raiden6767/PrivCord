//! A simulation of real use: a few people on phones that are opened, put in the background, restarted and reinstalled at
//! random, who add each other by QR code or six words, remove each other and add again, with a relay that only hands a message
//! to the phones listening at that moment (unless it was stored). After every step, rules that must always hold are checked;
//! when one breaks, the test prints the seed and the exact list of steps that led there.
//!
//! The rules (connection setup, the part that decides whether two phones ever find each other):
//!  * someone who added a friend from their card or six words, and the friend then had their app open for a few rounds within
//!    half an hour, must have reached the friend as a request (or as a contact) - however the friend's app was opened and
//!    closed meanwhile;
//!  * two people who are friends on both sides and kept their apps open for a few rounds receive each other's messages.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use privcord_crypto::rng::SeededEntropy;
use privcord_domain::UserId;
use privcord_engine::{Core, ErrorCode, Ports};
use privcord_ports::testing::{
    FakeClock, MemorySecureStore, MemoryStorage, RecordingNotifier, RecordingTransport, SignalOp,
};
use privcord_ports::Clock;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

/// A relay: a message is handed at once to every phone that is listening and online; it is also kept (for those that listen
/// later) only if it was sent as a stored one.
#[derive(Default)]
struct Relay {
    kept: HashMap<String, Vec<(String, i64)>>,
}

struct Phone {
    core: Core,
    transport: Arc<RecordingTransport>,
    storage: Arc<MemoryStorage>,
    secure: Arc<MemorySecureStore>,
    seed: String,
    installs: u32,
    subscribed: BTreeSet<String>,
    /// How much of each mailbox's kept list this phone has been given.
    read: HashMap<String, usize>,
    open: bool,
    /// Rounds this phone has been open for (never reset: compared with a value noted earlier).
    open_rounds: u64,
}

impl Phone {
    fn install(seed: &str, clock: &Arc<FakeClock>, installs: u32) -> Self {
        let storage = MemoryStorage::new();
        let secure = MemorySecureStore::new();
        let mut p = Self::start(seed, clock, storage, secure, installs);
        p.core.create_identity().unwrap();
        p.core.set_store_forward(false).unwrap();
        p
    }

    fn start(
        seed: &str,
        clock: &Arc<FakeClock>,
        storage: Arc<MemoryStorage>,
        secure: Arc<MemorySecureStore>,
        installs: u32,
    ) -> Self {
        let transport = RecordingTransport::new();
        transport.set_signaling(true);
        let core = Core::new(Ports {
            storage: storage.clone(),
            secure: secure.clone(),
            transport: transport.clone(),
            clock: clock.clone(),
            notifier: RecordingNotifier::new(),
            entropy: Box::new(SeededEntropy::new(&format!("{seed}-{installs}"))),
        })
        .unwrap();
        Self {
            core,
            transport,
            storage,
            secure,
            seed: seed.to_owned(),
            installs,
            subscribed: BTreeSet::new(),
            read: HashMap::new(),
            open: true,
            open_rounds: 0,
        }
    }

    fn uid(&self) -> UserId {
        self.core.own_uid().unwrap().clone()
    }

    /// The app is closed and opened again on the same phone.
    fn restart(&mut self, clock: &Arc<FakeClock>) {
        let next = Self::start(
            &self.seed.clone(),
            clock,
            self.storage.clone(),
            self.secure.clone(),
            self.installs + 1,
        );
        let (seed, installs, rounds) = (self.seed.clone(), self.installs + 1, self.open_rounds);
        *self = next;
        self.seed = seed;
        self.installs = installs;
        self.open_rounds = rounds;
    }
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    since: i64,
    their_rounds_then: u64,
    own_rounds_then: u64,
}

struct World {
    clock: Arc<FakeClock>,
    relay: Relay,
    phones: Vec<Phone>,
    trace: Vec<String>,
    /// (adder, added) whose request must arrive.
    pending: HashMap<(usize, usize), Pending>,
    /// Messages that must arrive: (from, to, text, sent-at rounds of the recipient).
    owed: Vec<(usize, usize, String, u64, i64)>,
    rng: u64,
    sent: u32,
    /// Whether messages wait on the relay (the simulation has no direct links, so only then can a message arrive).
    store_forward: bool,
}

impl World {
    fn new(seed: u64, people: usize, store_forward: bool) -> Self {
        let clock = FakeClock::at(1_700_000_000_000);
        let mut phones: Vec<Phone> = (0..people)
            .map(|i| Phone::install(&format!("sim-{seed}-{i}"), &clock, 0))
            .collect();
        for p in &mut phones {
            p.core.set_store_forward(store_forward).unwrap();
        }
        Self {
            clock,
            relay: Relay::default(),
            phones,
            trace: Vec::new(),
            pending: HashMap::new(),
            owed: Vec::new(),
            rng: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            sent: 0,
            store_forward,
        }
    }

    fn rand(&mut self, n: usize) -> usize {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % n as u64) as usize
    }

    fn note(&mut self, s: String) {
        self.trace.push(s);
    }

    /// One short while with the phone open: it works, posts, and is handed what is waiting for it.
    fn round(&mut self, i: usize) {
        let now = self.clock.now_ms();
        for _ in 0..2 {
            self.phones[i].core.tick();
            let ops = self.phones[i].transport.drain_signals();
            for op in ops {
                match op {
                    SignalOp::Subscribe(m) => {
                        self.phones[i].subscribed.insert(m);
                    }
                    SignalOp::Unsubscribe(m) => {
                        self.phones[i].subscribed.remove(&m);
                    }
                    SignalOp::Put {
                        mailbox,
                        blob,
                        ttl_sec,
                        ..
                    } => {
                        // Handed to every other phone that is open and listening.
                        for j in 0..self.phones.len() {
                            if j != i
                                && self.phones[j].open
                                && self.phones[j].subscribed.contains(&mailbox)
                            {
                                self.phones[j].core.signal_blob(&mailbox, &blob);
                            }
                        }
                        if ttl_sec >= 300 {
                            self.relay
                                .kept
                                .entry(mailbox)
                                .or_default()
                                .push((blob, now + i64::from(ttl_sec) * 1000));
                        }
                    }
                }
            }
            // What was kept for a mailbox this phone listens on.
            for m in self.phones[i].subscribed.clone() {
                let list = self.relay.kept.get(&m).cloned().unwrap_or_default();
                let from = self.phones[i].read.get(&m).copied().unwrap_or(0);
                for (blob, until) in list.iter().skip(from) {
                    if *until > now {
                        self.phones[i].core.signal_blob(&m, blob);
                    }
                }
                self.phones[i].read.insert(m, list.len());
            }
        }
        self.phones[i].open_rounds += 1;
    }

    fn settle(&mut self) {
        for i in 0..self.phones.len() {
            if self.phones[i].open {
                self.round(i);
            }
        }
    }

    fn knows(&self, a: usize, b: usize) -> bool {
        let bu = self.phones[b].uid();
        self.phones[a]
            .core
            .snapshot()
            .contacts
            .iter()
            .any(|c| c.uid == bu)
    }

    fn requested(&self, a: usize, b: usize) -> bool {
        let au = self.phones[a].uid();
        self.phones[b]
            .core
            .snapshot()
            .requests
            .iter()
            .any(|r| r.uid == au)
    }

    /// Two friends (on both sides) who have the app open write to each other: the way keys get used, and the way a broken
    /// friendship shows.
    fn chat(&mut self) {
        let n = self.phones.len();
        let mut pairs = Vec::new();
        for a in 0..n {
            for b in 0..n {
                if a != b
                    && self.phones[a].open
                    && self.phones[b].open
                    && self.knows(a, b)
                    && self.knows(b, a)
                {
                    pairs.push((a, b));
                }
            }
        }
        if pairs.is_empty() {
            return;
        }
        let (a, b) = pairs[self.rand(pairs.len())];
        let now = self.clock.now_ms();
        let text = format!("m{}", self.sent);
        self.sent += 1;
        let bu = self.phones[b].uid();
        if self.phones[a].core.send_message(&bu, &text, None).is_ok() {
            self.note(format!("{a} sends {text} to {b}"));
            if self.store_forward {
                let rounds = self.phones[b].open_rounds;
                self.owed.push((a, b, text, rounds, now));
            }
        }
    }

    /// One random thing a person does.
    fn act(&mut self) {
        if self.rand(100) < 30 {
            return self.chat();
        }
        let n = self.phones.len();
        let a = self.rand(n);
        let mut b = self.rand(n);
        if b == a {
            b = (a + 1) % n;
        }
        let now = self.clock.now_ms();
        match self.rand(15) {
            // Add someone from their QR code (their card).
            0..=3 if self.phones[a].open && !self.knows(a, b) => {
                let card = self.phones[b].core.own_card(Some("X")).unwrap();
                if self.phones[a].core.accept_invite(&card).is_ok() {
                    self.note(format!("{a} scans {b}'s QR code"));
                    self.pending.insert(
                        (a, b),
                        Pending {
                            since: now,
                            their_rounds_then: self.phones[b].open_rounds,
                            own_rounds_then: self.phones[a].open_rounds,
                        },
                    );
                }
            }
            // Six words: b shows them, then maybe leaves the app to send them, a types them.
            4 if self.phones[a].open && self.phones[b].open && !self.knows(a, b) => {
                let Ok((words, _)) = self.phones[b].core.create_invite() else {
                    return;
                };
                self.note(format!("{b} shows six words"));
                self.round(b);
                self.round(a);
                if self.phones[a].core.preview_invite(&words).is_ok() {
                    for _ in 0..2 {
                        self.round(a);
                        self.round(b);
                    }
                    // Often the person showing the words has left the app to paste them somewhere.
                    if self.rand(2) == 0 {
                        self.phones[b].open = false;
                        self.note(format!("{b} leaves the app"));
                    }
                    if self.phones[a].core.accept_invite(&words).is_ok() {
                        self.note(format!("{a} types {b}'s six words"));
                        self.pending.insert(
                            (a, b),
                            Pending {
                                since: now,
                                their_rounds_then: self.phones[b].open_rounds,
                                own_rounds_then: self.phones[a].open_rounds,
                            },
                        );
                    }
                }
            }
            // Accept whoever is waiting for a tap.
            5 | 6 if self.phones[a].open => {
                let reqs: Vec<UserId> = self.phones[a]
                    .core
                    .snapshot()
                    .requests
                    .iter()
                    .map(|r| r.uid.clone())
                    .collect();
                for u in reqs {
                    if self.phones[a].core.accept_request(&u).is_ok() {
                        self.note(format!("{a} accepts a request"));
                    }
                }
            }
            // Remove a friend (the other keeps theirs, or removes too).
            7 | 8 if self.phones[a].open && self.knows(a, b) => {
                let bu = self.phones[b].uid();
                if self.phones[a].core.remove_contact(&bu).is_ok() {
                    self.note(format!("{a} removes {b}"));
                    self.forget_pair(a, b);
                    if self.rand(2) == 0 && self.phones[b].open && self.knows(b, a) {
                        let au = self.phones[a].uid();
                        if self.phones[b].core.remove_contact(&au).is_ok() {
                            self.note(format!("{b} removes {a}"));
                        }
                    }
                }
            }
            // The app goes to the background, or comes back.
            9 | 10 => {
                let o = self.phones[a].open;
                self.phones[a].open = !o;
                // Coming back: the connection layer reconnects and asks again for what it listens to (it remembers
                // its mailboxes and how far it has been given each one).
                self.note(format!(
                    "{a} {}",
                    if o {
                        "goes to the background"
                    } else {
                        "comes back"
                    }
                ));
            }
            // The app is closed and opened again.
            11 if self.phones[a].open => {
                let clock = self.clock.clone();
                self.phones[a].restart(&clock);
                self.note(format!("{a} restarts the app"));
            }
            // A message between friends.
            12..=14 if self.phones[a].open && self.knows(a, b) && self.knows(b, a) => {
                let text = format!("m{}", self.sent);
                self.sent += 1;
                let bu = self.phones[b].uid();
                if self.phones[a].core.send_message(&bu, &text, None).is_ok() {
                    self.note(format!("{a} sends {text} to {b}"));
                    if self.store_forward {
                        let rounds = self.phones[b].open_rounds;
                        self.owed.push((a, b, text, rounds, now));
                    }
                }
            }
            // Time passes.
            _ => {
                let secs = [5, 30, 120, 600][self.rand(4)];
                self.clock.advance(secs * 1000);
                self.note(format!("{secs} s pass"));
            }
        }
    }

    fn forget_pair(&mut self, a: usize, b: usize) {
        self.pending.remove(&(a, b));
        self.pending.remove(&(b, a));
        self.owed
            .retain(|(f, t, ..)| !((*f == a && *t == b) || (*f == b && *t == a)));
    }

    /// Rules that must always hold. Returns what is broken.
    fn check(&mut self) -> Option<String> {
        let now = self.clock.now_ms();
        let mut done = Vec::new();
        for (&(a, b), p) in &self.pending {
            if self.knows(b, a) || self.requested(a, b) {
                done.push((a, b));
                continue;
            }
            let b_rounds = self.phones[b].open_rounds - p.their_rounds_then;
            let a_rounds = self.phones[a].open_rounds - p.own_rounds_then;
            // Both were around long enough (a keeps asking while it is open; b listens while it is open), within the half
            // hour a keeps asking for.
            if now - p.since <= 25 * 60_000 && b_rounds >= 6 && a_rounds >= 6 {
                return Some(format!("{a} added {b}, {b} had the app open for {b_rounds} rounds, but {b} never got a request"));
            }
        }
        for k in done {
            self.pending.remove(&k);
        }
        let mut delivered = Vec::new();
        for (idx, (a, b, text, rounds_then, _)) in self.owed.iter().enumerate() {
            let got = self.phones[*b]
                .core
                .snapshot()
                .messages
                .iter()
                .flat_map(|(_, v)| v)
                .any(|m| &m.text == text);
            if got {
                delivered.push(idx);
                continue;
            }
            let b_rounds = self.phones[*b].open_rounds - rounds_then;
            if b_rounds >= 10
                && self.phones[*a].open_rounds >= 10
                && self.knows(*a, *b)
                && self.knows(*b, *a)
            {
                return Some(format!("{a} sent {text} to {b}, who had the app open for {b_rounds} rounds, and never got it"));
            }
        }
        for i in delivered.into_iter().rev() {
            self.owed.remove(i);
        }
        None
    }
}

fn run(seed: u64, people: usize, store_forward: bool, steps: usize) -> Result<(), String> {
    let mut w = World::new(seed, people, store_forward);
    let mut mutual_steps = 0;
    for step in 0..steps {
        w.act();
        w.settle();
        if (0..people).any(|a| (0..people).any(|b| a != b && w.knows(a, b) && w.knows(b, a))) {
            mutual_steps += 1;
        }
        if std::env::var("SIM_VERBOSE").is_ok() && step + 1 == steps {
            eprintln!("seed {seed} sf={store_forward}: {mutual_steps}/{steps} steps with a mutual pair, {} messages sent, {} still owed", w.sent, w.owed.len());
        }
        if let Some(broken) = w.check() {
            return Err(format!(
                "seed {seed} (store-and-forward {store_forward}), step {step}: {broken}\nwhat happened:\n  {}",
                w.trace.join("\n  ")
            ));
        }
    }
    Ok(())
}

#[test]
fn connection_setup_survives_phones_that_come_and_go() {
    let seeds: u64 = std::env::var("SIM_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let mut failures = Vec::new();
    for seed in 0..seeds {
        for store_forward in [false, true] {
            if let Err(e) = run(seed, 3, store_forward, 70) {
                failures.push(e);
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} runs broke a rule; the first:\n{}",
        failures.len(),
        seeds * 2,
        failures.first().unwrap()
    );
    let _ = ErrorCode::Busy;
}
