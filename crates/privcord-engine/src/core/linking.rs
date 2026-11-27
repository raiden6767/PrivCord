//! Linking a new device by QR code (docs/design/syncengine.md §2), over the relays.
//!
//! The device that holds the account key shows a code ([`Core::start_device_link`]); the new device, which has no
//! account yet, takes it ([`Core::join_device_link`]). They exchange the messages of `privcord_crypto::devices::link`
//! through a relay mailbox only the holders of the code can compute, and both screens show the same 6-digit code
//! ([`EngineEvent::DeviceLink`], stage "code"). The person confirms on the first device
//! ([`Core::confirm_device_link`]); the new device then receives the account's **public** keys, its signed contact card
//! and the device list with itself in it. The account's private keys never leave the first device.
//!
//! The new device then asks the others for the sync key (its device inbox, `core/sync.rs`) and catches up on the
//! account's history. It talks to friends and changes groups as itself (docs/design/syncengine.md §4); linking another
//! device needs the account key and answers [`ErrorCode::NeedsAccountKey`].
//!
//! There are two directions. In the first the device that holds the account key shows the code and the new device
//! takes it (typed, or scanned where there is a camera). In the second, which is what a laptop with no camera needs, the
//! **new device shows the code** ([`Core::start_scan_link_offer`]) and the device that holds the account key scans it
//! ([`Core::host_scanned_link`]): nothing is typed and nothing travels through another app. The exchange is the same
//! (the secret is only a meeting point and a key for the relay mailbox), with two additions that the new direction
//! needs because the new device can no longer tell the real first device from someone who photographed its code:
//!
//! * the new device does not show its six digits. The person types the six digits the **first device** shows
//!   ([`Core::submit_link_code`]); a grant that arrives earlier is held until they match, and three wrong tries end the
//!   link. Someone who only has the photographed code never sees the first device's digits;
//! * the first device answers the first request it sees and no other (a second request with different keys is
//!   ignored; the same request again gets the same answer back, so a lost answer does not strand the link).
//!
//! The first device still grants only when the person taps Link on it.
//!
//! An offer lasts ten minutes and serves one new device.

use super::{Core, StorageOp};
use crate::{api::EngineEvent, error::ErrorCode};
use privcord_crypto::{
    devices::{
        link::{self, Host, Joiner},
        DeviceEntry, DeviceList,
    },
    identity::Identity,
    kdf::sha256_hex,
};
use privcord_domain::{
    blob::{PublicJwk, SealedBlob},
    signaling::PutKind,
};
use serde_json::json;

const LINK_TTL_MS: i64 = 10 * 60 * 1000;
const LINK_TTL_SEC: u32 = 10 * 60;

/// The new device shows the code and waits for the six digits from the first device (the scan direction).
pub(super) struct ScanJoin {
    /// The request as sent: posted again until an answer comes (the first device may scan it minutes later).
    request: String,
    posted_at: i64,
    /// The six digits this side computed; they are never shown, only compared with what the person types.
    expected: Option<String>,
    matched: bool,
    tries: u8,
    /// A grant that arrived before the digits matched.
    held_grant: Option<String>,
    /// The person types the digits (the scan direction). When false (the code was typed or scanned here), this device shows
    /// the digits and the person confirms they match the first device's: either way the grant waits for that.
    typed: bool,
}

pub(super) enum LinkState {
    Hosting {
        host: Host,
        mailbox: String,
        device: Option<DeviceEntry>,
        code_shown: bool,
        until: i64,
        /// The one request this offer answers, and the answer given: `(request, answer)`.
        answered: Option<(String, String)>,
    },
    Joining {
        joiner: Joiner,
        mailbox: String,
        device: Identity,
        code_shown: bool,
        until: i64,
        scan: Option<ScanJoin>,
    },
}

impl std::fmt::Debug for LinkState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Hosting { .. } => "LinkState::Hosting",
            Self::Joining { .. } => "LinkState::Joining",
        })
    }
}

/// Which message of the exchange a blob carries (it travels in the blob's `from`, its text in `ct`).
const STEP_REQUEST: &str = "link-request";
const STEP_ANSWER: &str = "link-answer";
const STEP_REVEAL: &str = "link-reveal";
const STEP_GRANT: &str = "link-grant";
/// How long the new device waits for an answer before it posts its request again (a relay may have dropped it).
const REPOST_AFTER_MS: i64 = 5_000;
/// Wrong tries at the six digits before the link ends.
const MAX_CODE_TRIES: u8 = 3;

