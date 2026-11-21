import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
const inShell = vi.fn(() => true);
vi.mock('../config', () => ({ isTauri: () => inShell() }));

import { checkForUpdate, dueForCheck, installUpdate, mayCheckForUpdates, RECHECK_MS, snoozeActive, SNOOZE_MS } from './updates';
import { parseSettings } from './settings';

beforeEach(() => {
  invoke.mockReset();
  inShell.mockReturnValue(true);
});

describe('updates', () => {
  it('never checks while Hide my IP or Tor mode is on, or outside the desktop app', () => {
    expect(mayCheckForUpdates({ forceRelay: false, torMode: false })).toBe(true);
    expect(mayCheckForUpdates({ forceRelay: true, torMode: false })).toBe(false);
    expect(mayCheckForUpdates({ forceRelay: false, torMode: true })).toBe(false);
    inShell.mockReturnValue(false);
    expect(mayCheckForUpdates({ forceRelay: false, torMode: false })).toBe(false);
  });

  it('asks the shell to check and to install', async () => {
    invoke.mockResolvedValueOnce({ version: '0.1.9', notes: null });
    expect(await checkForUpdate()).toEqual({ version: '0.1.9', notes: null });
    expect(invoke).toHaveBeenCalledWith('update_check');
    invoke.mockResolvedValueOnce(undefined);
    await installUpdate();
    expect(invoke).toHaveBeenLastCalledWith('update_install');
  });

  it('keeps "Later" for three days', () => {
    expect(snoozeActive(1000 + SNOOZE_MS, 1000)).toBe(true);
    expect(snoozeActive(1000, 1000 + 1)).toBe(false);
  });

  it('repeats the automatic check at most every half hour', () => {
    expect(dueForCheck(0, 5 * RECHECK_MS)).toBe(true);
    expect(dueForCheck(1000, 1000 + RECHECK_MS - 1)).toBe(false);
    expect(dueForCheck(1000, 1000 + RECHECK_MS)).toBe(true);
  });

  it('checks for updates by default, and keeps the setting', () => {
    expect(parseSettings(null).updateChecks).toBe(true);
    expect(parseSettings(JSON.stringify({ updateChecks: false })).updateChecks).toBe(false);
    expect(parseSettings(JSON.stringify({ updateSnoozedUntil: 5 })).updateSnoozedUntil).toBe(5);
    expect(parseSettings(JSON.stringify({ updateSnoozedUntil: 'x' })).updateSnoozedUntil).toBe(0);
  });
});
