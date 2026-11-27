/**
 * The two blobs of settings the app keeps ("settings": theme, your name, panic wipe…; "network": relay and TURN settings
 * with the password you typed). In the desktop app they are saved by the shell, encrypted under a key held in the
 * operating system's credential store, and never touch the webview's plain-text local storage. The screens read them
 * synchronously from a copy in memory that `loadSealedSettings` fills once at start. In a plain browser (tests, the dev
 * server) there is no shell, and they stay in local storage.
 */

const KEYS = { settings: 'duochat.settings.v1', network: 'duochat.network.v1', servers: 'privcord.servers.v1' } as const;
export type SealedName = keyof typeof KEYS;

const copy = new Map<SealedName, string | null>();

function inShell(): boolean {
  return typeof window !== 'undefined' && Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__);
}

function readLocal(name: SealedName): string | null {
  try {
    return localStorage.getItem(KEYS[name]);
  } catch {
    return null;
  }
}

function writeLocal(name: SealedName, text: string | null): void {
  try {
    if (text === null) localStorage.removeItem(KEYS[name]);
    else localStorage.setItem(KEYS[name], text);
  } catch {
    // Storage unavailable: the settings simply are not kept.
  }
}

async function shell<T>(command: string, args: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<T>(command, args);
}

/** Call once before anything reads settings. Moves a plain-text copy left by an older version into the encrypted file. */
export async function loadSealedSettings(): Promise<void> {
  for (const name of Object.keys(KEYS) as SealedName[]) {
    const legacy = readLocal(name);
    if (!inShell()) {
      copy.set(name, legacy);
      continue;
    }
    let value: string | null = null;
    try {
      value = await shell<string | null>('settings_get', { name });
    } catch {
      value = null;
    }
    if (value === null && legacy !== null) {
      value = legacy;
      try {
        await shell('settings_set', { name, text: legacy });
        writeLocal(name, null);
      } catch {
        // Could not be encrypted (no credential store): keep using it, and leave the old copy where it was.
      }
    } else if (value !== null && legacy !== null) {
      writeLocal(name, null);
    }
    copy.set(name, value);
  }
}

export function getSealed(name: SealedName): string | null {
  if (copy.has(name)) return copy.get(name) ?? null;
  return inShell() ? null : readLocal(name);
}

export function setSealed(name: SealedName, text: string): void {
  copy.set(name, text);
  if (!inShell()) {
    writeLocal(name, text);
    return;
  }
  // Never written to local storage: if the shell cannot encrypt it, it is kept for this run only.
  void shell('settings_set', { name, text }).catch(() => undefined);
}
