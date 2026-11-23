import type { NetworkConfig } from '@duochat/protocol';

export interface HostRow {
  host: string;
  /** What it is used for, in words. */
  why: string;
}

function hostOf(url: string): string {
  try {
    return new URL(url.replace(/^(stuns?|turns?):/i, 'https://')).host || url;
  } catch {
    return url.replace(/^(stuns?|turns?):/i, '').split('?')[0]!;
  }
}

/**
 * Every internet host this device may talk to for connecting, from the settings that decide it. The app has no analytics
 * and no other servers except GitHub for the optional update check (see lib/updates.ts); chats between friends go device
 * to device.
 */
export function networkHosts(n: NetworkConfig, updates = false): HostRow[] {
  const rows: HostRow[] = [];
  const add = (host: string, why: string) => {
    if (host && !rows.some((r) => r.host === host && r.why === why)) rows.push({ host, why });
  };
  const tor = n.torMode === true;
  if (n.transportMode !== 'BLUETOOTH_ONLY') {
    for (const s of n.signaling) {
      if (s.kind === 'nostr') {
        for (const r of s.relays) {
          add(hostOf(r), tor ? 'Finds your friend and passes sealed hellos (Nostr relay, reached through Tor)' : 'Finds your friend and passes sealed hellos (Nostr relay)');
        }
      } else {
        add(hostOf(s.url), tor ? 'Finds your friend and passes sealed messages (your own relay, reached through Tor)' : 'Finds your friend and passes sealed messages (your own relay)');
      }
    }
    // Private mode makes no direct connections, so no STUN or TURN server is ever contacted.
    if (!n.forceRelay && !tor) for (const u of n.stun) add(hostOf(u), 'Tells your device its public address so a direct connection can start (STUN)');
    if (!tor) for (const t of n.turn) for (const u of t.urls) add(hostOf(u), 'Carries encrypted traffic when a direct connection is not possible (TURN)');
  }
  // Whatever the connection mode, unless a relay or Tor hides this address (then the check is not made at all).
  if (updates && !n.forceRelay && !tor) {
    add('github.com', 'Looks for a new version and downloads it, while update checks are on. GitHub can see your IP address');
  }
  return rows;
}
