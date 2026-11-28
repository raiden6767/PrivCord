import {
  DEFAULT_NETWORK_CONFIG,
  iceServersFor,
  parseNetworkConfig,
  type NetworkConfig,
  type TurnServer,
} from '@duochat/protocol';
import { getSealed, setSealed } from './sealedSettings';
import { overlay, parseSavedServers, strip, type SavedServer, type ServerInfo } from './myServer';

/** What each saved server last offered (not saved: the TURN login is short-lived). */
const learned = new Map<string, ServerInfo>();

export function loadMyServers(): SavedServer[] {
  try {
    return parseSavedServers(JSON.parse(getSealed('servers') ?? '[]'));
  } catch {
    return [];
  }
}

export function saveMyServers(servers: SavedServer[]): void {
  setSealed('servers', JSON.stringify(servers));
  void pushNativeConfig();
}

export function serverInfo(base: string): ServerInfo | null {
  return learned.get(base) ?? null;
}

export function setServerInfo(base: string, info: ServerInfo): void {
  learned.set(base, info);
  void pushNativeConfig();
}

const known = () => loadMyServers().map((saved) => ({ saved, info: learned.get(saved.base) ?? null }));

/**
 * The TURN relay this release was built with (VITE_PRIVCORD_TURN_URLS / _USERNAME / _CREDENTIAL at build time; see
 * docs/turn-server/README.md), or none. It relays encrypted traffic only when two devices cannot reach each other directly.
 */
export const BUILTIN_TURN: TurnServer | null = (() => {
  const env = import.meta.env as Record<string, string | undefined>;
  const urls = (env.VITE_PRIVCORD_TURN_URLS ?? '').split(',').map((u) => u.trim()).filter(Boolean);
  if (urls.length === 0) return null;
  return { urls, username: env.VITE_PRIVCORD_TURN_USERNAME ?? '', credential: env.VITE_PRIVCORD_TURN_CREDENTIAL ?? '' };
})();

/** Whether this window was opened through Tor (private mode); read from the app once at start. */
let torActive = false;

export async function loadTorMode(): Promise<boolean> {
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    torActive = (await invoke<boolean>('tor_mode')) === true;
  } catch {
    torActive = false;
  }
  return torActive;
}

export function torModeActive(): boolean {
  return torActive;
}

/** Turns private mode on or off. The app restarts to apply it. */
export async function setTorMode(on: boolean): Promise<void> {
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('set_tor_mode', { on });
}

export function loadNetworkConfig(): NetworkConfig {
  let config: NetworkConfig;
  try {
    config = parseNetworkConfig(getSealed('network'));
  } catch {
    config = structuredClone(DEFAULT_NETWORK_CONFIG);
  }
  config = overlay(config, known());
  return torActive ? { ...config, torMode: true } : config;
}

export function saveNetworkConfig(config: NetworkConfig): void {
  const { torMode: _ignored, ...saved } = strip(config, known());
  setSealed('network', JSON.stringify(saved));
  void pushNativeConfig();
}

/**
 * The desktop shell owns the relay sockets and shows notifications in the tray: it gets the network settings (as saved
 * here) and the notification choices, and keeps them for a start in the tray. Nothing happens outside the app.
 */
export async function pushNativeConfig(notify?: { enabled: boolean; previews: boolean }): Promise<void> {
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    const { torMode: _ignored, ...network } = loadNetworkConfig();
    let prefs = notify;
    if (!prefs) {
      const { loadSettings } = await import('./settings');
      const s = loadSettings();
      prefs = { enabled: s.notificationsEnabled, previews: s.notificationPreviews };
    }
    await invoke('engine_network', { network: JSON.stringify(network), notify: prefs.enabled, previews: prefs.previews });
  } catch {
    // Not in the desktop app (browser preview, tests).
  }
}

export function rtcConfigFor(config: NetworkConfig): RTCConfiguration {
  return {
    iceServers: iceServersFor(config, BUILTIN_TURN),
    iceTransportPolicy: config.forceRelay ? 'relay' : 'all',
  };
}
