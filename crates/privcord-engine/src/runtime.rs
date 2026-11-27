//! The asynchronous shell around [`Core`].
//!
//! The engine owns exactly one thread with a current-thread Tokio runtime for its whole life (never one per call).
//! Commands arrive on a bounded queue and are handled one at a time, so there is no shared mutable state and no lock
//! around the engine. A timer drives retries. `shutdown` stops accepting commands, ends every session, joins the thread
//! and reports whether it ended cleanly.

use crate::core::InvitePreview;
use crate::{
    api::{MessagePage, Snapshot},
    core::{Core, LinkSecurity, Ports},
    error::ErrorCode,
    events::EventStream,
};
use privcord_domain::{
    killswitch::KillSwitchReport, privacy::ContactTrust, state::ConnectionPath, ConversationId,
    MessageId, UserId,
};
use privcord_ports::StoredFile;
use std::{sync::Mutex, thread::JoinHandle, time::Duration};
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

/// Commands waiting for the engine. A full queue rejects with [`ErrorCode::Busy`] instead of growing without bound.
pub const COMMAND_QUEUE: usize = 256;
const TICK: Duration = Duration::from_secs(1);

type Reply<T> = oneshot::Sender<Result<T, ErrorCode>>;

enum Command {
    CreateIdentity(Reply<Zeroizing<String>>),
    RecoverIdentity {
        phrase: Zeroizing<String>,
        passphrase: Zeroizing<String>,
        reply: Reply<()>,
    },
    OwnCard {
        name: Option<String>,
        reply: Reply<String>,
    },
    AddContact {
        card: String,
        trust: ContactTrust,
        reply: Reply<UserId>,
    },
    SetChecked {
        uid: UserId,
        checked: bool,
        reply: Reply<()>,
    },
    SetGroupOnly {
        value: bool,
        reply: Reply<()>,
    },
    SendMessage {
        peer: UserId,
        text: String,
        reply_to: Option<MessageId>,
        reply: Reply<MessageId>,
    },
    MarkRead {
        peer: UserId,
        reply: Reply<()>,
    },
    SendFile {
        to: FileTarget,
        name: String,
        mime: String,
        bytes: Vec<u8>,
        reply_to: Option<MessageId>,
        reply: Reply<MessageId>,
    },
    LoadFile {
        id: MessageId,
        reply: Reply<Option<StoredFile>>,
    },
    Misc {
        op: Op,
        reply: Reply<Answer>,
    },
    SignalBlob {
        mailbox: String,
        blob: String,
    },
    /// One entry for the diagnostic log from a screen or the platform. Fire and forget.
    Diag {
        cat: String,
        event: String,
        detail: String,
    },
    Channel {
        op: ChannelOp,
        reply: Reply<String>,
    },
    MeshLink {
        link: String,
        up: bool,
    },
    MeshFrame {
        link: String,
        text: String,
    },
    Group {
        op: GroupOp,
        /// The new group's id, or the new message's id; empty for operations that return nothing.
        reply: Reply<String>,
    },
    LinkUp {
        peer: UserId,
        path: ConnectionPath,
        security: LinkSecurity,
        reply: Reply<()>,
    },
    LinkDown {
        peer: UserId,
    },
    Frame {
        peer: UserId,
        text: String,
    },
    Snapshot(Reply<Snapshot>),
    SnapshotRecent(usize, Reply<Snapshot>),
    MessagesBefore {
        conversation: ConversationId,
        before: Option<MessageId>,
        limit: usize,
        reply: Reply<MessagePage>,
    },
    Shutdown(oneshot::Sender<()>),
}

