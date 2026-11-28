/** Small pure helpers that shape app state into the inputs of the shared engine's insights. The rules themselves live in the engine. */
import type { ChatState, PrefsView } from './chatTypes';
import type { SignalingStatus } from './signalingStatus';

/** The app's signaling status in the words the engine's `health` insight uses. */
export function engineSignaling(s: SignalingStatus): 'online' | 'connecting' | 'offline' | 'unconfigured' {
  return s === 'idle' ? 'unconfigured' : s === 'error' ? 'offline' : s;
}

/** Whether `now` falls inside the quiet hours (minutes after midnight; the window may cross midnight). */
export function quietNow(quiet: PrefsView['quiet'], now: Date = new Date()): boolean {
  if (!quiet.enabled || quiet.from === quiet.to) return false;
  const m = now.getHours() * 60 + now.getMinutes();
  return quiet.from < quiet.to ? m >= quiet.from && m < quiet.to : m >= quiet.from || m < quiet.to;
}

/** Chats whose mute has not run out yet. */
export function activeMutes(muteUntil: Record<string, number>, nowMs: number = Date.now()): number {
  return Object.values(muteUntil).filter((t) => t > nowMs).length;
}

/** Unread messages over friends, groups and channels. */
export function totalUnread(s: Pick<ChatState, 'contacts' | 'groups' | 'channels'>): number {
  const sum = (xs: Array<{ unread: number }>) => xs.reduce((n, x) => n + (x.unread || 0), 0);
  return sum(s.contacts) + sum(s.groups) + sum(s.channels);
}

export function titleFor(unread: number): string {
  return unread > 0 ? `(${unread > 99 ? '99+' : unread}) PrivCord` : 'PrivCord';
}

/** The tray tooltip: "PrivCord" or "PrivCord, 3 unread". */
export function tooltipFor(unread: number): string {
  return unread > 0 ? `PrivCord, ${unread > 99 ? '99+' : unread} unread` : 'PrivCord';
}

/** The number on the taskbar badge, capped at "9+" (empty for none). */
export function badgeText(unread: number): string {
  return unread <= 0 ? '' : unread > 9 ? '9+' : String(unread);
}

const SEEN_KEY = 'duochat.whatsNewSeen';

/** True when this version's What's new has not been dismissed yet. A first run (nothing stored) is not an update. */
export function whatsNewPending(version: string): boolean {
  try {
    const seen = localStorage.getItem(SEEN_KEY);
    if (seen === null) {
      localStorage.setItem(SEEN_KEY, version);
      return false;
    }
    return seen !== version;
  } catch {
    return false;
  }
}

export function markWhatsNewSeen(version: string): void {
  try {
    localStorage.setItem(SEEN_KEY, version);
  } catch {
    /* no storage */
  }
}

/** Whether friends are kept from seeing this device's address: a relay carries every link (it needs a TURN server to
 * carry them), or Private mode sends everything through Tor. A "hide my IP" switch with no relay behind it hides nothing. */
export function hidesIp(n: { forceRelay?: boolean; torMode?: boolean; turn: unknown[] }): boolean {
  return (n.forceRelay === true && n.turn.length > 0) || n.torMode === true;
}