fn wrap(step: &str, message: &str) -> SealedBlob {
    SealedBlob {
        v: 1,
        to: "link".into(),
        from: step.into(),
        // No key exchange of its own: the message is already encrypted under the link code.
        epk: PublicJwk::p256(String::new(), String::new()),
        iv: String::new(),
        ct: message.into(),
        // Relays and hubs tell blobs apart by their signature field.
        sig: sha256_hex(message.as_bytes()),
        kem: None,
        key: None,
    }
}

impl Core {
    fn link_put(&mut self, mailbox: &str, step: &str, message: &str) -> bool {
        self.ports.transport.signal_available()
            && self
                .ports
                .transport
                .signal_put(
                    mailbox,
                    &wrap(step, message).encode(),
                    PutKind::Msg,
                    LINK_TTL_SEC,
                )
                .is_ok()
    }

    /// On the device holding the account key: a code for the new device (shown as a QR code, or typed). Needs a relay.
    pub fn start_device_link(&mut self) -> Result<String, ErrorCode> {
        self.check_can_host()?;
        let (secret, code) = link::new_offer(&mut *self.ports.entropy)?;
        self.begin_hosting(secret)?;
        Ok(code)
    }

    /// On the device holding the account key: the person scanned the code a new device shows. From here it is the same
    /// as an offer made here, except that the new device made the secret and has already posted its request.
    pub fn host_scanned_link(&mut self, code: &str) -> Result<(), ErrorCode> {
        self.check_can_host()?;
        let secret = link::parse_offer(code).ok_or(ErrorCode::InvalidInvite)?;
        self.begin_hosting(secret)
    }

    fn check_can_host(&mut self) -> Result<(), ErrorCode> {
        self.require_live()?;
        self.identity()?;
        if self.device_list.is_none() {
            return Err(ErrorCode::NoIdentity);
        }
        if !self.ports.transport.signal_available() {
            return Err(ErrorCode::NoSignaling);
        }
        Ok(())
    }

    fn begin_hosting(&mut self, secret: link::Secret) -> Result<(), ErrorCode> {
        let mailbox = link::mailbox(&secret);
        let host = Host::new(secret, &mut *self.ports.entropy)?;
        self.ports
            .transport
            .signal_subscribe(&mailbox)
            .map_err(|_| ErrorCode::NoSignaling)?;
        self.link = Some(LinkState::Hosting {
            host,
            mailbox,
            device: None,
            code_shown: false,
            until: self.now() + LINK_TTL_MS,
            answered: None,
        });
        Ok(())
    }

    /// On the new device (no account yet): take the code from the other device.
    pub fn join_device_link(&mut self, code: &str, name: &str) -> Result<(), ErrorCode> {
        self.require_live()?;
        if self.account.is_some() {
            return Err(ErrorCode::IdentityExists);
        }
        let secret = link::parse_offer(code).ok_or(ErrorCode::InvalidInvite)?;
        let mailbox = link::mailbox(&secret);
        let device = Identity::generate(&mut *self.ports.entropy)?;
        let now = self.now();
        let (joiner, request) =
            Joiner::start(secret, &device, name, now, &mut *self.ports.entropy)?;
        self.ports
            .transport
            .signal_subscribe(&mailbox)
            .map_err(|_| ErrorCode::NoSignaling)?;
        if !self.link_put(&mailbox, STEP_REQUEST, &request) {
            return Err(ErrorCode::NoSignaling);
        }
        self.link = Some(LinkState::Joining {
            joiner,
            mailbox,
            device,
            code_shown: false,
            until: now + LINK_TTL_MS,
            // The grant waits until the person confirms the digits match: whoever saw the code could otherwise answer first
            // and send a grant that joins this device to their account.
            scan: Some(ScanJoin {
                request,
                posted_at: now,
                expected: None,
                matched: false,
                tries: 0,
                held_grant: None,
                typed: false,
            }),
        });
        Ok(())
    }

