/**
 * Everything the screens use from a chat engine. Two implementations: `ChatEngine` (the original TypeScript engine) and
 * `HostedChatEngine` (the same behaviour from the Rust engine hosted in the Tauri shell). Same idea as `ChatEngineApi` on Android.
 */
import type { FileMeta, LocalStore } from './localDb';
import type { ChatState, SearchHit } from './engine';

export interface DeviceReceipt {
  deviceId: string;
  deviceName: string | null;
  state: 'delivered' | 'read' | 'waiting';
  at: number;
}

export type SendResult = { ok: true } | { ok: false; error: string };

export interface ChatEngineApi {
  getState(): ChatState;
  subscribe(listener: (state: ChatState) => void): () => void;
  /** The store the emergency wipe works on. */
  readonly localStore: LocalStore;

  start(): Promise<void>;
  stop(): void;
  setVisible(visible: boolean): void;
  openConversation(convId: string | null): void;
  /** Load a page of older messages of a chat (only the hosted engine keeps a window; the other holds them all). */
  loadEarlier?(convId: string): Promise<void>;
  clearNotice(): void;

  /** The text a friend scans or pastes to add us. */
  myInvite(): string;
  refreshOwnName(name: string): Promise<void>;
  createInvite(): Promise<{ words: string; expiresAt: number }>;
  /** Stop publishing the six-word invite (the Add a friend screen was closed): while it is open, relays see it posted. */
  closeInvite?(): void;
  previewInvite(text: string): Promise<{ uid: string; name: string; safetyNumber: string; already: boolean }>;
  addContact(text: string): Promise<string>;
  retryAnswer(): void;
  clearAnswer(): void;
  acceptRequest(uid: string): Promise<string>;
  dismissRequest(uid: string): void;
  setChecked(uid: string): void;
  safetyNumberFor(uid: string): Promise<string>;
  setNickname(uid: string, name: string): void;
  removeContact(uid: string): void;
  setGroupOnly(value: boolean): void;

  send(convId: string, rawText: string, replyToId?: string): Promise<SendResult>;
  sendFile(convId: string, file: { name: string; mime: string; bytes: Uint8Array }, replyToId?: string): Promise<SendResult>;
  getFile(messageId: string): Promise<{ blob: Blob; meta: FileMeta } | null>;
  retry(messageId: string): Promise<void>;
  deleteMessage(messageId: string, everyone: boolean): void;
  react(messageId: string, emoji: string): void;
  search(query: string, limit?: number): SearchHit[];

  createGroup(name: string, memberUids: string[]): Promise<string>;
  addGroupMember(groupId: string, uid: string): Promise<void>;
  removeGroupMember(groupId: string, uid: string): Promise<void>;
  leaveGroup(groupId: string): Promise<void>;
  recreateLegacyGroup?(oldId: string): Promise<string>;
  dismissLegacyGroup?(oldId: string): Promise<void>;
  renameGroup(groupId: string, name: string): Promise<void>;

  /** Linking another device of the account (hosted engine only). */
  /** Preferences held by the engine and synced to every device of the account. */
  /** Ready-made times for "send later" in the person's own time: in an hour, this evening, tomorrow morning. */
  quickTimes?(): Promise<{ label: string; at: number }[]>;
  /** Small pure answers computed once in the shared engine (`checkup`, `hosts`): JSON in, JSON out. */
  insight?(name: string, input: unknown): Promise<unknown>;
  /** What each chat takes on this device (the shared engine counts it). */
  storageByChat?(): Promise<Array<{ id: string; messages: number; files: number; bytes: number; fileBytes: number; fileIds: string[] }>>;
  /** A counts-only report of this install (the engine builds it from `app`); pretty JSON text. */
  diagnostics?(app: unknown): Promise<string>;
  /** One entry for the diagnostic log (kept on this computer only). Never waits. */
  diagLog?(category: string, event: string, detail: string): void;
  /** The diagnostic log as a Markdown file; `about` is what the app says about itself, one line each. */
  diagExport?(about: string): Promise<string>;
  diagClear?(): Promise<void>;
  setDiagEnabled?(on: boolean): Promise<void>;
  diagStatus?(): Promise<{ enabled: boolean; entries: number }>;
  /** The engine's own checks, each with a verdict. */
  selfTest?(): Promise<Array<{ name: string; ok: boolean; detail: string }>>;
  /** Hide the app from screenshots and recording (a preference that follows the account). */
  setSecureScreen?(on: boolean): void;
  togglePin?(conversation: string): void;
  setArchived?(conversation: string, archived: boolean): void;
  /** Turn a chat's notifications off until a time in ms (0 turns them on again). */
  setMuteUntil?(conversation: string, untilMs: number): void;
  /** Notify without a sound for a chat (a preference that follows the account). */
  setSilent?(conversation: string, silent: boolean): void;
  /** Per-device receipts of one of our messages: one line per device of the person (the engine builds them). */
  messageReceipts?(messageId: string): Promise<DeviceReceipt[]>;
  /** How many nearby hops each chat's messages take over the Bluetooth/Wi-Fi mesh, by conversation id. */
  meshHops?(): Promise<Record<string, number>>;
  setQuietHours?(enabled: boolean, from: number, to: number): void;

  startDeviceLink?(): Promise<string>;
  joinDeviceLink?(code: string, name: string): Promise<void>;
  confirmDeviceLink?(accept: boolean): Promise<void>;
  cancelDeviceLink?(): void;
  /** New device: show a QR (the returned code) for the phone to scan; the phone then shows six digits to type here. */
  startScanLinkOffer?(name: string): Promise<string>;
  /** New device: the six digits the phone shows. Resolves true when they match (the phone still has to confirm). */
  submitLinkCode?(digits: string): Promise<boolean>;
  /** Device holding the account key: the code scanned (or pasted) from the new device's QR. */
  hostScannedLink?(code: string): Promise<void>;
  /** Scheduled send: a message to go out at a time (a friend or a group; kept on this device). */
  scheduleMessage?(convId: string, text: string, at: number): Promise<{ ok: true } | { ok: false; error: string }>;
  cancelScheduled?(id: string): Promise<void>;
  /** Removing another device of the account: in 24 hours (any device can take it back), or now with the phrase. */
  requestDeviceRemoval?(deviceId: string): Promise<void>;
  cancelDeviceRemoval?(deviceId: string): Promise<void>;
  removeDeviceNow?(deviceId: string, phrase: string, passphrase?: string): Promise<void>;

  joinChannel(raw: string, persist?: boolean): Promise<string>;
  leaveChannel(convId: string): void;
  muteSender(uid: string): void;

  exportBackup(passphrase: string): Promise<string>;
  importBackup(text: string, passphrase: string): Promise<{ friends: number; groups: number; messages: number }>;

  reloadNetwork(): void;
  enableStoreForward(): void;

  bluetoothAvailable(): Promise<boolean>;
  connectBluetooth(uid: string): Promise<void>;
  createManualOffer(uid: string): Promise<string>;
  applyManualAnswer(code: string): Promise<void>;
  acceptManualOffer(code: string): Promise<string>;
  cancelManual(): void;

  clearLocalData(): Promise<void>;
  clearLocalChat(): Promise<void>;
  eraseLocalChatSession(): void;
}