/// Invites, friends and emergency deletion.
#[derive(Debug)]
pub enum Op {
    CreateInvite,
    StopInvite,
    PreviewInvite(String),
    AcceptInvite(String),
    RetryAnswer,
    ClearAnswer,
    AcceptRequest(UserId),
    DismissRequest(UserId),
    RemoveContact(UserId),
    SetStoreForward(bool),
    SetDisplayName(String),
    DeleteMessage {
        id: MessageId,
        everyone: bool,
    },
    React {
        id: MessageId,
        emoji: String,
    },
    SetMuted {
        uid: UserId,
        muted: bool,
    },
    TogglePin(String),
    SetArchived {
        id: String,
        archived: bool,
    },
    SetMuteUntil {
        id: String,
        until_ms: i64,
    },
    SetSilent {
        id: String,
        silent: bool,
    },
    SetQuietHours {
        enabled: bool,
        from: u16,
        to: u16,
    },
    /// The account's synced preferences, as JSON.
    Prefs,
    /// Per-device receipts of one message, as JSON.
    MessageReceipts(MessageId),
    /// Radio hops of the latest mesh message per channel, as JSON.
    MeshHops,
    ShouldNotify(String),
    /// The person's time zone, minutes east of UTC.
    SetUtcOffset(i32),
    /// Ready-made times for "send later", as JSON.
    QuickTimes,
    /// What each chat takes on this device, as JSON.
    StorageByChat,
    SetSecureScreen(bool),
    /// The diagnostic log, as a Markdown file (given what the app says about itself, one line each).
    DiagExport(String),
    DiagClear,
    SetDiagEnabled(bool),
    /// `{ "enabled": bool, "entries": n }`.
    DiagStatus,
    /// A report with counts only (no content), given what the app says about itself, as JSON.
    Diagnostics(String),
    /// A quick check that the engine works, as JSON.
    SelfTest,
    RenameContact {
        uid: UserId,
        name: String,
    },
    BleSecret {
        peer: UserId,
    },
    ResetDialBackoff {
        peer: Option<UserId>,
    },
    SetLinkPath {
        peer: UserId,
        path: ConnectionPath,
    },
    RetryMessage {
        id: MessageId,
    },
    MarkConversationRead {
        conversation: ConversationId,
    },
    Search {
        query: String,
        limit: usize,
    },
    ExportBackup(String),
    StartDeviceLink,
    JoinDeviceLink {
        code: String,
        name: String,
    },
    /// On the new device: show a code for the first device to scan (returns it).
    StartScanLinkOffer {
        name: String,
    },
    /// On the first device: the code the new device shows was scanned.
    HostScannedLink {
        code: String,
    },
    /// On the new device: the six digits read off the first device.
    SubmitLinkCode(String),
    ConfirmDeviceLink(bool),
    CancelDeviceLink,
    /// Ask for a device of the account to be removed (in 24 hours, unless taken back).
    RequestDeviceRemoval(String),
    /// Take back a removal asked for.
    CancelDeviceRemoval(String),
    /// Remove a device at once, with the recovery phrase.
    RemoveDeviceNow {
        device: String,
        phrase: String,
        passphrase: String,
    },
    /// Write a message to go out later.
    ScheduleMessage {
        target: String,
        group: bool,
        text: String,
        at: i64,
    },
    CancelScheduled(String),
    /// Whether bulk work (media between devices) may run now.
    SetBulkAllowed(bool),
    /// The local-network beacon to send, saying this device listens on this port.
    LanBeacon(u16),
    /// A local-network beacon heard.
    LanBeaconSeen(String),
    ImportBackup {
        text: String,
        passphrase: String,
    },
    SendSignal {
        peer: UserId,
        message: crate::api::SignalMessage,
    },
    KillSwitchConversation {
        conversation: ConversationId,
        erase_session: bool,
    },
    KillSwitchApp,
    LastKillSwitchReport,
}

/// What an [`Op`] returns.
#[derive(Debug)]
pub enum Answer {
    Done,
    Invite {
        words: String,
        expires_at: i64,
    },
    Preview(Option<InvitePreview>),
    Person(UserId),
    Report(Option<KillSwitchReport>),
    Hits(Vec<crate::core::SearchHit>),
    Text(String),
    Restored(crate::core::RestoreSummary),
    Bytes(Vec<u8>),
    /// Another device of ours heard on the local network (or `None`).
    Lan(Option<crate::core::lan::LanPeer>),
    Maybe(Option<String>),
    Flag(bool),
}

/// A channel operation. Returns the channel's normalised name or the new message's id, else an empty string.
#[derive(Debug)]
pub enum ChannelOp {
    Join(String),
    Leave(String),
    Send { name: String, text: String },
}

/// Who a file goes to.
#[derive(Debug)]
pub enum FileTarget {
    Person(UserId),
    Group(String),
}

