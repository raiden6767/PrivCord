//! Signaling for every mailbox over **one connection per relay**.
//!
//! The older clients (`hub`, `relay`, `nostr`) listen on a single mailbox each, so an install with ten mailboxes opened
//! thirty sockets (ten per relay) and could run into a relay's connection limit. This keeps one link per URL, however many
//! mailboxes are listened on, and does everything the platform used to do in its own copy: reconnect with backoff, ask
//! again for what was missed, notice a silent link, publish to every relay and report one answer, and drop copies.
//!
//! Nothing here opens a socket or reads a clock. The adapter owns the WebSockets and reports what happened
//! (`socket_opened`, `socket_message`, `socket_closed`, `socket_alive`, `tick`); it carries out the [`Action`]s that come
//! back. That is all the platform has to be.

use super::{Action, PutRequest, PUT_TIMEOUT_MS};
use privcord_crypto::{
    nostr::{accept_relay_message, event_frame, make_event, NOSTR_LOOKBACK_SECS},
    rng::Entropy,
    seal::replay_id,
    CryptoError,
};
use privcord_domain::{
    blob::SealedBlob,
    signaling::{relay, socket_backoff_ms, SeenSet, SignalingStatus},
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// A link that has said nothing for this long (not even to a ping) is dead, whatever the OS thinks.
pub const DEAD_AFTER_MS: i64 = 55_000;
/// How often the adapter is asked to ping an open link.
pub const PING_EVERY_MS: i64 = 20_000;
/// A relay that holds more subscriptions than this on one connection refuses the rest; stay under the smallest limit seen.
pub const MAX_MAILBOXES: usize = 20;

/// Which relays to use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MuxConfig {
    /// Public Nostr relays (`wss://…`).
    pub nostr: Vec<String>,
    /// PrivCord relays (`ws://…` or `wss://…`, with `?token=` when the relay wants one).
    pub relays: Vec<String>,
}

/// News for the owner of the mux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxEvent {
    /// A blob that arrived for `mailbox`, with the service's id for it (used to remove it).
    Blob {
        mailbox: String,
        blob: Box<SealedBlob>,
        id: String,
    },
    /// A put was taken by at least one relay.
    PutOk { put_id: String },
    /// No relay took a put.
    PutFailed { put_id: String, reason: String },
}

/// What a call returns: sockets to drive, and news.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MuxOut {
    pub actions: Vec<Action>,
    pub events: Vec<MuxEvent>,
}

impl MuxOut {
    fn extend(&mut self, other: Self) {
        self.actions.extend(other.actions);
        self.events.extend(other.events);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Nostr,
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Down,
    Connecting,
    Open,
}

#[derive(Debug)]
struct Link {
    url: String,
    kind: Kind,
    state: State,
    attempt: u32,
    retry_at: Option<i64>,
    last_rx: i64,
    last_ping: i64,
}

#[derive(Debug)]
struct Sub {
    /// The id this mailbox's subscription has on Nostr relays.
    sub_id: String,
    /// The newest time a PrivCord relay told us about for this mailbox, so a reconnect asks only for what is newer.
    last_seen: i64,
}

/// Where one put stands: the relays still to answer, and whether any took it.
#[derive(Debug)]
struct Put {
    awaiting: BTreeSet<String>,
    done: bool,
    first_failure: Option<String>,
    deadline: i64,
}

#[derive(Debug)]
pub struct SignalingMux {
    links: Vec<Link>,
    subs: BTreeMap<String, Sub>,
    sub_to_mailbox: BTreeMap<String, String>,
    /// Nostr event ids, so a relay that sends one twice (or two that both hold it) counts once.
    events_seen: SeenSet,
    /// Blobs by what they say, so the first copy from any relay wins.
    blobs_seen: SeenSet,
    puts: BTreeMap<String, Put>,
    stopped: bool,
    ping_every_ms: i64,
    dead_after_ms: i64,
}

impl SignalingMux {
    pub fn new(config: &MuxConfig) -> Self {
        let mut links = Vec::new();
        let mut add = |urls: &[String], kind: Kind| {
            for url in urls {
                if !links.iter().any(|l: &Link| l.url == *url) {
                    links.push(Link {
                        url: url.clone(),
                        kind,
                        state: State::Down,
                        attempt: 0,
                        retry_at: None,
                        last_rx: 0,
                        last_ping: 0,
                    });
                }
            }
        };
        add(&config.relays, Kind::Relay);
        add(&config.nostr, Kind::Nostr);
        Self {
            links,
            subs: BTreeMap::new(),
            sub_to_mailbox: BTreeMap::new(),
            events_seen: SeenSet::default(),
            blobs_seen: SeenSet::default(),
            puts: BTreeMap::new(),
            stopped: false,
            ping_every_ms: PING_EVERY_MS,
            dead_after_ms: DEAD_AFTER_MS,
        }
    }