    /// On the new device (no account yet): make the code and show it (as a QR code) for the first device to scan. The
    /// request is posted at once and waits on the relay; the first device reads it when it scans. Returns the code.
    pub fn start_scan_link_offer(&mut self, name: &str) -> Result<String, ErrorCode> {
        self.require_live()?;
        if self.account.is_some() {
            return Err(ErrorCode::IdentityExists);
        }
        if !self.ports.transport.signal_available() {
            return Err(ErrorCode::NoSignaling);
        }
        let (secret, code) = link::new_offer(&mut *self.ports.entropy)?;
        let mailbox = link::mailbox(&secret);
        let device = Identity::generate(&mut *self.ports.entropy)?;
        let now = self.now();
        let (joiner, request) =
            Joiner::start(secret, &device, name, now, &mut *self.ports.entropy)?;
        self.ports
            .transport
            .signal_subscribe(&mailbox)
            .map_err(|_| ErrorCode::NoSignaling)?;
        if !self.link_put(&mailbox, STEP_REQUEST, &request) {
            return Err(ErrorCode::NoSignaling);
        }
        self.link = Some(LinkState::Joining {
            joiner,
            mailbox,
            device,
            code_shown: false,
            until: now + LINK_TTL_MS,
            scan: Some(ScanJoin {
                request,
                posted_at: now,
                expected: None,
                matched: false,
                tries: 0,
                held_grant: None,
                typed: true,
            }),
        });
        Ok(code)
    }

    /// On the new device (scan direction): the six digits the person read off the first device. `Ok(true)` when they
    /// match (a grant that came earlier is taken now), `Ok(false)` when they do not; after three wrong tries the link ends.
    pub fn submit_link_code(&mut self, digits: &str) -> Result<bool, ErrorCode> {
        self.require_live()?;
        let typed: String = digits.chars().filter(char::is_ascii_digit).collect();
        let Some(LinkState::Joining {
            scan: Some(scan),
            mailbox,
            code_shown: true,
            ..
        }) = self.link.as_mut()
        else {
            return Err(ErrorCode::InviteNotFound);
        };
        let Some(expected) = scan.expected.clone() else {
            return Err(ErrorCode::InviteNotFound);
        };
        if scan.matched {
            return Ok(true);
        }
        if !same_digits(&typed, &expected) {
            scan.tries += 1;
            let ended = scan.tries >= MAX_CODE_TRIES;
            let mailbox = mailbox.clone();
            if ended {
                self.end_link(&mailbox, "failed");
            } else {
                self.hub.publish(EngineEvent::DeviceLink {
                    stage: "wrong-code".into(),
                    code: None,
                    name: None,
                });
            }
            return Ok(false);
        }
        scan.matched = true;
        let held = scan.held_grant.take();
        self.hub.publish(EngineEvent::DeviceLink {
            stage: "matched".into(),
            code: None,
            name: None,
        });
        if let Some(grant) = held {
            self.link_blob(&wrap(STEP_GRANT, &grant));
        }
        Ok(true)
    }

    /// On the device holding the account key, once the person compared the codes: let the new device in (or not).
    pub fn confirm_device_link(&mut self, accept: bool) -> Result<(), ErrorCode> {
        self.require_live()?;
        let Some(LinkState::Hosting {
            host,
            mailbox,
            code_shown: true,
            ..
        }) = self.link.take()
        else {
            return Err(ErrorCode::InviteNotFound);
        };
        if !accept {
            self.end_link(&mailbox, "cancelled");
            return Ok(());
        }
        let card = self.own_card(self.display_name.clone().as_deref())?;
        let (Some(account), Some(current)) = (self.identity.as_ref(), self.device_list.as_ref())
        else {
            return Err(ErrorCode::NeedsAccountKey);
        };
        let (list, grant) = host
            .grant(account, current, &card, &mut *self.ports.entropy)
            .map_err(|e| match e {
                privcord_crypto::devices::DeviceError::TooMany => ErrorCode::GroupFull,
                _ => ErrorCode::Crypto,
            })?;
        self.ports.storage.apply(&[StorageOp::SetKv {
            key: super::devices::KV_DEVICE_LIST.into(),
            value: list.to_json().into_bytes(),
        }])?;
        self.device_list = Some(list.clone());
        if let Some(s) = self.sync.as_mut() {
            s.devices_changed(&list);
        }
        let sent = self.link_put(&mailbox, STEP_GRANT, &grant);
        self.end_link(&mailbox, if sent { "done" } else { "failed" });
        Ok(())
    }

    /// Stop linking (either side).
    pub fn cancel_device_link(&mut self) {
        if let Some(state) = self.link.take() {
            let mailbox = match state {
                LinkState::Hosting { mailbox, .. } | LinkState::Joining { mailbox, .. } => mailbox,
            };
            self.end_link(&mailbox, "cancelled");
        }
    }

    fn end_link(&mut self, mailbox: &str, stage: &str) {
        self.ports.transport.signal_unsubscribe(mailbox);
        self.link = None;
        self.hub.publish(EngineEvent::DeviceLink {
            stage: stage.into(),
            code: None,
            name: None,
        });
    }

