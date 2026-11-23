import { afterEach, describe, expect, it, vi } from 'vitest';
import { copySensitive, copyText, SENSITIVE_CLEAR_MS } from './clipboard';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('copyText', () => {
  it('copies the exact message text including lines and emoji', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal('navigator', { clipboard: { writeText } });
    vi.stubGlobal('window', { isSecureContext: true });

    await expect(copyText('First line\nSecond line 😀')).resolves.toBe(true);
    expect(writeText).toHaveBeenCalledWith('First line\nSecond line 😀');
  });

  it('reports failure instead of throwing', async () => {
    const writeText = vi.fn().mockRejectedValue(new Error('Clipboard unavailable'));
    vi.stubGlobal('navigator', { clipboard: { writeText } });
    vi.stubGlobal('window', { isSecureContext: true });

    await expect(copyText('Message')).resolves.toBe(false);
  });
});

describe('copySensitive', () => {
  it('clears the clipboard after thirty seconds, but only if the secret is still there', async () => {
    vi.useFakeTimers();
    let held = '';
    const writeText = vi.fn(async (v: string) => {
      held = v;
    });
    const readText = vi.fn(async () => held);
    vi.stubGlobal('navigator', { clipboard: { writeText, readText } });
    vi.stubGlobal('window', { isSecureContext: true, setTimeout: globalThis.setTimeout.bind(globalThis) });

    await copySensitive('six words here');
    await vi.advanceTimersByTimeAsync(SENSITIVE_CLEAR_MS);
    expect(held).toBe('');

    await copySensitive('another secret');
    held = 'something the person copied since';
    await vi.advanceTimersByTimeAsync(SENSITIVE_CLEAR_MS);
    expect(held).toBe('something the person copied since');
    vi.useRealTimers();
  });
});