    /// Shorter (or longer) than the defaults: how often a quiet link is pinged and how long before it is given up on.
    #[must_use]
    pub fn with_timing(mut self, ping_every_ms: i64, dead_after_ms: i64) -> Self {
        self.ping_every_ms = ping_every_ms;
        self.dead_after_ms = dead_after_ms;
        self
    }

    pub fn enabled(&self) -> bool {
        !self.links.is_empty()
    }

    /// How many connections are configured (one per relay, however many mailboxes).
    pub fn link_count(&self) -> usize {
        self.links.len()
    }

    pub fn mailboxes(&self) -> Vec<String> {
        self.subs.keys().cloned().collect()
    }

    pub fn status(&self) -> SignalingStatus {
        if self.stopped || self.links.is_empty() {
            return SignalingStatus::Idle;
        }
        if self.links.iter().any(|l| l.state == State::Open) {
            SignalingStatus::Online
        } else if self.links.iter().any(|l| l.state == State::Connecting) {
            SignalingStatus::Connecting
        } else {
            SignalingStatus::Error
        }
    }

    /// How many links are open right now (for the connection stage shown to the person).
    pub fn open_links(&self) -> usize {
        self.links.iter().filter(|l| l.state == State::Open).count()
    }

    fn sub_id_for(mailbox: &str) -> String {
        format!("pc-{}", mailbox.chars().take(20).collect::<String>())
    }

    /// Listen on a mailbox. Connects the relays if they are not connected yet; a link that is already open just gets the
    /// subscription added.
    pub fn subscribe(&mut self, mailbox: &str, now_ms: i64) -> MuxOut {
        let mut out = MuxOut::default();
        if self.subs.contains_key(mailbox) || self.subs.len() >= MAX_MAILBOXES {
            return out;
        }
        let sub_id = Self::sub_id_for(mailbox);
        self.sub_to_mailbox
            .insert(sub_id.clone(), mailbox.to_owned());
        self.subs.insert(
            mailbox.to_owned(),
            Sub {
                sub_id,
                last_seen: 0,
            },
        );
        self.stopped = false;
        let open: Vec<String> = self
            .links
            .iter()
            .filter(|l| l.state == State::Open)
            .map(|l| l.url.clone())
            .collect();
        for url in open {
            out.actions
                .extend(self.subscribe_frames(&url, mailbox, now_ms));
        }
        out.extend(self.connect_down_links(now_ms));
        out
    }

    /// Stop listening on a mailbox (the connections stay: other mailboxes still use them).
    pub fn unsubscribe(&mut self, mailbox: &str) -> MuxOut {
        let mut out = MuxOut::default();
        let Some(sub) = self.subs.remove(mailbox) else {
            return out;
        };
        self.sub_to_mailbox.remove(&sub.sub_id);
        for l in &self.links {
            if l.kind == Kind::Nostr && l.state == State::Open {
                out.actions.push(Action::Send {
                    url: l.url.clone(),
                    text: json!(["CLOSE", sub.sub_id]).to_string(),
                });
            }
        }
        out
    }

