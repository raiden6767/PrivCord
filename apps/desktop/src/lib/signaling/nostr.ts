import { schnorr } from '@noble/curves/secp256k1.js';
import { bytesToHex, hexToBytes } from '@noble/hashes/utils.js';
import { sha256 } from '@noble/hashes/sha2.js';
import { decodeBlob, encodeBlob, type SealedBlob } from '@duochat/protocol';
import {
  defaultSocketFactory,
  socketBackoff,
  type PutOptions,
  type SignalingStatus,
  type SignalingTransport,
  type WebSocketFactory,
} from './types';

type Timer = ReturnType<typeof setTimeout>;

export const NOSTR_KIND_SIGNAL = 21733; // ephemeral range: relays do not store it
export const NOSTR_KIND_MESSAGE = 31733; // parameterized replaceable range: relays store it

export interface NostrEvent {
  id: string;
  pubkey: string;
  created_at: number;
  kind: number;
  tags: string[][];
  content: string;
  sig: string;
}

const enc = new TextEncoder();

export function eventId(e: Omit<NostrEvent, 'id' | 'sig'>): string {
  return bytesToHex(sha256(enc.encode(JSON.stringify([0, e.pubkey, e.created_at, e.kind, e.tags, e.content]))));
}

/** Build an event signed with a fresh throwaway key, so events cannot be linked to the DuoChat identity. */
export function makeEvent(mailbox: string, blob: SealedBlob, opts: PutOptions, now = Math.floor(Date.now() / 1000)): NostrEvent {
  const sk = schnorr.utils.randomSecretKey();
  const base = {
    pubkey: bytesToHex(schnorr.getPublicKey(sk)),
    created_at: now,
    kind: opts.kind === 'sig' ? NOSTR_KIND_SIGNAL : NOSTR_KIND_MESSAGE,
    tags: [
      ['t', mailbox],
      // `d` makes every stored message its own addressable event instead of replacing the last one.
      ['d', crypto.randomUUID()],
      ['expiration', String(now + opts.ttlSec)],
    ],
    content: encodeBlob(blob),
  };
  const id = eventId(base);
  return { ...base, id, sig: bytesToHex(schnorr.sign(hexToBytes(id), sk)) };
}

export function verifyEvent(e: NostrEvent): boolean {
  try {
    if (typeof e.content !== 'string' || !Array.isArray(e.tags)) return false;
    if (eventId(e) !== e.id) return false;
    return schnorr.verify(hexToBytes(e.sig), hexToBytes(e.id), hexToBytes(e.pubkey));
  } catch {
    return false;
  }
}

interface Link {
  socket: WebSocket | null;
  attempt: number;
  timer: Timer | undefined;
  /** When anything last came from this relay: a socket that has been silent for too long is dead, whatever it claims. */
  lastRx: number;
}

/** How often each open socket is probed, and how long a silence is accepted before the socket is replaced. */
export const HEARTBEAT_MS = 20_000;
export const DEAD_AFTER_MS = 55_000;
const NO_SUCH_EVENT = '0'.repeat(64);

/** Signaling and mailboxes over public Nostr relays: decentralized, nothing to host. */
export class NostrTransport implements SignalingTransport {
  readonly name = 'nostr';
  private links = new Map<string, Link>();
  private mailbox: string | null = null;
  private onBlob: ((blob: SealedBlob, id: string) => void) | null = null;
  private stopped = true;
  private seen = new Set<string>();
  private subId = `duo-${Math.random().toString(36).slice(2, 10)}`;
  private beat: Timer | undefined;
  private lastBeat = 0;
  private probes = 0;
  private readonly wake = () => {
    if (typeof document === 'undefined' || document.visibilityState === 'visible') this.reconnectAll();
  };

  constructor(
    private readonly relays: string[],
    private readonly makeSocket: WebSocketFactory = defaultSocketFactory,
  ) {}

  start(mailbox: string, onBlob: (blob: SealedBlob, id: string) => void): void {
    this.mailbox = mailbox;
    this.onBlob = onBlob;
    this.stopped = false;
    for (const url of this.relays) {
      this.links.set(url, { socket: null, attempt: 0, timer: undefined, lastRx: Date.now() });
      this.open(url);
    }
    // A laptop that slept or changed network leaves sockets that look open and carry nothing (a relay never stores the
    // live signals sent meanwhile): probe them, and start over at once when the network or the window comes back.
    this.lastBeat = Date.now();
    globalThis.clearInterval(this.beat);
    this.beat = globalThis.setInterval(() => this.heartbeat(), HEARTBEAT_MS);
    globalThis.addEventListener?.('online', this.wake);
    globalThis.addEventListener?.('focus', this.wake);
    globalThis.document?.addEventListener?.('visibilitychange', this.wake);
  }

