import { describe, expect, it } from 'vitest';
import type { EngineBridge } from '../engineBridge';
import { HostedChatEngine } from './hostedEngine';

function make(answers: Record<string, unknown> = {}, relayWaitMs?: number) {
  const calls: [string, unknown][] = [];
  const bridge: EngineBridge = {
    start: async () => undefined,
    stop: async () => undefined,
    invoke: async (command, args) => {
      calls.push([command, args]);
      if (command in answers) {
        const a = answers[command];
        if (a instanceof Error) throw a;
        return a;
      }
      return null;
    },
    on: async () => () => undefined,
  };
  const engine = new HostedChatEngine({ bridge, ownName: () => '', network: () => ({ signaling: [], turn: [], storeForward: false }), relayWaitMs } as never);
  return { engine, calls };
}

describe('linking a new device by scanning', () => {
  it('asks the engine for a code to show, under a trimmed device name', async () => {
    const { engine, calls } = make({ start_scan_link_offer: 'duocord-link-1:abc' });
    expect(await engine.startScanLinkOffer('  My Laptop ')).toBe('duocord-link-1:abc');
    expect(calls[0]).toEqual(['start_scan_link_offer', { name: 'My Laptop' }]);
  });

  it('sends the six digits and says whether they matched', async () => {
    const yes = make({ submit_link_code: true });
    expect(await yes.engine.submitLinkCode('123456')).toBe(true);
    expect(yes.calls[0]).toEqual(['submit_link_code', { digits: '123456' }]);
    const no = make({ submit_link_code: false });
    expect(await no.engine.submitLinkCode('000000')).toBe(false);
  });

  it('passes a scanned code to the engine, and an engine error comes back as words', async () => {
    const ok = make();
    await ok.engine.hostScannedLink(' duocord-link-1:xyz ');
    expect(ok.calls[0]).toEqual(['host_scanned_link', { code: 'duocord-link-1:xyz' }]);
    const bad = make({ host_scanned_link: new Error('NotAdmin') });
    await expect(bad.engine.hostScannedLink('x')).rejects.toBeInstanceOf(Error);
  });

  it('shows the new device the stages the engine reports', () => {
    const { engine } = make();
    (engine as unknown as { onEvent(e: unknown): void }).onEvent({ DeviceLink: { stage: 'enter-code' } });
    expect((engine as unknown as { deviceLink: unknown }).deviceLink).toEqual({ stage: 'enter-code' });
    (engine as unknown as { onEvent(e: unknown): void }).onEvent({ DeviceLink: { stage: 'wrong-code' } });
    expect((engine as unknown as { deviceLink: unknown }).deviceLink).toEqual({ stage: 'wrong-code' });
  });

  it('waits for the relay sockets right after start instead of failing on the first tap', async () => {
    let tries = 0;
    const bridge: EngineBridge = {
      start: async () => undefined,
      stop: async () => undefined,
      invoke: async (command) => {
        // Only the link call counts; the state refresh after it gets nothing back, as in the first helper.
        if (command !== 'start_scan_link_offer') return null;
        tries += 1;
        if (tries < 3) throw new Error('NoSignaling');
        return 'duocord-link-1:ok';
      },
      on: async () => () => undefined,
    };
    const engine = new HostedChatEngine({ bridge, ownName: () => '', network: () => ({ signaling: [], turn: [], storeForward: false }) } as never);
    expect(await engine.startScanLinkOffer('PC')).toBe('duocord-link-1:ok');
    expect(tries).toBe(3);
  });

  it('gives up with a clear message when no relay ever comes up, and does not wait on other errors', async () => {
    const down = make({ start_scan_link_offer: new Error('NoSignaling') }, 50);
    await expect(down.engine.startScanLinkOffer('PC')).rejects.toThrow(/Could not reach a connection service/);
    expect(down.calls.length).toBeGreaterThan(0);
    const other = make({ start_scan_link_offer: new Error('IdentityExists') }, 5000);
    const started = Date.now();
    await expect(other.engine.startScanLinkOffer('PC')).rejects.toBeInstanceOf(Error);
    expect(Date.now() - started).toBeLessThan(1000);
    expect(other.calls.length).toBe(1);
  });
});