    pub(super) fn is_link_mailbox(&self, mailbox: &str) -> bool {
        matches!(&self.link, Some(LinkState::Hosting { mailbox: m, .. } | LinkState::Joining { mailbox: m, .. }) if m == mailbox)
    }

    /// Ten minutes after the code was made, it is gone. A new device that shows the code posts its request again every
    /// few seconds until it hears an answer (the first device may scan the code long after the request was posted).
    pub(super) fn tick_link(&mut self) {
        let now = self.now();
        let expired = match &self.link {
            Some(
                LinkState::Hosting { until, mailbox, .. }
                | LinkState::Joining { until, mailbox, .. },
            ) if now > *until => Some(mailbox.clone()),
            _ => None,
        };
        if let Some(m) = expired {
            self.end_link(&m, "expired");
            return;
        }
        let repost = match &self.link {
            Some(LinkState::Joining {
                scan: Some(scan),
                mailbox,
                code_shown: false,
                ..
            }) if now - scan.posted_at >= REPOST_AFTER_MS => {
                Some((mailbox.clone(), scan.request.clone()))
            }
            _ => None,
        };
        if let Some((mailbox, request)) = repost {
            // A failed post is tried again at the next tick.
            if self.link_put(&mailbox, STEP_REQUEST, &request) {
                if let Some(LinkState::Joining {
                    scan: Some(scan), ..
                }) = self.link.as_mut()
                {
                    scan.posted_at = now;
                }
            }
        }
    }

