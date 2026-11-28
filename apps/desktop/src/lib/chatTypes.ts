/**
 * The shapes the screens read: what the engine publishes (`ChatState` and its parts) and the stored message. The engine itself
 * is the Rust one, hosted in the shell (`hosted/`); this file holds only types and two constants.
 */
import type { ChatMessage, ConnectionPath, ContactCard, DeliveryState, Diagnosis, ReplyRef } from '@duochat/protocol';
import type { SignalingStatus } from './signalingStatus';

export interface FileMeta {
  name: string;
  mime: string;
  size: number;
  sha256: string;
}

export interface StoredMessage extends ChatMessage {
  /** Present when `type` is 'file'. The bytes live in the file store under the message id. */
  file?: FileMeta;
  /** Channel messages: the sender's self-chosen name plus a short id (they are not in our contacts). */
  senderName?: string;
  /** Who reacted with what (uid to emoji). */
  reactions?: Record<string, string>;
  /** Deleted for everyone: only a tombstone is kept. */
  deleted?: boolean;
  /** Contact uid for a direct chat, group id for a group chat. */
  conversationId: string;
  /** Delivery state per recipient uid. `deliveryState` is the weakest of these. */
  recipients?: Record<string, DeliveryState>;
}

export type EnginePhase = 'starting' | 'ready' | 'error' | 'stopped';
export type Trust = 'verified' | 'introduced';
export type ConnState = 'connected' | 'connecting' | 'offline';

export interface ContactRecord {
  uid: string;
  card: ContactCard;
  trust: Trust;
  /** uid of the member who introduced them (trust = introduced). */
  introducedBy?: string;
  nickname?: string;
  /** The user compared the safety number with them. */
  checked: boolean;
  addedAt: number;
}

export interface LastMessage {
  text: string;
  at: number;
  own: boolean;
  senderName?: string;
}

export interface ContactView {
  uid: string;
  name: string;
  trust: Trust;
  checked: boolean;
  introducedBy?: string;
  introducedByName?: string;
  conn: ConnState;
  path: ConnectionPath | null;
  via: 'webrtc' | 'ble' | null;
  unread: number;
  last?: LastMessage;
  /** Plain-language status line. */
  status: string;
  /** Why it is not connecting, in plain words, once it has been tried a few times. */
  diagnosis?: Diagnosis;
  /** Appears in the chat list and can be messaged. Group-only people do not until verified. */
  direct: boolean;
}

export interface MemberView {
  uid: string;
  name: string;
  role: 'admin' | 'member';
  isMe: boolean;
  addedBy: string;
  addedByName: string;
  conn: ConnState;
  introducedHere: boolean;
  /** Direct messages are possible with this person (you exchanged invites or verified them). */
  canMessage: boolean;
}

export interface GroupView {
  groupId: string;
  name: string;
  /** `member`: active. `removed` / `left`: read-only. */
  status: 'member' | 'removed' | 'left';
  members: MemberView[];
  amAdmin: boolean;
  /** The group has reached the hard member limit. */
  full: boolean;
  maxMembers: number;
  connected: number;
  unread: number;
  last?: LastMessage;
}

export interface UiMessage extends StoredMessage {
  own: boolean;
  senderName: string;
  /** What the tick shows: the weakest recipient. */
  tick: DeliveryState;
  delivered: number;
  total: number;
  /** 0..1 while a file is being sent or received. */
  progress?: number;
  /** Reactions grouped for display. */
  reactionChips: { emoji: string; count: number; mine: boolean }[];
}

export interface SearchHit {
  messageId: string;
  convId: string;
  convName: string;
  senderName: string;
  text: string;
  at: number;
}

/** Someone who used our invite words and answered with their own card. */
export interface InviteRequest {
  uid: string;
  name: string;
  safetyNumber: string;
}

/** A public channel we have joined. */
export interface ChannelView {
  /** Conversation id: `chan:` plus the normalized name. */
  id: string;
  name: string;
  unread: number;
  last?: LastMessage;
  conn: ConnState;
}