/// A group operation (one command variant for all of them keeps the queue's shape small).
#[derive(Debug)]
pub enum GroupOp {
    Create {
        name: String,
        members: Vec<UserId>,
    },
    /// Make a group from before protocol v2 again (returns the new id).
    RecreateLegacy {
        old_id: String,
    },
    DismissLegacy {
        old_id: String,
    },
    Add {
        group: String,
        uid: UserId,
    },
    Remove {
        group: String,
        uid: UserId,
    },
    Leave {
        group: String,
    },
    Rename {
        group: String,
        name: String,
    },
    Message {
        group: String,
        text: String,
        reply_to: Option<MessageId>,
    },
}

/// A handle to a running engine. Cheap to share by reference; dropping it shuts the engine down.
#[derive(Debug)]
pub struct EngineHandle {
    tx: mpsc::Sender<Command>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

fn pump(core: &mut Core, cmd: Command) -> bool {
    match cmd {
        Command::CreateIdentity(r) => drop(r.send(core.create_identity())),
        Command::RecoverIdentity {
            phrase,
            passphrase,
            reply,
        } => drop(reply.send(core.recover_identity(&phrase, &passphrase))),
        Command::OwnCard { name, reply } => drop(reply.send(core.own_card(name.as_deref()))),
        Command::AddContact { card, trust, reply } => {
            drop(reply.send(core.add_contact(&card, trust)))
        }
        Command::SetChecked {
            uid,
            checked,
            reply,
        } => drop(reply.send(core.set_checked(&uid, checked))),
        Command::SetGroupOnly { value, reply } => drop(reply.send(core.set_group_only(value))),
        Command::SendMessage {
            peer,
            text,
            reply_to,
            reply,
        } => drop(reply.send(core.send_message(&peer, &text, reply_to.as_ref()))),
        Command::MarkRead { peer, reply } => drop(reply.send(core.mark_read(&peer))),
        Command::SendFile {
            to,
            name,
            mime,
            bytes,
            reply_to,
            reply,
        } => drop(reply.send(match to {
            FileTarget::Person(p) => core.send_file(&p, &name, &mime, bytes, reply_to.as_ref()),
            FileTarget::Group(g) => {
                core.send_group_file(&g, &name, &mime, bytes, reply_to.as_ref())
            }
        })),
        Command::LoadFile { id, reply } => drop(reply.send(core.file(&id))),
        Command::Misc { op, reply } => drop(
            reply.send(match op {
                Op::CreateInvite => core
                    .create_invite()
                    .map(|(words, expires_at)| Answer::Invite { words, expires_at }),
                Op::StopInvite => {
                    core.stop_invite();
                    Ok(Answer::Done)
                }
                Op::PreviewInvite(t) => core.preview_invite(&t).map(Answer::Preview),
                Op::AcceptInvite(t) => core.accept_invite(&t).map(Answer::Person),
                Op::RetryAnswer => core.retry_answer().map(|()| Answer::Done),
                Op::ClearAnswer => {
                    core.clear_answer();
                    Ok(Answer::Done)
                }
                Op::AcceptRequest(u) => core.accept_request(&u).map(Answer::Person),
                Op::DismissRequest(u) => {
                    core.dismiss_request(&u);
                    Ok(Answer::Done)
                }
                Op::RemoveContact(u) => core.remove_contact(&u).map(|()| Answer::Done),
                Op::SetStoreForward(v) => core.set_store_forward(v).map(|()| Answer::Done),
                Op::SetDisplayName(n) => core.set_display_name(&n).map(|()| Answer::Done),
                Op::DeleteMessage { id, everyone } => {
                    core.delete_message(&id, everyone).map(|()| Answer::Done)
                }
                Op::React { id, emoji } => core.react(&id, &emoji).map(|()| Answer::Done),
                Op::SetMuted { uid, muted } => core.set_muted(&uid, muted).map(|()| Answer::Done),
                Op::TogglePin(id) => core.toggle_pin(&id).map(|()| Answer::Done),
                Op::SetArchived { id, archived } => {
                    core.set_archived(&id, archived).map(|()| Answer::Done)
                }
                Op::SetSilent { id, silent } => core.set_silent(&id, silent).map(|()| Answer::Done),
                Op::SetMuteUntil { id, until_ms } => {
                    core.set_mute_until(&id, until_ms).map(|()| Answer::Done)
                }
                Op::SetQuietHours { enabled, from, to } => core
                    .set_quiet_hours(enabled, from, to)
                    .map(|()| Answer::Done),
                Op::MessageReceipts(id) => Ok(Answer::Text(
                    serde_json::to_string(&core.message_receipts(&id)).unwrap_or_default(),
                )),
                Op::MeshHops => Ok(Answer::Text(
                    serde_json::to_string(&core.mesh_hops()).unwrap_or_default(),
                )),
                Op::Prefs => Ok(Answer::Text(
                    serde_json::to_string(&core.prefs()).unwrap_or_default(),
                )),
                Op::ShouldNotify(id) => Ok(Answer::Text(
                    if core.should_notify(&id) { "1" } else { "0" }.into(),
                )),
                Op::SetSecureScreen(on) => core.set_secure_screen(on).map(|()| Answer::Done),
                Op::DiagExport(about) => Ok(Answer::Text(core.diag_export(&about))),
                Op::DiagClear => {
                    core.diag_clear();
                    Ok(Answer::Done)
                }
                Op::SetDiagEnabled(on) => {
                    core.set_diag_enabled(on);
                    Ok(Answer::Done)
                }
                Op::DiagStatus => {
                    let enabled = core.diag_enabled();
                    let entries = core.diag_len();
                    Ok(Answer::Text(
                        serde_json::json!({ "enabled": enabled, "entries": entries }).to_string(),
                    ))
                }
                Op::Diagnostics(app) => Ok(Answer::Text(core.diagnostics(&app))),
                Op::SelfTest => Ok(Answer::Text(
                    serde_json::to_string(&core.self_test()).unwrap_or_default(),
                )),
                Op::StorageByChat => Ok(Answer::Text(
                    serde_json::to_string(&core.storage_by_chat()).unwrap_or_default(),
                )),
                Op::QuickTimes => Ok(Answer::Text(
                    serde_json::to_string(&core.quick_times()).unwrap_or_default(),
                )),
                Op::SetUtcOffset(minutes) => {
                    core.set_utc_offset(minutes);
                    Ok(Answer::Done)
                }
                Op::RetryMessage { id } => core.retry_message(&id).map(|()| Answer::Done),
                Op::SetLinkPath { peer, path } => {
                    core.set_link_path(&peer, path);
                    Ok(Answer::Done)
                }
                Op::ResetDialBackoff { peer } => {
                    core.reset_dial_backoff(peer.as_ref());
                    Ok(Answer::Done)
                }
                Op::BleSecret { peer } => core.ble_secret(&peer).map(Answer::Bytes),
                Op::RenameContact { uid, name } => {
                    core.rename_contact(&uid, &name).map(|()| Answer::Done)
                }
                Op::MarkConversationRead { conversation } => core
                    .mark_conversation_read(&conversation)
                    .map(|()| Answer::Done),
                Op::Search { query, limit } => Ok(Answer::Hits(core.search(&query, limit))),
                Op::ExportBackup(p) => core.export_backup(&p).map(Answer::Text),
                Op::StartDeviceLink => core.start_device_link().map(Answer::Text),
                Op::JoinDeviceLink { code, name } => {
                    core.join_device_link(&code, &name).map(|()| Answer::Done)
                }
                Op::StartScanLinkOffer { name } => {
                    core.start_scan_link_offer(&name).map(Answer::Text)
                }
                Op::HostScannedLink { code } => {
                    core.host_scanned_link(&code).map(|()| Answer::Done)
                }
                Op::SubmitLinkCode(digits) => core.submit_link_code(&digits).map(Answer::Flag),
                Op::ConfirmDeviceLink(accept) => {
                    core.confirm_device_link(accept).map(|()| Answer::Done)
                }
                Op::CancelDeviceLink => {
                    core.cancel_device_link();
                    Ok(Answer::Done)
                }
                Op::RequestDeviceRemoval(d) => {
                    core.request_device_removal(&d).map(|()| Answer::Done)
                }
                Op::CancelDeviceRemoval(d) => core.cancel_device_removal(&d).map(|()| Answer::Done),
                Op::RemoveDeviceNow {
                    device,
                    phrase,
                    passphrase,
                } => core
                    .remove_device_now(&device, &phrase, &passphrase)
                    .map(|()| Answer::Done),
                Op::ScheduleMessage {
                    target,
                    group,
                    text,
                    at,
                } => core
                    .schedule_message(&target, group, &text, at)
                    .map(|_| Answer::Done),
                Op::CancelScheduled(id) => core.cancel_scheduled(&id).map(|()| Answer::Done),
                Op::LanBeacon(port) => Ok(Answer::Maybe(core.lan_beacon(port))),
                Op::LanBeaconSeen(text) => Ok(Answer::Lan(core.lan_beacon_seen(&text))),
                Op::SetBulkAllowed(allowed) => {
                    core.set_bulk_allowed(allowed);
                    Ok(Answer::Done)
                }
                Op::ImportBackup { text, passphrase } => {
                    core.import_backup(&text, &passphrase).map(Answer::Restored)
                }
                Op::SendSignal { peer, message } => {
                    core.send_signal(&peer, &message).map(|()| Answer::Done)
                }
                Op::KillSwitchConversation {
                    conversation,
                    erase_session,
                } => Ok(Answer::Report(Some(
                    core.kill_switch_conversation(conversation, erase_session),
                ))),
                Op::KillSwitchApp => Ok(Answer::Report(Some(core.kill_switch_app()))),
                Op::LastKillSwitchReport => Ok(Answer::Report(core.last_kill_switch_report())),
            }),
        ),
        Command::SignalBlob { mailbox, blob } => core.signal_blob(&mailbox, &blob),
        Command::Diag { cat, event, detail } => core.diag_log(&cat, &event, &detail),
        Command::Channel { op, reply } => drop(
            reply.send(match op {
                ChannelOp::Join(n) => core.join_channel(&n),
                ChannelOp::Leave(n) => core.leave_channel(&n).map(|()| String::new()),
                ChannelOp::Send { name, text } => core
                    .send_channel_message(&name, &text)
                    .map(|id| id.to_string()),
            }),
        ),
        Command::MeshLink { link, up } => {
            if up {
                let _ = core.mesh_link_up(&link);
            } else {
                core.mesh_link_down(&link);
            }
        }
        Command::MeshFrame { link, text } => core.mesh_frame(&link, &text),
        Command::Group { op, reply } => drop(
            reply.send(match op {
                GroupOp::Create { name, members } => core.create_group(&name, &members),
                GroupOp::RecreateLegacy { old_id } => core.recreate_legacy_group(&old_id),
                GroupOp::DismissLegacy { old_id } => {
                    core.dismiss_legacy_group(&old_id).map(|()| String::new())
                }
                GroupOp::Add { group, uid } => {
                    core.add_group_member(&group, &uid).map(|()| String::new())
                }
                GroupOp::Remove { group, uid } => core
                    .remove_group_member(&group, &uid)
                    .map(|()| String::new()),
                GroupOp::Leave { group } => core.leave_group(&group).map(|()| String::new()),
                GroupOp::Rename { group, name } => {
                    core.rename_group(&group, &name).map(|()| String::new())
                }
                GroupOp::Message {
                    group,
                    text,
                    reply_to,
                } => core
                    .send_group_message(&group, &text, reply_to.as_ref())
                    .map(|id| id.to_string()),
            }),
        ),
        Command::LinkUp {
            peer,
            path,
            security,
            reply,
        } => drop(reply.send(core.link_up_with(&peer, path, security))),
        Command::LinkDown { peer } => core.link_down(&peer),
        Command::Frame { peer, text } => core.frame_received(&peer, &text),
        Command::Snapshot(r) => drop(r.send(Ok(core.snapshot()))),
        Command::SnapshotRecent(n, r) => drop(r.send(Ok(core.snapshot_recent(n)))),
        Command::MessagesBefore {
            conversation,
            before,
            limit,
            reply,
        } => drop(reply.send(core.messages_before(&conversation, before.as_ref(), limit))),
        Command::Shutdown(done) => {
            core.shutdown();
            let _ = done.send(());
            return false;
        }
    }
    true
}

impl EngineHandle {
    /// Start an engine. Loads persisted state first, so a failure to read storage is reported here.
    pub fn start(ports: Ports) -> Result<(Self, EventStream), ErrorCode> {
        Self::start_with_context(ports, "duo")
    }