    /// The frames that ask one link for one mailbox.
    fn subscribe_frames(&self, url: &str, mailbox: &str, now_ms: i64) -> Option<Action> {
        let link = self.links.iter().find(|l| l.url == url)?;
        let sub = self.subs.get(mailbox)?;
        let text = match link.kind {
            Kind::Nostr => privcord_crypto::nostr::req_frame(
                &sub.sub_id,
                mailbox,
                now_ms / 1000 - NOSTR_LOOKBACK_SECS,
            ),
            Kind::Relay => relay::sub_frame(mailbox, sub.last_seen),
        };
        Some(Action::Send {
            url: url.to_owned(),
            text,
        })
    }

    fn connect_down_links(&mut self, now_ms: i64) -> MuxOut {
        let mut out = MuxOut::default();
        if self.stopped || self.subs.is_empty() {
            return out;
        }
        for l in &mut self.links {
            if l.state == State::Down && l.retry_at.is_none_or(|t| t <= now_ms) {
                l.state = State::Connecting;
                l.retry_at = None;
                out.actions.push(Action::Connect { url: l.url.clone() });
            }
        }
        out
    }

    /// The adapter opened a socket: ask for every mailbox, from where each left off.
    pub fn socket_opened(&mut self, url: &str, now_ms: i64) -> MuxOut {
        let mut out = MuxOut::default();
        let Some(l) = self.links.iter_mut().find(|l| l.url == url) else {
            return out;
        };
        l.state = State::Open;
        l.attempt = 0;
        l.retry_at = None;
        l.last_rx = now_ms;
        l.last_ping = now_ms;
        let mailboxes: Vec<String> = self.subs.keys().cloned().collect();
        for m in mailboxes {
            out.actions.extend(self.subscribe_frames(url, &m, now_ms));
        }
        out
    }

    /// Anything arrived on the socket, a pong included: it is alive.
    pub fn socket_alive(&mut self, url: &str, now_ms: i64) {
        if let Some(l) = self.links.iter_mut().find(|l| l.url == url) {
            l.last_rx = now_ms;
        }
    }

    /// The socket failed to open or dropped: try again after the backoff, and settle what was waiting on it.
    pub fn socket_closed(&mut self, url: &str, now_ms: i64) -> MuxOut {
        let mut out = MuxOut::default();
        let stopped = self.stopped;
        let Some(l) = self.links.iter_mut().find(|l| l.url == url) else {
            return out;
        };
        l.state = State::Down;
        if !stopped {
            let delay = i64::try_from(socket_backoff_ms(l.attempt)).unwrap_or(i64::MAX);
            l.attempt = l.attempt.saturating_add(1);
            l.retry_at = Some(now_ms + delay);
        }
        out.extend(self.settle_link(url, "Relay connection closed."));
        out
    }

    /// A text frame from a relay.
    pub fn socket_message(&mut self, url: &str, text: &str, now_ms: i64) -> MuxOut {
        let mut out = MuxOut::default();
        let Some(kind) = self.links.iter().find(|l| l.url == url).map(|l| l.kind) else {
            return out;
        };
        self.socket_alive(url, now_ms);
        match kind {
            Kind::Nostr => self.nostr_message(text, now_ms / 1000, &mut out),
            Kind::Relay => self.relay_message(url, text, &mut out),
        }
        out
    }

    fn deliver(&mut self, mailbox: &str, blob: SealedBlob, id: String, out: &mut MuxOut) {
        if !self.subs.contains_key(mailbox) {
            return;
        }
        if !self.blobs_seen.first_time(&replay_id(&blob)) {
            return;
        }
        out.events.push(MuxEvent::Blob {
            mailbox: mailbox.to_owned(),
            blob: Box::new(blob),
            id,
        });
    }

    fn nostr_message(&mut self, text: &str, now_secs: i64, out: &mut MuxOut) {
        // Which subscription is it for? ["EVENT", sub_id, event]
        let Ok(Value::Array(frame)) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let Some(sub_id) = frame.get(1).and_then(Value::as_str) else {
            return;
        };
        let Some(mailbox) = self.sub_to_mailbox.get(sub_id).cloned() else {
            return;
        };
        if let Some((blob, id)) =
            accept_relay_message(text, sub_id, &mailbox, &mut self.events_seen, now_secs)
        {
            self.deliver(&mailbox, blob, id, out);
        }
    }

