import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DEAD_AFTER_MS, HEARTBEAT_MS, NostrTransport } from './nostr';

/** A socket that records what it is sent and says whatever the test makes it say. */
class FakeSocket {
  readyState = 1; // OPEN
  sent: string[] = [];
  closed = false;
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  send(text: string) {
    this.sent.push(text);
  }
  close() {
    this.closed = true;
    this.readyState = 3;
  }
  /** The relay answers. */
  say(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
}

function setup(urls = ['wss://a.example', 'wss://b.example']) {
  const sockets: FakeSocket[] = [];
  const transport = new NostrTransport(urls, () => {
    const s = new FakeSocket();
    sockets.push(s);
    return s as unknown as WebSocket;
  });
  transport.start('box', () => undefined);
  for (const s of sockets) s.onopen?.();
  return { transport, sockets };
}

let current: NostrTransport | null = null;
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(1_700_000_000_000);
});
afterEach(() => {
  current?.stop();
  current = null;
  vi.useRealTimers();
});

/** Moves the clock and runs one heartbeat, as the timer would. */
function beat(t: NostrTransport, ms = HEARTBEAT_MS) {
  vi.setSystemTime(Date.now() + ms);
  t.heartbeat();
}

describe('a relay socket that looks open but carries nothing', () => {
  it('is asked a question every beat, and kept while the relay answers', () => {
    const { transport, sockets } = setup();
    current = transport;
    for (let i = 0; i < 5; i++) {
      beat(transport);
      for (const s of sockets) s.say(['EOSE', 'hb']); // the relay is alive
    }
    expect(sockets).toHaveLength(2); // nothing was replaced
    expect(sockets[0]?.sent.some((m) => m.includes('"REQ"') && m.includes('hb-'))).toBe(true);
    expect(sockets.every((s) => !s.closed)).toBe(true);
  });

  it('is replaced after it has been silent too long, and only that one', () => {
    const { transport, sockets } = setup();
    current = transport;
    // Both are probed; only the first relay keeps answering.
    for (let t = 0; t <= DEAD_AFTER_MS + HEARTBEAT_MS; t += HEARTBEAT_MS) {
      beat(transport);
      sockets[0]?.say(['EOSE', 'hb']);
    }
    expect(sockets[1]?.closed).toBe(true);
    expect(sockets[0]?.closed).toBe(false);
    expect(sockets).toHaveLength(3); // a fresh socket for the silent relay
  });

  it('is replaced at once when the machine has been asleep (the beat comes very late)', () => {
    const { transport, sockets } = setup();
    current = transport;
    beat(transport, 10 * 60_000);
    expect(sockets.slice(0, 2).every((s) => s.closed)).toBe(true);
    expect(sockets).toHaveLength(4);
  });

  it('is replaced when the network or the window comes back', () => {
    const { transport, sockets } = setup(['wss://a.example']);
    current = transport;
    transport.reconnectAll();
    expect(sockets[0]?.closed).toBe(true);
    expect(sockets).toHaveLength(2);
    // and the new one subscribes as soon as it opens
    sockets[1]?.onopen?.();
    expect(sockets[1]?.sent[0]).toContain('"REQ"');
  });

  it('is not replaced twice when the old socket reports its own close afterwards', () => {
    const { transport, sockets } = setup(['wss://a.example']);
    current = transport;
    const old = sockets[0];
    transport.reconnectAll();
    old?.onclose?.(); // a late close event from the thrown-away socket
    expect(sockets).toHaveLength(2);
  });
});