    /// Start with the conversation context the legacy handshake uses (the apps' `conversationId` setting).
    pub fn start_with_context(
        ports: Ports,
        context: &str,
    ) -> Result<(Self, EventStream), ErrorCode> {
        let mut core = Core::new_with_context(ports, context)?;
        let events = core.subscribe();
        let (tx, mut rx) = mpsc::channel::<Command>(COMMAND_QUEUE);
        let thread = std::thread::Builder::new()
            .name("privcord-engine".into())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                else {
                    return;
                };
                rt.block_on(async move {
                    let mut timer = tokio::time::interval(TICK);
                    loop {
                        tokio::select! {
                            cmd = rx.recv() => match cmd {
                                Some(cmd) => { if !pump(&mut core, cmd) { break; } }
                                None => { core.shutdown(); break; }
                            },
                            _ = timer.tick() => core.tick(),
                        }
                    }
                });
            })
            .map_err(|_| ErrorCode::Busy)?;
        Ok((
            Self {
                tx,
                thread: Mutex::new(Some(thread)),
            },
            events,
        ))
    }

    async fn ask<T>(&self, build: impl FnOnce(Reply<T>) -> Command) -> Result<T, ErrorCode> {
        let (r, rx) = oneshot::channel();
        self.tx.try_send(build(r)).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => ErrorCode::Busy,
            mpsc::error::TrySendError::Closed(_) => ErrorCode::ShuttingDown,
        })?;
        rx.await.map_err(|_| ErrorCode::ShuttingDown)?
    }

    pub async fn create_identity(&self) -> Result<Zeroizing<String>, ErrorCode> {
        self.ask(Command::CreateIdentity).await
    }
    pub async fn recover_identity(&self, phrase: &str, passphrase: &str) -> Result<(), ErrorCode> {
        self.ask(|reply| Command::RecoverIdentity {
            phrase: Zeroizing::new(phrase.to_owned()),
            passphrase: Zeroizing::new(passphrase.to_owned()),
            reply,
        })
        .await
    }
    pub async fn own_card(&self, name: Option<&str>) -> Result<String, ErrorCode> {
        self.ask(|reply| Command::OwnCard {
            name: name.map(str::to_owned),
            reply,
        })
        .await
    }
    pub async fn add_contact(&self, card: &str, trust: ContactTrust) -> Result<UserId, ErrorCode> {
        self.ask(|reply| Command::AddContact {
            card: card.to_owned(),
            trust,
            reply,
        })
        .await
    }
    pub async fn set_checked(&self, uid: &UserId, checked: bool) -> Result<(), ErrorCode> {
        self.ask(|reply| Command::SetChecked {
            uid: uid.clone(),
            checked,
            reply,
        })
        .await
    }
    pub async fn set_group_only(&self, value: bool) -> Result<(), ErrorCode> {
        self.ask(|reply| Command::SetGroupOnly { value, reply })
            .await
    }
    pub async fn send_message(
        &self,
        peer: &UserId,
        text: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<MessageId, ErrorCode> {
        self.ask(|reply| Command::SendMessage {
            peer: peer.clone(),
            text: text.to_owned(),
            reply_to: reply_to.cloned(),
            reply,
        })
        .await
    }
    pub async fn mark_read(&self, peer: &UserId) -> Result<(), ErrorCode> {
        self.ask(|reply| Command::MarkRead {
            peer: peer.clone(),
            reply,
        })
        .await
    }
    pub async fn send_file(
        &self,
        to: FileTarget,
        name: &str,
        mime: &str,
        bytes: Vec<u8>,
        reply_to: Option<&MessageId>,
    ) -> Result<MessageId, ErrorCode> {
        self.ask(|reply| Command::SendFile {
            to,
            name: name.to_owned(),
            mime: mime.to_owned(),
            bytes,
            reply_to: reply_to.cloned(),
            reply,
        })
        .await
    }
    pub async fn load_file(&self, id: &MessageId) -> Result<Option<StoredFile>, ErrorCode> {
        self.ask(|reply| Command::LoadFile {
            id: id.clone(),
            reply,
        })
        .await
    }

    /// Invites, friends and emergency deletion.
    pub async fn op(&self, op: Op) -> Result<Answer, ErrorCode> {
        self.ask(|reply| Command::Misc { op, reply }).await
    }
    /// A blob arrived from the signaling service on a mailbox we listen on. Fire and forget.
    pub fn signal_blob(&self, mailbox: String, blob: String) -> Result<(), ErrorCode> {
        self.push(Command::SignalBlob { mailbox, blob })
    }
    /// An entry for the diagnostic log from a screen or the platform. Fire and forget (a screen must never wait on it).
    pub fn diag(&self, cat: String, event: String, detail: String) -> Result<(), ErrorCode> {
        self.push(Command::Diag { cat, event, detail })
    }
    pub async fn channel(&self, op: ChannelOp) -> Result<String, ErrorCode> {
        self.ask(|reply| Command::Channel { op, reply }).await
    }
    /// A Bluetooth mesh neighbour appeared (`up`) or went away. Fire and forget.
    pub fn mesh_link(&self, link: String, up: bool) -> Result<(), ErrorCode> {
        self.push(Command::MeshLink { link, up })
    }
    /// A mesh packet arrived from a neighbour. Fire and forget.
    pub fn mesh_frame(&self, link: String, text: String) -> Result<(), ErrorCode> {
        self.push(Command::MeshFrame { link, text })
    }

    /// Run one group operation. Returns the new group's or message's id where there is one, else an empty string.
    pub async fn group(&self, op: GroupOp) -> Result<String, ErrorCode> {
        self.ask(|reply| Command::Group { op, reply }).await
    }
    pub async fn snapshot(&self) -> Result<Snapshot, ErrorCode> {
        self.ask(Command::Snapshot).await
    }

    /// The snapshot with only the newest `per_chat` messages of each chat (`Snapshot::older` counts the rest).
    pub async fn snapshot_recent(&self, per_chat: usize) -> Result<Snapshot, ErrorCode> {
        self.ask(|reply| Command::SnapshotRecent(per_chat, reply))
            .await
    }

    /// Up to `limit` messages of a chat before `before` (the newest when `None`), oldest first.
    pub async fn messages_before(
        &self,
        conversation: ConversationId,
        before: Option<MessageId>,
        limit: usize,
    ) -> Result<MessagePage, ErrorCode> {
        self.ask(|reply| Command::MessagesBefore {
            conversation,
            before,
            limit,
            reply,
        })
        .await
    }

    /// An adapter reports that a link to a contact is up and that the engine must encrypt the frames itself.
    pub async fn link_up(&self, peer: &UserId, path: ConnectionPath) -> Result<(), ErrorCode> {
        self.link_up_with(peer, path, LinkSecurity::EngineEncrypted)
            .await
    }

    /// An adapter reports that a link to a contact is up, and says how it is protected.
    pub async fn link_up_with(
        &self,
        peer: &UserId,
        path: ConnectionPath,
        security: LinkSecurity,
    ) -> Result<(), ErrorCode> {
        self.ask(|reply| Command::LinkUp {
            peer: peer.clone(),
            path,
            security,
            reply,
        })
        .await
    }
    /// An adapter reports that the link went away. Fire and forget; the queue is bounded, so a full queue is reported.
    pub fn link_down(&self, peer: &UserId) -> Result<(), ErrorCode> {
        self.push(Command::LinkDown { peer: peer.clone() })
    }
    /// An adapter hands over a frame it received.
    pub fn frame_received(&self, peer: &UserId, text: String) -> Result<(), ErrorCode> {
        self.push(Command::Frame {
            peer: peer.clone(),
            text,
        })
    }

    fn push(&self, cmd: Command) -> Result<(), ErrorCode> {
        self.tx.try_send(cmd).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => ErrorCode::Busy,
            mpsc::error::TrySendError::Closed(_) => ErrorCode::ShuttingDown,
        })
    }

    /// Stop accepting commands, end every session, join the engine thread. Safe to call twice.
    pub async fn shutdown(&self) -> Result<(), ErrorCode> {
        let (done, rx) = oneshot::channel();
        if self.tx.send(Command::Shutdown(done)).await.is_ok() {
            let _ = rx.await;
        }
        self.join()
    }

    fn join(&self) -> Result<(), ErrorCode> {
        let handle = self
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match handle {
            Some(h) => h.join().map_err(|_| ErrorCode::ShuttingDown),
            None => Ok(()),
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        let (done, _rx) = oneshot::channel();
        // Only wait for the thread if it was told to stop. If the queue is full the thread still ends on its own as soon
        // as the sender below is dropped, and waiting here would risk a deadlock.
        if self.tx.try_send(Command::Shutdown(done)).is_ok() {
            let _ = self.join();
        }
    }
}
