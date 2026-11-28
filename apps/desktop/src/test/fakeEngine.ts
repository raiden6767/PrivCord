/**
 * A small in-memory stand-in for the engine (`ChatEngineApi`) so the screens can be rendered and driven in a test without Tauri
 * or the Rust engine. It records what the UI asked for; `push` publishes a new state the way the real engine does. Any method the
 * test does not care about answers with a harmless default.
 */
import { vi } from 'vitest';
import { NO_PREFS, type ChatState, type ContactView, type GroupView, type UiMessage } from '../lib/chatTypes';
import type { ChatEngineApi } from '../lib/engineApi';

export function contact(uid: string, name: string, extra: Partial<ContactView> = {}): ContactView {
  return { uid, name, trust: 'verified', checked: true, conn: 'connected', path: null, via: 'webrtc', unread: 0, status: 'Connected', direct: true, ...extra };
}

export function group(groupId: string, name: string, members: number, connected: number, extra: Partial<GroupView> = {}): GroupView {
  return {
    groupId,
    name,
    status: 'member',
    members: Array.from({ length: members }, (_, i) => ({
      uid: `m${i}`, name: `Member ${i}`, role: i === 0 ? 'admin' : 'member', isMe: i === 0, addedBy: 'm0', addedByName: 'Member 0',
      conn: 'connected', introducedHere: false, canMessage: true,
    })),
    amAdmin: true,
    full: members >= 10,
    maxMembers: 10,
    connected,
    unread: 0,
    ...extra,
  };
}

export function message(id: string, conv: string, text: string, own: boolean, extra: Partial<UiMessage> = {}): UiMessage {
  return {
    messageId: id, clientMessageId: id, senderId: own ? 'me' : conv, conversationId: conv, text, type: 'text', transport: 'webrtc',
    deliveryState: 'SENT', clientCreatedAt: 1_700_000_000_000, readAt: null, own, senderName: own ? 'You' : 'Them', tick: 'SENT',
    delivered: 0, total: 1, reactionChips: [], ...extra,
  } as UiMessage;
}

export function baseState(extra: Partial<ChatState> = {}): ChatState {
  return {
    prefs: NO_PREFS, channels: [], answer: null, invite: null, requests: [], phase: 'ready', uid: 'me', contacts: [], groups: [],
    messages: {}, signaling: 'online', storageBackend: 'sqlite', queued: 0, error: null, notice: null, ...extra,
  };
}

export type FakeEngine = ChatEngineApi & { push(next: Partial<ChatState>): void; calls: Record<string, ReturnType<typeof vi.fn>> };

export function fakeEngine(initial: ChatState, overrides: Partial<Record<keyof ChatEngineApi, unknown>> = {}): FakeEngine {
  let state = initial;
  const listeners = new Set<(s: ChatState) => void>();
  const calls: Record<string, ReturnType<typeof vi.fn>> = {};
  const own: Record<string, unknown> = {
    getState: () => state,
    subscribe: (l: (s: ChatState) => void) => {
      listeners.add(l);
      l(state);
      return () => listeners.delete(l);
    },
    push(next: Partial<ChatState>) {
      state = { ...state, ...next };
      listeners.forEach((l) => l(state));
    },
    calls,
    myInvite: () => 'privcord-invite-me',
    createInvite: async () => ({ words: 'apple river stone cloud maple quiet', expiresAt: Date.now() + 900_000 }),
    send: async () => ({ ok: true }),
    sendFile: async () => ({ ok: true }),
    getFile: async () => null,
    search: () => [],
    quickTimes: async () => [],
    bluetoothAvailable: async () => false,
    localStore: { clearAll: async () => undefined, clearMessages: async () => undefined, destroySchema: async () => undefined },
    ...overrides,
  };
  // Every other method is a recorded no-op, so a screen can call anything without the test listing it.
  return new Proxy(own, {
    get(target, key: string) {
      if (key in target) {
        const value = target[key];
        if (typeof value !== 'function' || key === 'push') return value;
        return (calls[key] ??= vi.fn(value as (...a: unknown[]) => unknown));
      }
      if (key === 'then') return undefined;
      // Anything else (optional capabilities included) is present and answers with an empty promise.
      return (calls[key] ??= vi.fn(() => Promise.resolve([])));
    },
  }) as unknown as FakeEngine;
}