    /// A blob on the link mailbox: the next message of the exchange.
    pub(super) fn link_blob(&mut self, blob: &SealedBlob) {
        if blob.to != "link" {
            return;
        }
        let Some(state) = self.link.take() else {
            return;
        };
        let message = blob.ct.clone();
        let step = blob.from.as_str();
        let mut put: Option<(String, &'static str, String)> = None;
        let mut event: Option<EngineEvent> = None;
        let mut failed: Option<String> = None;
        let mut granted = None;
        let next = match state {
            LinkState::Hosting {
                mut host,
                mailbox,
                device,
                code_shown,
                until,
                mut answered,
            } => {
                match step {
                    STEP_REQUEST if device.is_none() => match &answered {
                        // The one request this offer answers, seen again (the new device posts it until it hears back):
                        // the same answer goes out again, with no new key.
                        Some((request, answer)) if *request == message => {
                            put = Some((mailbox.clone(), STEP_ANSWER, answer.clone()));
                        }
                        // Any other request, once one was answered: ignored.
                        Some(_) => {}
                        None => {
                            // Not a request made with this code: ignored, the real one may still come.
                            if let Ok(answer) = host.on_request(&message, &mut *self.ports.entropy)
                            {
                                put = Some((mailbox.clone(), STEP_ANSWER, answer.clone()));
                                answered = Some((message.clone(), answer));
                            }
                        }
                    },
                    STEP_REVEAL if !code_shown && answered.is_some() => {
                        match host.on_reveal(&message) {
                            Ok((code, entry)) => {
                                event = Some(EngineEvent::DeviceLink {
                                    stage: "code".into(),
                                    code: Some(code),
                                    name: Some(entry.name.clone()),
                                });
                                self.link = Some(LinkState::Hosting {
                                    host,
                                    mailbox: mailbox.clone(),
                                    device: Some(entry),
                                    code_shown: true,
                                    until,
                                    answered,
                                });
                                return self.finish_link_blob(put, event, failed, granted);
                            }
                            Err(_) => {
                                failed = Some(mailbox);
                                self.link = None;
                                return self.finish_link_blob(put, event, failed, granted);
                            }
                        }
                    }
                    _ => {}
                }
                Some(LinkState::Hosting {
                    host,
                    mailbox,
                    device,
                    code_shown,
                    until,
                    answered,
                })
            }
            LinkState::Joining {
                mut joiner,
                mailbox,
                device,
                code_shown,
                until,
                mut scan,
            } => match step {
                STEP_ANSWER if !code_shown => {
                    match joiner.on_answer(&message, &mut *self.ports.entropy) {
                        Ok((reveal, code)) => {
                            put = Some((mailbox.clone(), STEP_REVEAL, reveal));
                            event = Some(match scan.as_mut() {
                                // Scan direction: the digits are not shown here, the person types the first device's.
                                Some(sc) if sc.typed => {
                                    sc.expected = Some(code);
                                    EngineEvent::DeviceLink {
                                        stage: "enter-code".into(),
                                        code: None,
                                        name: None,
                                    }
                                }
                                // Shown here: the person compares them with the first device and confirms by submitting them.
                                other => {
                                    if let Some(sc) = other {
                                        sc.expected = Some(code.clone());
                                    }
                                    EngineEvent::DeviceLink {
                                        stage: "code".into(),
                                        code: Some(code),
                                        name: None,
                                    }
                                }
                            });
                            Some(LinkState::Joining {
                                joiner,
                                mailbox,
                                device,
                                code_shown: true,
                                until,
                                scan,
                            })
                        }
                        Err(_) => Some(LinkState::Joining {
                            joiner,
                            mailbox,
                            device,
                            code_shown,
                            until,
                            scan,
                        }),
                    }
                }
                // Scan direction, digits not typed yet: the grant waits (and a newer one replaces an older one).
                STEP_GRANT if code_shown && scan.as_ref().is_some_and(|s| !s.matched) => {
                    if let Some(sc) = scan.as_mut() {
                        sc.held_grant = Some(message.clone());
                    }
                    Some(LinkState::Joining {
                        joiner,
                        mailbox,
                        device,
                        code_shown,
                        until,
                        scan,
                    })
                }
                STEP_GRANT if code_shown => match joiner.on_grant(&message) {
                    Ok(g) => {
                        granted = Some((g, device, mailbox));
                        None
                    }
                    Err(_) => {
                        failed = Some(mailbox);
                        None
                    }
                },
                _ => Some(LinkState::Joining {
                    joiner,
                    mailbox,
                    device,
                    code_shown,
                    until,
                    scan,
                }),
            },
        };
        self.link = next;
        self.finish_link_blob(put, event, failed, granted);
    }

    /// What a message of the exchange led to: the next message to post, an event, a failure, a grant to adopt.
    #[allow(clippy::type_complexity)]
    fn finish_link_blob(
        &mut self,
        put: Option<(String, &'static str, String)>,
        event: Option<EngineEvent>,
        failed: Option<String>,
        granted: Option<(link::Granted, Identity, String)>,
    ) {
        if let Some((mailbox, step, text)) = put {
            if !self.link_put(&mailbox, step, &text) {
                self.end_link(&mailbox, "failed");
                return;
            }
        }
        if let Some(e) = event {
            self.hub.publish(e);
        }
        if let Some(m) = failed {
            self.end_link(&m, "failed");
        }
        if let Some((g, device, mailbox)) = granted {
            let ok = self
                .adopt_linked(
                    &g.account_sign,
                    &g.account_dh,
                    g.card.as_deref(),
                    &device,
                    &g.list,
                )
                .is_ok();
            self.end_link(&mailbox, if ok { "done" } else { "failed" });
        }
    }

    /// The new device becomes a device of the account: its public keys, its card, this device's keys and the list
    /// that names them. No private account key.
    fn adopt_linked(
        &mut self,
        sign: &PublicJwk,
        dh: &PublicJwk,
        card: Option<&str>,
        device: &Identity,
        list: &DeviceList,
    ) -> Result<(), ErrorCode> {
        let uid = privcord_crypto::identity::uid_from_keys(sign, dh)?;
        let keys = device.export_for_storage();
        let mut both = zeroize::Zeroizing::new(Vec::with_capacity(64));
        both.extend_from_slice(&keys.sign);
        both.extend_from_slice(&keys.dh);
        let mut ops = vec![
            StorageOp::SetKv {
                key: super::devices::KV_ACCOUNT_PUBLIC.into(),
                value: json!({ "sign": sign, "dh": dh }).to_string().into_bytes(),
            },
            StorageOp::SetKv {
                key: format!("{}.{uid}", super::devices::KV_DEVICE_KEYS),
                value: both.to_vec(),
            },
            StorageOp::SetKv {
                key: super::devices::KV_DEVICE_LIST.into(),
                value: list.to_json().into_bytes(),
            },
        ];
        if let Some(card) = card {
            ops.push(StorageOp::SetKv {
                key: super::devices::KV_ACCOUNT_CARD.into(),
                value: card.as_bytes().to_vec(),
            });
        }
        self.ports.storage.apply(&ops)?;
        self.uid = Some(uid);
        self.account = Some((sign.clone(), dh.clone()));
        self.reload_from_storage()?;
        // It joins an account that already has a device: it says hello, and gets the history.
        self.mark_joined();
        self.hub.publish(EngineEvent::IdentityCreated);
        Ok(())
    }
}

/// Two strings of digits are equal (every position is looked at, whatever the first difference).
fn same_digits(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}
