import { describe, expect, it } from 'vitest';
import type { ChatEngineApi } from './engineApi';
import { attachDiag, plainTitle, withDiag } from './diag';

describe('the diagnostic log from the page', () => {
  const entries: Array<[string, string, string]> = [];
  const engine = {
    diagLog: (c: string, e: string, d: string) => void entries.push([c, e, d]),
    diagStatus: async () => ({ enabled: true, entries: 0 }),
    send: async (_to: string, text: string) => text.length,
    renameFriend: async () => {
      throw new Error('could not');
    },
    getState: () => ({ secret: 'kept out' }),
    refresh: () => undefined,
  } as unknown as ChatEngineApi;

  it('writes down what was asked and how it ended, never what was said', async () => {
    attachDiag(engine);
    const logged = withDiag(engine) as unknown as Record<string, (...a: unknown[]) => Promise<unknown>>;
    await logged.send!('friend-uid-1234567890abcdef', 'my secret plan for tuesday');
    await expect(logged.renameFriend!('friend-uid', 'Alice Wonderland')).rejects.toThrow('could not');
    const text = JSON.stringify(entries);
    expect(text).toContain('send: asked');
    expect(text).toContain('chars=26');
    expect(text).toContain('send: done');
    expect(text).toContain('renameFriend: failed');
    expect(text).not.toContain('secret plan');
    expect(text).not.toContain('Alice');
    expect(text).not.toContain('friend-uid');
  });

  it('leaves out the calls that run all the time', () => {
    const before = entries.length;
    const logged = withDiag(engine) as unknown as Record<string, () => unknown>;
    logged.getState!();
    logged.refresh!();
    expect(entries.length).toBe(before);
  });

  it('names a dialog only when its title is the same for everyone', () => {
    expect(plainTitle('Settings')).toBe('Settings');
    expect(plainTitle('Alice Wonderland')).toBe('a dialog');
    expect(plainTitle('Alice used your invite')).toBe('a dialog');
  });
});
