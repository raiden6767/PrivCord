//! Six-word invites, the people who answer them, and removing a friend (`engine.ts` "six-word invites").
//!
//! The engine never opens a socket. It tells the transport which mailbox to listen on and what to publish
//! (`Transport::signal_*`); blobs that arrive come back through `signal_blob`. Time passes through `tick`.

use super::{Core, StorageOp};
use crate::{
    api::{AnswerStatus, EngineEvent},
    error::ErrorCode,
};
use privcord_crypto::{
    card::{Contact, ContactCard},
    invite::{
        derive_invite, new_invite_words, open_invite, parse_invite_words, seal_invite, InviteKind,
        Rendezvous, INVITE_TTL_SEC,
    },
    safety::derive_safety_number,
};
use privcord_domain::{
    blob::SealedBlob,
    privacy::ContactTrust,
    signaling::{PutKind, SeenSet},
    ConversationId, UserId,
};
use privcord_ports::EraseScope;
use std::collections::BTreeMap;

pub(super) const KV_STORE_FORWARD: &str = "settings.store_forward";
pub(super) const KV_NAME: &str = "profile.name";
/// The invite card is published again this often while the invite is open: relays differ in what they keep. Each post
/// shows a relay that someone is waiting at this mailbox, so after the first minute (when a friend is most likely to be
/// answering) it slows down; a search lasts 40 s, so a later one still finds it.
const REPUBLISH_MS: i64 = 4_000;
const REPUBLISH_SLOW_MS: i64 = 12_000;
const REPUBLISH_FAST_FOR_MS: i64 = 60_000;
const LOOKUP_MS: i64 = 40_000;
const ANSWER_LINGER_MS: i64 = 1_500;
const LIVE_TTL_SEC: u32 = 120;
/// After adding someone from their card (a QR code), we ask them to add us back this often until we are connected...
const HELLO_EVERY_MS: i64 = 60_000;
/// After the first ten minutes the request is repeated less often: every quarter hour for six hours, then hourly.
const HELLO_SLOW_EVERY_MS: i64 = 15 * 60_000;
const HELLO_SLOWER_EVERY_MS: i64 = 60 * 60_000;
/// A copy of the request the relay keeps for a week, left now and then (a friend who is away from their phone, or asleep, or
/// on the other side of the world gets it whenever they next open the app).
const HELLO_KEPT_TTL_SEC: u32 = 7 * 24 * 3600;
const HELLO_KEPT_EVERY_MS: i64 = 12 * 3600 * 1000;
/// ...for at most this long (they may be away for days; on their side it waits as a request).
const HELLO_FOR_MS: i64 = 30 * 24 * 3600 * 1000;
/// Requests kept at once: someone who saw our QR code cannot fill the screen with them.
const MAX_REQUESTS: usize = 20;
const KV_PENDING: &str = "invites.pending.v1";

struct Hello {
    next_at: i64,
    until: i64,
    /// When a stored copy of the request was last left on the relay (0: never). One now and then, not one a minute: they are
    /// for a phone that was away, and must not pile up in its mailbox.
    stored_at: i64,
}

struct ActiveInvite {
    words: String,
    rv: Rendezvous,
    started_at: i64,
    expires_at: i64,
    next_publish_at: i64,
    /// A stored copy of the card is on the relay (so a friend who types the words after the app was left still finds it).
    stored: bool,
}

struct Lookup {
    words: String,
    rv: Rendezvous,
    deadline: i64,
}

struct Redeemed {
    words: String,
    rv: Rendezvous,
    contact: Contact,
}

struct Answer {
    uid: UserId,
    rv: Rendezvous,
    status: AnswerStatus,
    /// Stop listening on the mailbox at this time.
    unsubscribe_at: Option<i64>,
}

pub(super) struct Request {
    pub contact: Contact,
    pub safety_number: String,
}

#[derive(Default)]
pub(super) struct InviteState {
    active: Option<ActiveInvite>,
    lookup: Option<Lookup>,
    redeemed: Option<Redeemed>,
    answer: Option<Answer>,
    pub(super) requests: BTreeMap<UserId, Request>,
    hellos: BTreeMap<UserId, Hello>,
    seen: SeenSet,
    /// `requests` and `hellos` are kept across restarts (see `persist_pending`).
    pending_loaded: bool,
    pending_saved: Vec<u8>,
}

