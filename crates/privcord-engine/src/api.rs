//! What a UI sees: typed commands in, typed events and snapshots out. UIs hold no business state of their own.

use privcord_domain::{
    message::{ChatMessage, DeliveryState},
    privacy::ContactTrust,
    state::ConnectionState,
    ConversationId, MessageId, UserId,
};

/// A contact as shown to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactView {
    pub uid: UserId,
    pub name: Option<String>,
    /// What to show: their name, or "Friend" and the start of their id.
    pub display_name: String,
    /// Who introduced them (an introduced contact), if known.
    pub introduced_by: Option<UserId>,
    pub trust: ContactTrust,
    pub checked: bool,
    pub safety_number: String,
    pub connection: ConnectionState,
    /// Whether a direct chat is allowed with this person right now (the group-only rules).
    pub can_message: bool,
    /// We added them from their card and they have not added us back yet: nothing can connect until they accept.
    pub awaiting_acceptance: bool,
}

/// A group you are in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupView {
    pub group_id: String,
    pub name: String,
    pub created_by: String,
    /// Each member and whether they are an admin.
    pub members: Vec<(String, bool)>,
    /// You are an admin (you may remove others).
    pub am_admin: bool,
    /// The group has reached its member limit.
    pub full: bool,
    pub max_members: usize,
    /// How many of the other members are connected right now.
    pub connected: usize,
}

/// The newest message of a conversation, as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastMessage {
    /// The text, or "Message deleted", or a paperclip and the file's name.
    pub text: String,
    pub at: i64,
    pub own: bool,
    pub sender: UserId,
}

/// One conversation as a chat list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSummary {
    pub conversation: ConversationId,
    pub last: Option<LastMessage>,
    /// Messages from others you have not read.
    pub unread: usize,
}

/// How far a message of yours has got with each person it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryDetail {
    pub message_id: MessageId,
    /// People it has still to reach.
    pub pending: usize,
    /// People it is for (the others in the chat).
    pub total: usize,
}

/// Connection setup carried in sealed messages. The platform's WebRTC code produces and consumes the descriptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalMessage {
    /// "I am online and waiting": the other side (if it dials) should offer now.
    Ready,
    Offer(String),
    Answer(String),
}

/// Our open six-word invite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteView {
    pub words: String,
    pub expires_at: i64,
}

/// Someone who used our words and waits for us to accept them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestView {
    pub uid: UserId,
    pub name: String,
    pub safety_number: String,
}

/// Did our own card reach the friend whose words we typed?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerStatus {
    Sent,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerView {
    pub uid: UserId,
    pub status: AnswerStatus,
}

/// Everything a recreated UI needs, so it never depends on having seen every historical event.
/// A group made before protocol v2, which can be made again with one tap (see `Core::recreate_legacy_group`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyGroupView {
    /// Its old id: what to pass back, and where its messages are.
    pub old_id: String,
    pub name: String,
    /// Who was in it at the end (v2 ids; only those who are still friends can be added again).
    pub members: Vec<UserId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Number of the last event this snapshot includes. Events after it have a higher number.
    pub seq: u64,
    pub own_uid: Option<UserId>,
    pub contacts: Vec<ContactView>,
    pub groups: Vec<GroupView>,
    /// Last message and unread count per conversation (only conversations with messages).
    pub summaries: Vec<ConversationSummary>,
    /// Delivery detail for your own messages that still have people to reach.
    pub deliveries: Vec<DeliveryDetail>,
    /// Names of the public channels joined.
    pub channels: Vec<String>,
    /// Reactions per message: who reacted with what.
    pub reactions: Vec<(MessageId, Vec<(UserId, String)>)>,
    /// Messages deleted for everyone (they show as removed).
    pub deleted: Vec<MessageId>,
    /// People whose channel messages are hidden.
    pub muted: Vec<UserId>,
    pub invite: Option<InviteView>,
    pub requests: Vec<RequestView>,
    pub answer: Option<AnswerView>,
    /// We are looking for the card behind words we typed.
    pub lookup_pending: bool,
    pub messages: Vec<(ConversationId, Vec<ChatMessage>)>,
    pub queued: usize,
    /// The name each person gave in a public channel (they are not contacts).
    pub channel_people: Vec<(UserId, String)>,
    /// Type and size of every file message that is stored.
    pub files: Vec<(MessageId, privcord_ports::FileInfo)>,
    /// Groups from before protocol v2, waiting to be made again or dismissed.
    pub legacy_groups: Vec<LegacyGroupView>,
    /// The account's devices (active ones), this one included.
    pub devices: Vec<DeviceView>,
    /// Whether this install holds the account key (it can link other devices and change groups).
    pub holds_account_key: bool,
    /// Items from other devices of the account that could not be recovered (a device gone before they left it).
    pub sync_lost: u64,
    /// Messages written for later on this device, soonest first.
    pub scheduled: Vec<crate::core::Scheduled>,
    /// What this device holds (the usage screen).
    pub usage: UsageView,
    /// Chats whose older messages were left out of `messages` (a windowed snapshot): how many are still to load with
    /// `messages_before`. Empty when `messages` holds everything.
    pub older: Vec<(ConversationId, usize)>,
}