    fn relay_message(&mut self, url: &str, text: &str, out: &mut MuxOut) {
        use relay::Incoming;
        match relay::parse_incoming(text) {
            Some(Incoming::Event {
                id,
                blob,
                ts,
                mailbox,
            }) => {
                let Some(mailbox) = mailbox else { return };
                if let (Some(ts), Some(sub)) = (ts, self.subs.get_mut(&mailbox)) {
                    sub.last_seen = sub.last_seen.max(ts);
                }
                if let Some(blob) = SealedBlob::decode(&blob) {
                    self.deliver(&mailbox, blob, id, out);
                }
            }
            Some(Incoming::Ok { id }) => out.extend(self.put_answer(&id, url, None)),
            Some(Incoming::Err { id, reason }) => {
                out.extend(self.put_answer(&id, url, Some(reason)));
            }
            None => {}
        }
    }

    /// Publish a blob to every open relay. One answer comes back as an event: `PutOk` as soon as any relay took it, or
    /// `PutFailed` (with the first reason) when none did.
    pub fn put(
        &mut self,
        req: PutRequest<'_>,
        now_ms: i64,
        entropy: &mut dyn Entropy,
    ) -> Result<MuxOut, CryptoError> {
        let PutRequest {
            mailbox,
            blob,
            kind,
            ttl_sec,
            put_id,
        } = req;
        let mut out = MuxOut::default();
        let open: Vec<(String, Kind)> = self
            .links
            .iter()
            .filter(|l| l.state == State::Open)
            .map(|l| (l.url.clone(), l.kind))
            .collect();
        if open.is_empty() {
            out.events.push(MuxEvent::PutFailed {
                put_id: put_id.to_owned(),
                reason: "No relay is connected.".into(),
            });
            return Ok(out);
        }
        let mut awaiting = BTreeSet::new();
        let mut accepted = false;
        let mut nostr_frame: Option<String> = None;
        for (url, kind_of_link) in open {
            match kind_of_link {
                Kind::Nostr => {
                    // Nostr has no answer we wait for: taken when the relay was open.
                    let text = match &nostr_frame {
                        Some(t) => t.clone(),
                        None => {
                            let event =
                                make_event(mailbox, blob, kind, ttl_sec, now_ms / 1000, entropy)?;
                            let t = event_frame(&event);
                            nostr_frame = Some(t.clone());
                            t
                        }
                    };
                    out.actions.push(Action::Send { url, text });
                    accepted = true;
                }
                Kind::Relay => {
                    out.actions.push(Action::Send {
                        url: url.clone(),
                        text: relay::put_frame(
                            mailbox,
                            put_id,
                            &blob.encode(),
                            ttl_sec,
                            kind.as_str(),
                        ),
                    });
                    awaiting.insert(url);
                }
            }
        }
        if accepted {
            out.events.push(MuxEvent::PutOk {
                put_id: put_id.to_owned(),
            });
        }
        if !awaiting.is_empty() {
            self.puts.insert(
                put_id.to_owned(),
                Put {
                    awaiting,
                    done: accepted,
                    first_failure: None,
                    deadline: now_ms + PUT_TIMEOUT_MS,
                },
            );
        }
        Ok(out)
    }

    /// One relay answered a put (`reason` is the refusal, if it was one).
    fn put_answer(&mut self, put_id: &str, url: &str, refusal: Option<String>) -> MuxOut {
        let mut out = MuxOut::default();
        let Some(p) = self.puts.get_mut(put_id) else {
            return out;
        };
        if !p.awaiting.remove(url) {
            return out;
        }
        match refusal {
            None if !p.done => {
                p.done = true;
                out.events.push(MuxEvent::PutOk {
                    put_id: put_id.to_owned(),
                });
            }
            Some(reason) => {
                p.first_failure.get_or_insert(reason);
            }
            None => {}
        }
        if p.awaiting.is_empty() {
            let failed = (!p.done).then(|| {
                p.first_failure
                    .clone()
                    .unwrap_or_else(|| "Relay refused the message.".into())
            });
            self.puts.remove(put_id);
            if let Some(reason) = failed {
                out.events.push(MuxEvent::PutFailed {
                    put_id: put_id.to_owned(),
                    reason,
                });
            }
        }
        out
    }