/** After typing a friend's words: did our own invite reach them? (They then confirm us on their side.) */
export interface InviteAnswer {
  uid: string;
  name: string;
  status: 'sending' | 'sent' | 'failed';
  error?: string;
}

export interface LegacyGroup {
  oldId: string;
  name: string;
  /** Names of friends who will be in it again. */
  members: string[];
  /** People who were in it but are not friends any more. */
  missing: number;
}

/** A message waiting for its time (kept on this device, sent by it). */
export interface ScheduledMessage {
  id: string;
  /** The conversation: a friend's uid or a group's id. */
  target: string;
  group: boolean;
  text: string;
  at: number;
}

/** One device of the account (hosted engine only). */
export interface DeviceInfo {
  id: string;
  name: string;
  addedAt: number;
  thisDevice: boolean;
  /** When it last wrote anything this device has, if ever. */
  lastSeen?: number;
  /** Not heard from for 90 days: "Remove or reconnect". */
  stale?: boolean;
  /** A removal was asked for: when it takes effect, unless taken back. */
  removalAt?: number;
}

/**
 * Linking a device, while it happens: the 6 digits to compare ("code"); on a new device that showed the QR, "enter-code"
 * (type the phone's digits), "wrong-code" and "matched"; then how it ended.
 */
export interface DeviceLinkState {
  stage: 'code' | 'enter-code' | 'matched' | 'wrong-code' | 'done' | 'failed' | 'cancelled' | 'expired';
  code?: string;
  /** On the device holding the account key: the new device's name. */
  name?: string;
}

/** Preferences that follow the person to every device (the engine holds and syncs them). */
export interface PrefsView {
  /** Conversation ids in the order pinned (at most 3). */
  pins: string[];
  archived: string[];
  /** Conversation id to the time (ms) its notifications are off until. */
  muteUntil: Record<string, number>;
  /** No notifications between two times of day, in minutes after midnight. */
  quiet: { enabled: boolean; from: number; to: number };
  /** Hide the app from screenshots and screen recording (applied per device). */
  secureScreen: boolean;
  /** Chats that notify without a sound. */
  silentChats: string[];
}

export const NO_PREFS: PrefsView = { pins: [], archived: [], muteUntil: {}, quiet: { enabled: false, from: 22 * 60, to: 7 * 60 }, secureScreen: false, silentChats: [] };

/** A mute with no end (small enough for JavaScript to hold exactly). */
export const MUTE_FOREVER = 9_000_000_000_000_000;

/** What the emergency wipe works on (the engine's database sits behind it). */
export interface WipeStore {
  clearAll(): Promise<void>;
  clearMessages(): Promise<void>;
  destroySchema(): Promise<void>;
}

export interface ChatState {
  /** Pins, archive, mutes and quiet hours (from the engine). */
  prefs?: PrefsView;
  /** The account's devices, and whether this one holds the account key (hosted engine only). */
  devices?: DeviceInfo[];
  holdsAccountKey?: boolean;
  /** Items from other devices that could not be recovered (the device was gone before they left it). */
  syncLost?: number;
  /** What this device holds (the usage screen). */
  usage?: { messages: number; files: number; fileBytes: number; contacts: number; syncEntries: number };
  /** Messages written for later on this device, soonest first. */
  scheduled?: ScheduledMessage[];
  deviceLink?: DeviceLinkState | null;
  channels: ChannelView[];
  answer: InviteAnswer | null;
  /** Our current six-word invite, if one is being shown. */
  invite: { words: string; expiresAt: number } | null;
  /** People who used our words and are waiting for us to accept them. */
  requests: InviteRequest[];
  /** Groups from before protocol v2 that must be made again (hosted engine only). */
  legacyGroups?: LegacyGroup[];
  phase: EnginePhase;
  uid: string;
  contacts: ContactView[];
  groups: GroupView[];
  messages: Record<string, UiMessage[]>;
  /** Hosted engine: how many older messages of a chat are not loaded yet (absent or 0: all are). */
  earlier?: Record<string, number>;
  signaling: SignalingStatus;
  storageBackend: 'sqlite' | 'browser';
  /** Messages waiting for someone to come online. */
  queued: number;
  error: string | null;
  notice: string | null;
}