/// A page of one chat's history, oldest first (see `Core::messages_before`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePage {
    pub messages: Vec<ChatMessage>,
    /// Messages older than the first of these are still to load.
    pub more: bool,
    pub own_uid: Option<UserId>,
    /// What a snapshot carries beside the messages, for these messages only.
    pub files: Vec<(MessageId, privcord_ports::FileInfo)>,
    pub reactions: Vec<(MessageId, Vec<(UserId, String)>)>,
    pub deleted: Vec<MessageId>,
    pub deliveries: Vec<DeliveryDetail>,
}

/// What this device holds: counts and the size of the files (build step 10, the usage screen).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageView {
    pub messages: u64,
    pub files: u64,
    pub file_bytes: u64,
    pub contacts: u64,
    /// Entries of the account's sync log held here (zero on a one-device account that never synced).
    pub sync_entries: u64,
}

/// One device of the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    /// When it was linked (milliseconds).
    pub added_at: i64,
    pub this_device: bool,
    /// When it last wrote anything this device has (milliseconds), if ever.
    pub last_seen: Option<i64>,
    /// Not heard from for 90 days: "Remove or reconnect".
    pub stale: bool,
    /// A removal was asked for: when it takes effect (milliseconds), unless taken back.
    pub removal_at: Option<i64>,
}

/// Something that happened. Each has a number; numbers rise by one, so a gap means events were missed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    Ready,
    IdentityCreated,
    ContactAdded {
        uid: UserId,
    },
    MessageAdded {
        conversation: ConversationId,
        message_id: MessageId,
        own: bool,
    },
    MessageStatusChanged {
        message_id: MessageId,
        status: DeliveryState,
    },
    /// A group's membership, name or history changed (or you joined or left it).
    GroupChanged {
        group_id: String,
    },
    /// Our open invite appeared, changed or closed, or the list of requests changed.
    InviteChanged,
    InviteRequest {
        uid: UserId,
    },
    InviteFound {
        uid: UserId,
        name: String,
        safety_number: String,
        already: bool,
    },
    InviteLookupFailed,
    AnswerChanged,
    ContactRemoved {
        uid: UserId,
    },
    /// Time to dial this person: their link is down and the backoff has passed. `initiator` is true when we are the
    /// side that sends the offer (the lower uid).
    DialRequested {
        peer: UserId,
        initiator: bool,
    },
    /// An emergency deletion ran (the report says how far it got).
    Wiped,
    /// A contact sent connection setup. Hand `Offer` and `Answer` to the WebRTC code; on `Ready`, dial if you are the
    /// side that offers.
    SignalReceived {
        peer: UserId,
        message: SignalMessage,
    },
    /// A message's reactions or its deleted mark changed.
    MessageChanged {
        message_id: MessageId,
    },
    /// A message was removed from this device ("delete for me").
    MessageRemoved {
        message_id: MessageId,
    },
    /// A backup was restored: ask for a snapshot.
    Restored,
    /// Linking a device by QR code: `stage` is "code" (show `code`, the 6 digits; on the device holding the account
    /// key, `name` is the new device's), then "done", "failed", "cancelled" or "expired".
    DeviceLink {
        stage: String,
        code: Option<String>,
        name: Option<String>,
    },
    /// A setting the snapshot shows changed (the muted list).
    SettingsChanged,
    /// A file is arriving: how much of it is here, in percent.
    FileProgress {
        message_id: MessageId,
        percent: u8,
    },
    /// A file from `conversation` has started to arrive (so the chat can show it before it is whole).
    FileIncoming {
        conversation: ConversationId,
        message_id: MessageId,
        name: String,
        size: usize,
    },
    /// Older versions: a file has been handed to the link for `peer`, `frames` frames, the first of which is the offer.
    /// Files now go in windows, so this is no longer sent; the sender's progress comes as `FileProgress`.
    FileQueued {
        peer: UserId,
        message_id: MessageId,
        frames: u32,
    },
    MessageRefused {
        message_id: MessageId,
    },
    PeerStateChanged {
        peer: UserId,
        state: ConnectionState,
    },
    /// This subscriber fell behind and `missed` events were dropped. Ask for a snapshot.
    Lagged {
        missed: u64,
    },
    ShutDown,
}