  /** Probe every open socket (any answer counts), and replace the ones that have gone quiet. A late tick means the machine slept. */
  heartbeat(now = Date.now()): void {
    if (this.stopped) return;
    const slept = now - this.lastBeat > HEARTBEAT_MS * 2.5;
    this.lastBeat = now;
    if (slept) return this.reconnectAll();
    for (const [url, link] of this.links) {
      const socket = link.socket;
      if (!socket || socket.readyState !== WebSocket.OPEN) continue;
      if (now - link.lastRx > DEAD_AFTER_MS) {
        this.replace(url);
        continue;
      }
      try {
        // A question every relay answers with EOSE at once: no subscription is left behind.
        const id = `hb-${this.probes++}`;
        socket.send(JSON.stringify(['REQ', id, { ids: [NO_SUCH_EVENT], limit: 1 }]));
        socket.send(JSON.stringify(['CLOSE', id]));
      } catch {
        this.replace(url);
      }
    }
  }

  /** Throw every socket away and connect again now (the network came back, or the window did). */
  reconnectAll(): void {
    if (this.stopped) return;
    for (const url of this.links.keys()) this.replace(url);
  }

  private replace(url: string): void {
    const link = this.links.get(url);
    if (!link || this.stopped) return;
    globalThis.clearTimeout(link.timer);
    const old = link.socket;
    link.socket = null;
    link.attempt = 0;
    if (old) {
      old.onclose = null;
      try {
        old.close();
      } catch {
        /* already gone */
      }
    }
    this.open(url);
  }

  status(): SignalingStatus {
    if (this.stopped) return 'idle';
    let connecting = false;
    for (const link of this.links.values()) {
      if (link.socket?.readyState === WebSocket.OPEN) return 'online';
      if (link.socket?.readyState === WebSocket.CONNECTING) connecting = true;
    }
    return connecting ? 'connecting' : 'error';
  }

  private open(url: string): void {
    const link = this.links.get(url);
    if (!link || this.stopped) return;
    let socket: WebSocket;
    try {
      socket = this.makeSocket(url);
    } catch {
      this.retry(url);
      return;
    }
    link.socket = socket;
    link.lastRx = Date.now();
    socket.onopen = () => {
      link.attempt = 0;
      link.lastRx = Date.now();
      // As far back as a stored message lives on a relay (a week): what was sent while the app was closed still arrives.
      const since = Math.floor(Date.now() / 1000) - 7 * 24 * 3600;
      socket.send(
        JSON.stringify(['REQ', this.subId, { kinds: [NOSTR_KIND_SIGNAL, NOSTR_KIND_MESSAGE], '#t': [this.mailbox], since }]),
      );
    };
    socket.onmessage = (event) => {
      link.lastRx = Date.now();
      this.handle(String(event.data));
    };
    socket.onclose = () => {
      if (link.socket !== socket) return;
      link.socket = null;
      this.retry(url);
    };
    socket.onerror = () => undefined;
  }

  private retry(url: string): void {
    const link = this.links.get(url);
    if (!link || this.stopped) return;
    globalThis.clearTimeout(link.timer);
    link.timer = globalThis.setTimeout(() => this.open(url), socketBackoff(link.attempt++));
  }

  private handle(raw: string): void {
    let frame: unknown;
    try {
      frame = JSON.parse(raw);
    } catch {
      return;
    }
    if (!Array.isArray(frame) || frame[0] !== 'EVENT' || frame[1] !== this.subId) return;
    const event = frame[2] as NostrEvent;
    if (!event || this.seen.has(event.id) || !verifyEvent(event)) return;
    if (!event.tags.some((t) => t[0] === 't' && t[1] === this.mailbox)) return;
    const expiry = event.tags.find((t) => t[0] === 'expiration')?.[1];
    if (expiry !== undefined && Number(expiry) < Date.now() / 1000) return;
    this.seen.add(event.id);
    if (this.seen.size > 2000) this.seen.delete(this.seen.values().next().value as string);
    const blob = decodeBlob(event.content);
    if (blob) this.onBlob?.(blob, event.id);
  }

  async put(mailbox: string, blob: SealedBlob, opts: PutOptions): Promise<void> {
    const event = makeEvent(mailbox, blob, opts);
    const payload = JSON.stringify(['EVENT', event]);
    let sent = 0;
    for (const link of this.links.values()) {
      if (link.socket?.readyState === WebSocket.OPEN) {
        link.socket.send(payload);
        sent += 1;
      }
    }
    if (sent === 0) throw new Error('No Nostr relay is connected.');
  }

  remove(): void {
    // Nostr relays cannot delete on our behalf; the expiration tag bounds retention.
  }

  stop(): void {
    this.stopped = true;
    globalThis.clearInterval(this.beat);
    globalThis.removeEventListener?.('online', this.wake);
    globalThis.removeEventListener?.('focus', this.wake);
    globalThis.document?.removeEventListener?.('visibilitychange', this.wake);
    for (const link of this.links.values()) {
      globalThis.clearTimeout(link.timer);
      const socket = link.socket;
      link.socket = null;
      if (socket) {
        socket.onclose = null;
        socket.close();
      }
    }
    this.links.clear();
  }
}