/// What a person sees before saving a friend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitePreview {
    pub uid: UserId,
    pub name: String,
    pub safety_number: String,
    /// They are already a verified friend.
    pub already: bool,
}

fn short(uid: &UserId) -> String {
    uid.as_str().chars().take(8).collect()
}

impl Core {
    /// We added this person from their card and they have not added us back yet (the request is still being sent).
    pub(super) fn awaiting_acceptance(&self, person: &UserId) -> bool {
        self.invites.hellos.contains_key(person)
    }

    /// Messages (and setup) wait on the relay, sealed, for a friend who is offline. On unless the person turned it off:
    /// without it a message reaches a friend only while both phones are online and able to open a direct link, which is a
    /// gamble; with it, delivery needs only a relay that both can reach, whenever each opens the app.
    pub(super) fn store_forward(&self) -> bool {
        !matches!(self.ports.storage.kv(KV_STORE_FORWARD), Ok(Some(v)) if v == b"0")
    }

    /// With this on, invites are also kept on the relay for 15 minutes; with it off nothing may be stored there.
    pub fn set_store_forward(&mut self, value: bool) -> Result<(), ErrorCode> {
        self.require_live()?;
        self.ports.storage.apply(&[StorageOp::SetKv {
            key: KV_STORE_FORWARD.into(),
            value: vec![u8::from(value) + b'0'],
        }])?;
        Ok(())
    }

    /// The name put on invite cards.
    pub fn set_display_name(&mut self, name: &str) -> Result<(), ErrorCode> {
        self.require_live()?;
        let name: String = privcord_domain::text::js_trim(name)
            .chars()
            .take(40)
            .collect();
        self.ports.storage.apply(&[StorageOp::SetKv {
            key: KV_NAME.into(),
            value: name.clone().into_bytes(),
        }])?;
        self.display_name = Some(name).filter(|n| !n.is_empty());
        Ok(())
    }