    /// A link went away: the puts that were waiting on it will not be answered.
    fn settle_link(&mut self, url: &str, reason: &str) -> MuxOut {
        let mut out = MuxOut::default();
        let ids: Vec<String> = self
            .puts
            .iter()
            .filter(|(_, p)| p.awaiting.contains(url))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            out.extend(self.put_answer(&id, url, Some(reason.to_owned())));
        }
        out
    }

    /// Tell the relays a stored item was handled (PrivCord relays only; Nostr relays cannot delete on our behalf).
    pub fn remove(&mut self, mailbox: &str, id: &str) -> MuxOut {
        MuxOut {
            actions: self
                .links
                .iter()
                .filter(|l| l.kind == Kind::Relay && l.state == State::Open)
                .map(|l| Action::Send {
                    url: l.url.clone(),
                    text: relay::del_frame(mailbox, id),
                })
                .collect(),
            events: vec![],
        }
    }

    /// The machine woke from sleep, or the network changed: every open link is suspect, so replace them all now instead
    /// of waiting out the silence.
    pub fn suspect_all(&mut self, now_ms: i64) -> MuxOut {
        let mut out = MuxOut::default();
        let urls: Vec<String> = self
            .links
            .iter()
            .filter(|l| l.state != State::Down)
            .map(|l| l.url.clone())
            .collect();
        for url in urls {
            out.actions.push(Action::Close { url: url.clone() });
            // Not a failure of the relay: reconnect at once, without growing the backoff.
            if let Some(l) = self.links.iter_mut().find(|l| l.url == url) {
                l.attempt = 0;
            }
            out.extend(self.socket_closed(&url, now_ms));
            if let Some(l) = self.links.iter_mut().find(|l| l.url == url) {
                l.retry_at = Some(now_ms);
            }
        }
        out
    }

    /// Time passes: reconnect what is due, give up on puts nobody answered, ping quiet links and replace silent ones.
    pub fn tick(&mut self, now_ms: i64) -> MuxOut {
        let mut out = MuxOut::default();
        if self.stopped {
            return out;
        }
        // Puts nobody answered.
        let expired: Vec<String> = self
            .puts
            .iter()
            .filter(|(_, p)| p.deadline <= now_ms)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            let urls: Vec<String> = self
                .puts
                .get(&id)
                .map(|p| p.awaiting.iter().cloned().collect())
                .unwrap_or_default();
            for url in urls {
                out.extend(self.put_answer(&id, &url, Some("Relay did not answer.".into())));
            }
        }
        // Links that went quiet, and links to ping.
        let mut dead = Vec::new();
        for l in &mut self.links {
            if l.state != State::Open {
                continue;
            }
            if now_ms - l.last_rx > self.dead_after_ms {
                dead.push(l.url.clone());
            } else if now_ms - l.last_ping >= self.ping_every_ms {
                l.last_ping = now_ms;
                out.actions.push(Action::Ping { url: l.url.clone() });
            }
        }
        for url in dead {
            out.actions.push(Action::Close { url: url.clone() });
            out.extend(self.socket_closed(&url, now_ms));
        }
        out.extend(self.connect_down_links(now_ms));
        out
    }

    /// Close everything (the owner is done).
    pub fn stop(&mut self) -> MuxOut {
        self.stopped = true;
        let mut out = MuxOut::default();
        let urls: Vec<String> = self
            .links
            .iter()
            .filter(|l| l.state != State::Down)
            .map(|l| l.url.clone())
            .collect();
        for url in urls {
            out.actions.push(Action::Close { url });
        }
        for l in &mut self.links {
            l.state = State::Down;
            l.retry_at = None;
        }
        for (id, p) in std::mem::take(&mut self.puts) {
            if !p.done {
                out.events.push(MuxEvent::PutFailed {
                    put_id: id,
                    reason: "Stopped.".into(),
                });
            }
        }
        out
    }
}
