/**
 * In-app updates (the Windows app only; a plain browser has nothing to update). The shell does the work: it reads the
 * signed `latest.json` of our release and checks the installer's signature. A check contacts GitHub directly, so it is
 * never made while Hide my IP or Tor mode is on.
 */
import { isTauri } from '../config';

export interface FoundUpdate {
  version: string;
  notes: string | null;
}

/** "Later" hides the prompt for this long (every merged change is a new build, so it must not nag). */
export const SNOOZE_MS = 3 * 24 * 60 * 60 * 1000;

export function mayCheckForUpdates(network: { forceRelay: boolean; torMode?: boolean }): boolean {
  return isTauri() && !network.forceRelay && !network.torMode;
}

/** Null when this is the newest build. Throws if the update server cannot be reached or its answer is not trusted. */
export async function checkForUpdate(): Promise<FoundUpdate | null> {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<FoundUpdate | null>('update_check');
}

/** Downloads, verifies and starts the installer; the app closes and comes back as the new version. */
export async function installUpdate(): Promise<void> {
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('update_install');
}

/** The automatic check repeats at most this often (it runs at start, when the window comes back, and on a timer). */
export const RECHECK_MS = 30 * 60 * 1000;

export function dueForCheck(lastDoneAt: number, now: number = Date.now()): boolean {
  return now - lastDoneAt >= RECHECK_MS;
}

export function snoozeActive(snoozedUntil: number, now: number = Date.now()): boolean {
  return now < snoozedUntil;
}