    fn preview_of(&self, contact: &Contact) -> Result<InvitePreview, ErrorCode> {
        if Some(&contact.uid) == self.uid.as_ref() {
            return Err(ErrorCode::OwnInvite);
        }
        let own = self.identity()?;
        Ok(InvitePreview {
            uid: contact.uid.clone(),
            name: contact
                .card
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("Friend {}", short(&contact.uid))),
            safety_number: derive_safety_number(&own.sign_public(), &contact.card.sign)?,
            already: self
                .contacts
                .get(&contact.uid)
                .is_some_and(|c| c.stored.trust == ContactTrust::Verified),
        })
    }

    /* ---------------------------------------------------------------- show your words */

    /// Publish our card under fresh words and listen for people who answer. Returns the words and when they expire.
    pub fn create_invite(&mut self) -> Result<(String, i64), ErrorCode> {
        self.require_live()?;
        let now = self.now();
        if let Some(a) = &self.invites.active {
            if a.expires_at > now + 30_000 {
                return Ok((a.words.clone(), a.expires_at));
            }
        }
        self.stop_invite();
        if !self.ports.transport.signal_available() {
            return Err(ErrorCode::NoSignaling);
        }
        let words = new_invite_words(&mut *self.ports.entropy)?;
        let rv = derive_invite(&words)?;
        self.ports
            .transport
            .signal_subscribe(&rv.mailbox)
            .map_err(|_| ErrorCode::NoSignaling)?;
        let text = words.join(" ");
        let expires_at = now + i64::try_from(INVITE_TTL_SEC).unwrap_or(900) * 1000;
        self.invites.active = Some(ActiveInvite {
            words: text.clone(),
            rv,
            started_at: now,
            expires_at,
            next_publish_at: now + REPUBLISH_MS,
            stored: false,
        });
        if self.publish_invite().is_err() {
            self.stop_invite();
            return Err(ErrorCode::NoSignaling);
        }
        self.hub.publish(EngineEvent::InviteChanged);
        Ok((text, expires_at))
    }

    /// Sends a connection-setup message: live to whoever is listening, and, when `keep` (or the person allows messages to
    /// wait), a copy the relay keeps for 15 minutes for a phone that is not listening right now (a friend who left the app to
    /// send the words, a phone with no signal for a minute). Setup messages are sealed (the card is under a key from the words,
    /// the request to the friend's own key) and short-lived, so the relay learns nothing from holding one.
    fn put_everywhere(
        &self,
        mailbox: &str,
        blob: &SealedBlob,
        keep: Option<u32>,
    ) -> Result<(), ErrorCode> {
        let text = blob.encode();
        let live = self
            .ports
            .transport
            .signal_put(mailbox, &text, PutKind::Sig, LIVE_TTL_SEC);
        let stored = keep.map(|ttl| {
            self.ports
                .transport
                .signal_put(mailbox, &text, PutKind::Msg, ttl)
        });
        if live.is_ok() || stored.is_some_and(|s| s.is_ok()) {
            Ok(())
        } else {
            Err(ErrorCode::NoSignaling)
        }
    }

    fn publish_invite(&mut self) -> Result<(), ErrorCode> {
        if self.invites.active.is_none() {
            return Ok(());
        }
        let card = self.own_contact_card(self.display_name.as_deref())?;
        let author = Self::author_of(
            self.uid.as_ref(),
            self.identity.as_ref(),
            self.sync.as_ref(),
            self.device_list.as_ref(),
        )?;
        let Some(a) = &self.invites.active else {
            return Ok(());
        };
        let first = !a.stored;
        let blob = seal_invite(
            &a.rv,
            InviteKind::Card,
            &author,
            &card,
            &mut *self.ports.entropy,
        )?;
        let mailbox = a.rv.mailbox.clone();
        let sent = self.put_everywhere(
            &mailbox,
            &blob,
            (first || self.store_forward()).then(|| u32::try_from(INVITE_TTL_SEC).unwrap_or(900)),
        );
        if first && sent.is_ok() {
            if let Some(a) = self.invites.active.as_mut() {
                a.stored = true;
            }
        }
        sent
    }

    pub fn stop_invite(&mut self) {
        if let Some(a) = self.invites.active.take() {
            self.ports.transport.signal_unsubscribe(&a.rv.mailbox);
            self.hub.publish(EngineEvent::InviteChanged);
        }
    }

    /* ---------------------------------------------------------------- enter their words (or paste a card) */

    /// Look at an invite without saving anything. A full card answers at once; six words start a search that ends with
    /// `InviteFound` or `InviteLookupFailed` (15 seconds at most).
    pub fn preview_invite(&mut self, text: &str) -> Result<Option<InvitePreview>, ErrorCode> {
        self.require_live()?;
        let Some(words) = parse_invite_words(text) else {
            let contact = ContactCard::decode(text).ok_or(ErrorCode::InvalidInvite)?;
            return self.preview_of(&contact).map(Some);
        };
        let key = words.join(" ");
        if let Some(r) = self.invites.redeemed.as_ref().filter(|r| r.words == key) {
            return self.preview_of(&r.contact).map(Some);
        }
        if !self.ports.transport.signal_available() {
            return Err(ErrorCode::NoSignaling);
        }
        self.cancel_lookup();
        let rv = derive_invite(&words)?;
        self.ports
            .transport
            .signal_subscribe(&rv.mailbox)
            .map_err(|_| ErrorCode::NoSignaling)?;
        let deadline = self.now() + LOOKUP_MS;
        self.invites.lookup = Some(Lookup {
            words: key,
            rv,
            deadline,
        });
        Ok(None)
    }

    fn cancel_lookup(&mut self) {
        if let Some(l) = self.invites.lookup.take() {
            self.ports.transport.signal_unsubscribe(&l.rv.mailbox);
        }
    }

    /// Save a friend from their invite and return who they are. After typing their words we also answer with our own
    /// card, so they can accept us on their side.
    pub fn accept_invite(&mut self, text: &str) -> Result<UserId, ErrorCode> {
        self.require_live()?;
        let (contact, answer_with) = match parse_invite_words(text) {
            Some(words) => {
                let key = words.join(" ");
                match self.invites.redeemed.take() {
                    Some(r) if r.words == key => (r.contact, Some(r.rv)),
                    other => {
                        self.invites.redeemed = other;
                        return Err(ErrorCode::InviteNotFound);
                    }
                }
            }
            None => (
                ContactCard::decode(text).ok_or(ErrorCode::InvalidInvite)?,
                None,
            ),
        };
        if Some(&contact.uid) == self.uid.as_ref() {
            return Err(ErrorCode::OwnInvite);
        }
        let text = contact.card.encode()?;
        let uid = self.add_contact_from(&text, ContactTrust::Verified, None)?;
        self.invites.requests.remove(&uid);
        if let Some(rv) = answer_with {
            self.run_answer(uid.clone(), rv);
        }
        // They do not know us yet. Ask them to add us back, so they only have to accept: from a QR code that is the only
        // way, and with six words it is the way when they missed the answer (they left the app to send the words, or
        // closed the invite) - the answer is only heard by an app that is open at that moment.
        let now = self.now();
        self.invites.hellos.insert(
            uid.clone(),
            Hello {
                next_at: now,
                until: now + HELLO_FOR_MS,
                stored_at: 0,
            },
        );
        self.send_hellos();
        Ok(uid)
    }

    /// People who asked to be added (they wait for a tap) and people we asked to add us back (we keep asking for half an
    /// hour) survive the app being closed: otherwise a request is lost with the process, and with relays handing a week
    /// of messages over again the same blob is rightly not handled twice.
    fn persist_pending(&mut self) {
        if !self.invites.pending_loaded {
            let Some(identity) = self.identity.as_ref() else {
                return;
            };
            self.invites.pending_loaded = true;
            let saved: serde_json::Value = self
                .ports
                .storage
                .kv(KV_PENDING)
                .ok()
                .flatten()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            let now = self.now();
            let mut woke = Vec::new();
            for (uid, card) in saved
                .get("requests")
                .and_then(serde_json::Value::as_object)
                .into_iter()
                .flatten()
            {
                let (Some(text), Ok(uid)) = (card.as_str(), UserId::new(uid)) else {
                    continue;
                };
                let Some(contact) = ContactCard::decode(text) else {
                    continue;
                };
                let Ok(safety_number) =
                    derive_safety_number(&identity.sign_public(), &contact.card.sign)
                else {
                    continue;
                };
                if contact.uid == uid && !self.contacts.contains_key(&uid) {
                    self.invites.requests.insert(
                        uid.clone(),
                        Request {
                            contact,
                            safety_number,
                        },
                    );
                    woke.push(uid);
                }
            }
            for (uid, until) in saved
                .get("hellos")
                .and_then(serde_json::Value::as_object)
                .into_iter()
                .flatten()
            {
                let (Some(until), Ok(uid)) = (until.as_i64(), UserId::new(uid)) else {
                    continue;
                };
                if until > now {
                    self.invites.hellos.insert(
                        uid,
                        Hello {
                            next_at: now,
                            until,
                            stored_at: 0,
                        },
                    );
                }
            }
            for uid in woke {
                self.hub.publish(EngineEvent::InviteRequest { uid });
            }
        }
        let requests: serde_json::Map<String, serde_json::Value> = self
            .invites
            .requests
            .iter()
            .filter_map(|(u, r)| {
                Some((
                    u.to_string(),
                    serde_json::json!(r.contact.card.encode().ok()?),
                ))
            })
            .collect();
        let hellos: serde_json::Map<String, serde_json::Value> = self
            .invites
            .hellos
            .iter()
            .map(|(u, h)| (u.to_string(), serde_json::json!(h.until)))
            .collect();
        let bytes =
            serde_json::to_vec(&serde_json::json!({ "requests": requests, "hellos": hellos }))
                .unwrap_or_default();
        if bytes != self.invites.pending_saved {
            self.invites.pending_saved = bytes.clone();
            let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                key: KV_PENDING.into(),
                value: bytes,
            }]);
        }
    }

    /// Sends the "please add me back" requests that are due, until the link is up or the time runs out.
    fn send_hellos(&mut self) {
        let now = self.now();
        let due: Vec<UserId> = self
            .invites
            .hellos
            .iter()
            .filter(|(_, h)| h.next_at <= now)
            .map(|(u, _)| u.clone())
            .collect();
        for uid in due {
            let linked = self.person_ready(&uid);
            let expired = self.invites.hellos.get(&uid).is_none_or(|h| h.until <= now);
            if linked || expired || !self.contacts.contains_key(&uid) {
                self.invites.hellos.remove(&uid);
                continue;
            }
            let mut keep = None;
            if let Some(h) = self.invites.hellos.get_mut(&uid) {
                let age = now - (h.until - HELLO_FOR_MS);
                h.next_at = now
                    + if age < 10 * 60_000 {
                        HELLO_EVERY_MS
                    } else if age < 6 * 3600 * 1000 {
                        HELLO_SLOW_EVERY_MS
                    } else {
                        HELLO_SLOWER_EVERY_MS
                    };
                if h.stored_at == 0 || now - h.stored_at >= HELLO_KEPT_EVERY_MS {
                    h.stored_at = now;
                    keep = Some(HELLO_KEPT_TTL_SEC);
                }
            }
            let Ok(card) = self.own_card(self.display_name.clone().as_deref()) else {
                continue;
            };
            let payload = serde_json::json!({ "t": "hello", "card": card });
            self.diag("invite", "asking to be added back", uid.as_str());
            if let Ok(blob) = self.seal_to(&uid, &payload) {
                // A failed send is tried again on the next round.
                let _ = self.put_everywhere(
                    &privcord_crypto::seal::mailbox_for(uid.as_str()),
                    &blob,
                    keep,
                );
            }
        }
    }

    /// Someone added us from our card and asks to be added back: it waits as a request, like someone who used our
    /// six words. Only a blob signed by the identity on the card inside it gets this far (`open_from_stranger`).
    pub(super) fn on_friend_request(&mut self, blob: &SealedBlob) {
        let (Some(own_uid), Some(identity)) = (self.uid.clone(), self.identity.as_ref()) else {
            return;
        };
        let Some((contact, opened)) =
            privcord_crypto::seal::open_from_stranger(blob, &own_uid, identity)
        else {
            self.diag(
                "invite",
                "request refused: not readable or not signed by the card inside",
                blob.from.as_str(),
            );
            return;
        };
        let payload = opened.payload;
        if payload.get("t").and_then(serde_json::Value::as_str) != Some("hello")
            || contact.uid == own_uid
        {
            self.diag(
                "invite",
                "request refused: not a hello",
                contact.uid.as_str(),
            );
            return;
        }
        if self.invites.requests.len() >= MAX_REQUESTS
            && !self.invites.requests.contains_key(&contact.uid)
        {
            self.diag(
                "invite",
                "request refused: too many waiting",
                contact.uid.as_str(),
            );
            return;
        }
        let Ok(safety_number) = derive_safety_number(&identity.sign_public(), &contact.card.sign)
        else {
            return;
        };
        let uid = contact.uid.clone();
        let new = !self.invites.requests.contains_key(&uid);
        self.invites.requests.insert(
            uid.clone(),
            Request {
                contact,
                safety_number,
            },
        );
        self.diag(
            "invite",
            if new {
                "someone asked to be added"
            } else {
                "asked again to be added"
            },
            uid.as_str(),
        );
        if new {
            self.hub.publish(EngineEvent::InviteRequest { uid });
        }
    }

    fn run_answer(&mut self, uid: UserId, rv: Rendezvous) {
        let _ = self.ports.transport.signal_subscribe(&rv.mailbox);
        let status = match self.send_reply(&rv) {
            Ok(()) => AnswerStatus::Sent,
            Err(_) => AnswerStatus::Failed,
        };
        let unsubscribe_at = Some(self.now() + ANSWER_LINGER_MS);
        self.invites.answer = Some(Answer {
            uid,
            rv,
            status,
            unsubscribe_at,
        });
        self.hub.publish(EngineEvent::AnswerChanged);
    }

    fn send_reply(&mut self, rv: &Rendezvous) -> Result<(), ErrorCode> {
        let card = self.own_contact_card(self.display_name.as_deref())?;
        let author = Self::author_of(
            self.uid.as_ref(),
            self.identity.as_ref(),
            self.sync.as_ref(),
            self.device_list.as_ref(),
        )?;
        let blob = seal_invite(
            rv,
            InviteKind::Reply,
            &author,
            &card,
            &mut *self.ports.entropy,
        )?;
        self.put_everywhere(
            &rv.mailbox,
            &blob,
            Some(u32::try_from(INVITE_TTL_SEC).unwrap_or(900)),
        )
    }

    /// Send our answer again after it failed.
    pub fn retry_answer(&mut self) -> Result<(), ErrorCode> {
        self.require_live()?;
        let Some(a) = self.invites.answer.take() else {
            return Ok(());
        };
        self.run_answer(a.uid, a.rv);
        Ok(())
    }

    pub fn clear_answer(&mut self) {
        if let Some(a) = self.invites.answer.take() {
            self.ports.transport.signal_unsubscribe(&a.rv.mailbox);
            self.hub.publish(EngineEvent::AnswerChanged);
        }
    }

    /* ---------------------------------------------------------------- people who used our words */

    /// Accept somebody who used our words.
    pub fn accept_request(&mut self, uid: &UserId) -> Result<UserId, ErrorCode> {
        self.require_live()?;
        let r = self
            .invites
            .requests
            .remove(uid)
            .ok_or(ErrorCode::InviteNotFound)?;
        let text = r.contact.card.encode()?;
        self.add_contact_from(&text, ContactTrust::Verified, None)
    }

    pub fn dismiss_request(&mut self, uid: &UserId) {
        if self.invites.requests.remove(uid).is_some() {
            self.hub.publish(EngineEvent::InviteChanged);
        }
    }

    /* ---------------------------------------------------------------- blobs from the signaling service */

    /// A blob arrived on a mailbox we listen on. The first copy wins when several services deliver it.
    pub fn signal_blob(&mut self, mailbox: &str, encoded: &str) {
        if self.shut_down {
            return;
        }
        let Some(blob) = SealedBlob::decode(encoded) else {
            return;
        };
        if !self
            .invites
            .seen
            .first_time(&privcord_crypto::seal::replay_id(&blob))
        {
            return;
        }
        // An anonymous envelope: only on our own mailboxes, opened with one of our standing keys; what is inside goes
        // through the same checks as any other message.
        if blob.v == 3 {
            if self.own_mailboxes().iter().any(|m| m == mailbox) {
                match self.open_anon_blob(&blob) {
                    Some(inner) => self.signal_blob(mailbox, &inner.encode()),
                    None => self.diag(
                        "relay",
                        "an envelope on our mailbox could not be opened",
                        "",
                    ),
                }
            } else {
                self.diag(
                    "relay",
                    "an envelope came on a mailbox that is not ours",
                    "",
                );
            }
            return;
        }
        // Linking a device: the new device has no account yet, so this comes first.
        if self.is_link_mailbox(mailbox) {
            self.link_blob(&blob);
            return;
        }
        // Another device of this account (a linked device has no account key, so this comes before what needs it).
        if self.is_self_mailbox(mailbox) {
            self.sync_blob(mailbox, &blob);
            return;
        }
        // A piece of a file coming from another device of this account.
        if self.is_media_mailbox(mailbox) {
            self.media_blob(mailbox, &blob);
            return;
        }
        let Ok(value) = serde_json::to_value(&blob) else {
            return;
        };
        // The account's public key (for safety numbers): every device of the account has it.
        let Some(own) = self.account.as_ref().map(|(sign, _)| sign.clone()) else {
            return;
        };
        // A message in a public channel we joined, carried by the signaling service.
        if self.channels.values().any(|k| k.mailbox == mailbox) {
            self.accept_channel_blob(&blob, None);
            return;
        }
        // Someone answered our words.
        if let Some(a) = self
            .invites
            .active
            .as_ref()
            .filter(|a| a.rv.mailbox == mailbox)
        {
            let Some(m) = open_invite(&a.rv, &value).filter(|m| m.kind == InviteKind::Reply) else {
                return;
            };
            let contact = m.contact;
            let already_friend = self
                .contacts
                .get(&contact.uid)
                .is_some_and(|c| c.stored.trust == ContactTrust::Verified);
            if Some(&contact.uid) == self.uid.as_ref() || already_friend {
                return;
            }
            // Anyone holding the words can answer under any number of fresh identities: same cap as card requests.
            if self.invites.requests.len() >= MAX_REQUESTS
                && !self.invites.requests.contains_key(&contact.uid)
            {
                return;
            }
            let Ok(safety_number) = derive_safety_number(&own, &contact.card.sign) else {
                return;
            };
            let uid = contact.uid.clone();
            self.invites.requests.insert(
                uid.clone(),
                Request {
                    contact,
                    safety_number,
                },
            );
            self.hub.publish(EngineEvent::InviteRequest { uid });
            return;
        }
        // Something sealed for us, on our own mailbox.
        if self.own_mailboxes().iter().any(|m| m == mailbox) {
            self.handle_own_blob(blob);
            return;
        }
        // The card behind words we typed.
        let found = self
            .invites
            .lookup
            .as_ref()
            .filter(|l| l.rv.mailbox == mailbox)
            .and_then(|l| open_invite(&l.rv, &value))
            .filter(|m| m.kind == InviteKind::Card);
        if let Some(m) = found {
            if let Some(l) = self.invites.lookup.take() {
                self.ports.transport.signal_unsubscribe(&l.rv.mailbox);
                let preview = self.preview_of(&m.contact);
                self.invites.redeemed = Some(Redeemed {
                    words: l.words,
                    rv: l.rv,
                    contact: m.contact,
                });
                match preview {
                    Ok(p) => self.hub.publish(EngineEvent::InviteFound {
                        uid: p.uid,
                        name: p.name,
                        safety_number: p.safety_number,
                        already: p.already,
                    }),
                    // Their own card (words typed by the person who made them).
                    Err(_) => {
                        self.invites.redeemed = None;
                        self.hub.publish(EngineEvent::InviteLookupFailed);
                    }
                }
            }
        }
    }

    /// Time passes: republish the open invite, expire it, give up on a search, stop listening after an answer.
    pub(super) fn tick_invites(&mut self) {
        self.persist_pending();
        self.send_hellos();
        let now = self.now();
        let (expired, due) = self.invites.active.as_ref().map_or((false, false), |a| {
            (a.expires_at <= now, a.next_publish_at <= now)
        });
        if expired {
            self.stop_invite();
        } else if due {
            if let Some(a) = self.invites.active.as_mut() {
                let every = if now - a.started_at < REPUBLISH_FAST_FOR_MS {
                    REPUBLISH_MS
                } else {
                    REPUBLISH_SLOW_MS
                };
                a.next_publish_at = now + every;
            }
            let _ = self.publish_invite(); // a failed republish is retried on the next round
        }
        if self
            .invites
            .lookup
            .as_ref()
            .is_some_and(|l| l.deadline <= now)
        {
            self.cancel_lookup();
            self.hub.publish(EngineEvent::InviteLookupFailed);
        }
        if let Some(a) = self.invites.answer.as_mut() {
            if a.unsubscribe_at.is_some_and(|t| t <= now) {
                a.unsubscribe_at = None;
                let mailbox = a.rv.mailbox.clone();
                self.ports.transport.signal_unsubscribe(&mailbox);
            }
        }
    }

    pub(super) fn invite_views(
        &self,
    ) -> (
        Option<crate::api::InviteView>,
        Vec<crate::api::RequestView>,
        Option<crate::api::AnswerView>,
        bool,
    ) {
        let invite = self
            .invites
            .active
            .as_ref()
            .map(|a| crate::api::InviteView {
                words: a.words.clone(),
                expires_at: a.expires_at,
            });
        let requests = self
            .invites
            .requests
            .iter()
            .map(|(uid, r)| crate::api::RequestView {
                uid: uid.clone(),
                name: r
                    .contact
                    .card
                    .name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("Friend {}", short(uid))),
                safety_number: r.safety_number.clone(),
            })
            .collect();
        let answer = self
            .invites
            .answer
            .as_ref()
            .map(|a| crate::api::AnswerView {
                uid: a.uid.clone(),
                status: a.status,
            });
        (invite, requests, answer, self.invites.lookup.is_some())
    }

    /* ---------------------------------------------------------------- forgetting a friend */

    /// Forget a friend and their chat. Refused while they share a group with us, because the group would bring them
    /// straight back.
    /// Everything else kept about a person besides their messages and keys: a person removed and added again must come
    /// back as a stranger would, not muted, pinned or half-introduced (and nothing about them may stay on the phone).
    fn forget_person_state(&mut self, uid: &UserId) {
        self.invites.hellos.remove(uid);
        self.forget_aware(uid);
        if self.muted.remove(uid) {
            let list: Vec<&str> = self.muted.iter().map(UserId::as_str).collect();
            let _ = self.ports.storage.apply(&[StorageOp::SetKv {
                key: super::social::KV_MUTED.into(),
                value: serde_json::to_vec(&list).unwrap_or_default(),
            }]);
        }
        self.forget_chat_prefs(uid.as_str());
        let mut all = self.scheduled();
        let before = all.len();
        all.retain(|s| s.group || s.target != uid.as_str());
        if all.len() != before {
            let _ = self.save_scheduled_list(&all);
        }
    }

    /// Starts the connection with a friend over, for a friendship that stays stuck: the keys exchanged with them are forgotten
    /// (they are given again on the next link or through the relay), any link is dropped, they are asked once more to add us
    /// back (a no-op for a friend who already did), and the next tick dials at once. Nothing else about them changes.
    pub fn repair_contact(&mut self, uid: &UserId) -> Result<(), ErrorCode> {
        self.require_live()?;
        if !self.contacts.contains_key(uid) {
            return Err(ErrorCode::UnknownContact);
        }
        let addrs: Vec<UserId> = self
            .peers
            .keys()
            .filter(|a| &Self::person_of(a) == uid)
            .cloned()
            .collect();
        for a in &addrs {
            self.link_down(a);
            self.ports.transport.close(a);
        }
        self.forget_keys_of_person(uid);
        for p in self.peers.values_mut() {
            p.attempts = 0;
            p.last_attempt_at = 0;
        }
        let now = self.now();
        self.invites.hellos.insert(
            uid.clone(),
            Hello {
                next_at: now,
                until: now + HELLO_FOR_MS,
                stored_at: 0,
            },
        );
        self.send_hellos();
        self.diag("link", "connection reset by the person", uid.as_str());
        Ok(())
    }

    pub fn remove_contact(&mut self, uid: &UserId) -> Result<(), ErrorCode> {
        self.require_live()?;
        if !self.contacts.contains_key(uid) {
            return Err(ErrorCode::UnknownContact);
        }
        if self.groups.as_ref().is_some_and(|g| {
            g.my_groups()
                .iter()
                .any(|s| s.members.contains_key(uid.as_str()))
        }) {
            return Err(ErrorCode::StillInGroup);
        }
        self.ports.storage.apply(&[
            StorageOp::Erase(EraseScope::Conversation(ConversationId::Direct(
                uid.clone(),
            ))),
            StorageOp::DeleteContact(uid.clone()),
        ])?;
        self.forget_contact_devices(uid);
        self.forget_keys_of_person(uid);
        self.forget_person_state(uid);
        // They know our private inbox: move to a new one and tell everyone else.
        self.rotate_inbox();
        let addrs: Vec<UserId> = self
            .peers
            .keys()
            .filter(|a| &Self::person_of(a) == uid)
            .cloned()
            .collect();
        for a in addrs {
            self.link_down(&a);
            self.ports.transport.close(&a);
            self.peers.remove(&a);
        }
        self.contacts.remove(uid);
        self.messages
            .retain(|m| m.conversation != ConversationId::Direct(uid.clone()));
        self.outbox
            .retain(|o| self.messages.contains_key(&o.message_id));
        self.invites.requests.remove(uid);
        self.hub
            .publish(EngineEvent::ContactRemoved { uid: uid.clone() });
        Ok(())
    }
}
