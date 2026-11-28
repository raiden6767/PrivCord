/**
 * "Use my server": one address (and a token, when the server asks for one) is all a person types. Everything else comes from
 * the server itself: where its relay is, which STUN/TURN addresses it offers, and a short-lived TURN login that is fetched
 * again before it runs out. The result is laid over the saved network settings when they are read (never written into them),
 * so the settings form cannot save an old login back, and removing a server takes all of it away.
 *
 * What the server answers (see crates/privcord-server/src/net.rs):
 *   GET /.well-known/privcord-server -> { name: "privcord-server", requires_token, relay: {...}|null,
 *                                         turn: { urls: [...], credentials: "/turn", ttl_secs }|null, ... }
 *   GET /turn (Authorization: Bearer <token>) -> { urls, username, credential, ttl }
 */

import type { NetworkConfig } from '@duochat/protocol';

/** What is kept (encrypted, with the other settings): enough to find the server again. */
export interface SavedServer {
  /** `https://relay.example.com` or `http://192.168.1.5:8787`: no path. */
  base: string;
  token: string;
  /** The host, shown to the person. */
  name: string;
}

/** What was learned from the server: not saved (the login lasts an hour; it is fetched again at start). */
export interface ServerInfo {
  stun: string[];
  turn: { urls: string[]; username: string; credential: string; expiresAt: number } | null;
  relay: boolean;
  requiresToken: boolean;
  checkedAt: number;
  error: string | null;
}

export const MAX_SERVERS = 4;
/** The login is fetched again when less than this is left on it. */
export const REFRESH_BEFORE_MS = 15 * 60_000;
/** Whether a server still answers is looked at again after this long, login or not. */
export const RECHECK_MS = 6 * 3_600_000;
const TIMEOUT_MS = 8_000;

export class MyServerError extends Error {
  constructor(
    message: string,
    readonly kind: 'unreachable' | 'unauthorized' | 'not-privcord' | 'needs-token' | 'invalid',
  ) {
    super(message);
  }
}

/** The addresses to try for what a person typed: with a scheme as given, otherwise https first, then http. */
export function candidates(input: string): string[] {
  const text = input.trim();
  if (!text || text.length > 300 || /\s/.test(text)) return [];
  const m = /^([a-z][a-z0-9+.-]*):\/\/(.*)$/i.exec(text);
  const scheme = m ? m[1]!.toLowerCase() : null;
  const rest = m ? m[2]! : text;
  const host = rest.split(/[/?#]/)[0]!;
  if (!host || host.includes('@')) return [];
  if (scheme === null) return [`https://${host}`, `http://${host}`];
  if (scheme === 'https' || scheme === 'wss') return [`https://${host}`];
  if (scheme === 'http' || scheme === 'ws') return [`http://${host}`];
  return [];
}

/** Plain http is for this machine and private networks only: a token must never cross the internet unencrypted. */
export function isPrivateHost(base: string): boolean {
  const host = base.replace(/^https?:\/\//i, '').replace(/:\d+$/, '').replace(/^\[|\]$/g, '').toLowerCase();
  if (host === 'localhost' || host.endsWith('.local') || host.endsWith('.localhost') || host === '::1') return true;
  const m = /^(\d+)\.(\d+)\.\d+\.\d+$/.exec(host);
  if (!m) return false;
  const [a, b] = [Number(m[1]), Number(m[2])];
  return a === 10 || a === 127 || (a === 192 && b === 168) || (a === 172 && b >= 16 && b <= 31) || (a === 100 && b >= 64 && b <= 127);
}

/** The relay's WebSocket address for a base address (no token). */
export function wsBase(base: string): string {
  return base.replace(/^http/i, 'ws');
}

/** The relay address the apps connect to, with the token on it when there is one (the way the server reads it). */
export function relayUrl(s: SavedServer): string {
  return s.token ? `${wsBase(s.base)}/?token=${encodeURIComponent(s.token)}` : wsBase(s.base);
}

/** A server's TURN address without `?transport=` is also its STUN address (the same port answers both). */
export function stunFromTurn(urls: string[]): string[] {
  return urls.filter((u) => /^turn:[^\s?]+$/i.test(u)).map((u) => u.replace(/^turn:/i, 'stun:'));
}

export function parseSavedServers(raw: unknown): SavedServer[] {
  if (!Array.isArray(raw)) return [];
  const out: SavedServer[] = [];
  for (const item of raw) {
    const o = (item ?? {}) as Record<string, unknown>;
    const base = typeof o.base === 'string' ? candidates(o.base)[0] : undefined;
    if (!base || out.some((s) => s.base === base)) continue;
    const token = typeof o.token === 'string' ? o.token.slice(0, 512) : '';
    const name = typeof o.name === 'string' && o.name.trim() ? o.name.trim().slice(0, 80) : base.replace(/^https?:\/\//, '');
    out.push({ base, token, name });
    if (out.length >= MAX_SERVERS) break;
  }
  return out;
}

export type FetchLike = (
  url: string,
  init?: { headers?: Record<string, string>; signal?: AbortSignal },
) => Promise<{ ok: boolean; status: number; json(): Promise<unknown> }>;

async function getJson(fetchImpl: FetchLike, url: string, token: string): Promise<unknown> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), TIMEOUT_MS);
  try {
    const res = await fetchImpl(url, { headers: token ? { Authorization: `Bearer ${token}` } : {}, signal: controller.signal });
    if (res.status === 401) {
      throw token
        ? new MyServerError('The server did not accept the token.', 'unauthorized')
        : new MyServerError('This server needs an access token. Ask the person who runs it.', 'needs-token');
    }
    if (!res.ok) throw new MyServerError(`The server answered ${res.status}.`, 'not-privcord');
    try {
      return await res.json();
    } catch {
      throw new MyServerError('That address answers, but it does not look like a PrivCord server.', 'not-privcord');
    }
  } catch (e) {
    if (e instanceof MyServerError) throw e;
    throw new MyServerError('Could not reach that address. Check it, and that the server is running.', 'unreachable');
  } finally {
    clearTimeout(timer);
  }
}

interface About {
  requires_token?: boolean;
  relay?: unknown;
  turn?: { urls?: unknown; credentials?: unknown; ttl_secs?: unknown } | null;
}

function parseAbout(v: unknown): About | null {
  const o = v as Record<string, unknown> | null;
  return o && typeof o === 'object' && o.name === 'privcord-server' ? (o as About) : null;
}

const strings = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string' && x.length > 0 && x.length < 300) : []);

async function loadTurn(fetchImpl: FetchLike, base: string, token: string, path: string, now: number): Promise<ServerInfo['turn']> {
  const where = path.startsWith('/') ? path : '/turn';
  const t = (await getJson(fetchImpl, `${base}${where}`, token)) as Record<string, unknown>;
  const urls = strings(t.urls);
  const username = typeof t.username === 'string' ? t.username : '';
  const credential = typeof t.credential === 'string' ? t.credential : '';
  const ttl = typeof t.ttl === 'number' && t.ttl > 0 ? t.ttl : 3600;
  if (urls.length === 0 || !username || !credential) {
    throw new MyServerError('The server sent a TURN login that cannot be used.', 'invalid');
  }
  return { urls, username, credential, expiresAt: now + ttl * 1000 };
}

/** Looks the server up from what was typed. Throws a [`MyServerError`] in words a person can act on. */
export async function discover(
  input: string,
  token: string,
  fetchImpl: FetchLike,
  now = Date.now(),
): Promise<{ saved: SavedServer; info: ServerInfo }> {
  const tries = candidates(input);
  if (tries.length === 0) throw new MyServerError('Type the server’s address, for example relay.example.com.', 'invalid');
  let last: MyServerError | null = null;
  for (const base of tries) {
    try {
      if (token && base.startsWith('http://') && !isPrivateHost(base)) {
        throw new MyServerError('An access token is only sent over https. Use an https address (or a private network address).', 'invalid');
      }
      const about = parseAbout(await getJson(fetchImpl, `${base}/.well-known/privcord-server`, token));
      if (!about) throw new MyServerError('That address answers, but it is not a PrivCord server.', 'not-privcord');
      if (about.requires_token === true && !token) {
        throw new MyServerError('This server needs an access token. Ask the person who runs it.', 'needs-token');
      }
      const turnPath = typeof about.turn?.credentials === 'string' ? about.turn.credentials : '/turn';
      const turn = about.turn ? await loadTurn(fetchImpl, base, token, turnPath, now) : null;
      const saved: SavedServer = { base, token, name: base.replace(/^https?:\/\//, '') };
      return {
        saved,
        info: { stun: turn ? stunFromTurn(turn.urls) : [], turn, relay: Boolean(about.relay), requiresToken: about.requires_token === true, checkedAt: now, error: null },
      };
    } catch (e) {
      last = e instanceof MyServerError ? e : new MyServerError('Could not reach that address.', 'unreachable');
      // A wrong token or a missing one is the answer: another scheme would not change it.
      if (last.kind === 'unauthorized' || last.kind === 'needs-token') break;
    }
  }
  throw last ?? new MyServerError('Could not reach that address.', 'unreachable');
}

/** Asks a saved server again (what it offers and a new login). On a failure the old answer stays usable and says why. */
export async function refresh(saved: SavedServer, previous: ServerInfo | null, fetchImpl: FetchLike, now = Date.now()): Promise<ServerInfo> {
  try {
    const found = await discover(saved.base, saved.token, fetchImpl, now);
    return found.info;
  } catch (e) {
    const why = e instanceof Error ? e.message : 'Could not reach the server.';
    return previous ? { ...previous, error: why } : { stun: [], turn: null, relay: true, requiresToken: false, checkedAt: now, error: why };
  }
}

export function needsRefresh(info: ServerInfo | null, now = Date.now()): boolean {
  if (!info) return true;
  if (info.turn && info.turn.expiresAt - now < REFRESH_BEFORE_MS) return true;
  return now - info.checkedAt > (info.error ? 60_000 : RECHECK_MS);
}

type Known = { saved: SavedServer; info: ServerInfo | null };

/** The saved network settings with what the servers offer laid over them. */
export function overlay(net: NetworkConfig, servers: Known[]): NetworkConfig {
  if (servers.length === 0) return net;
  const out: NetworkConfig = { ...net, stun: [...net.stun], turn: [...net.turn], signaling: [...net.signaling] };
  for (const { saved, info } of servers) {
    const url = relayUrl(saved);
    // The relay is added from the saved address alone: it works before the first answer, and offline-first.
    if (!out.signaling.some((s) => s.kind === 'relay' && s.url === url)) out.signaling.push({ kind: 'relay', url });
    if (!info) continue;
    for (const s of info.stun) if (!out.stun.includes(s)) out.stun.unshift(s);
    if (info.turn && info.turn.expiresAt > Date.now()) {
      out.turn.push({ urls: info.turn.urls, username: info.turn.username, credential: info.turn.credential });
    }
  }
  return out;
}

/** Takes the servers' own entries out again (what the settings form hands back must not be saved as if typed by hand). */
export function strip(net: NetworkConfig, servers: Known[]): NetworkConfig {
  if (servers.length === 0) return net;
  const bases = servers.map((s) => wsBase(s.saved.base));
  const stun = new Set(servers.flatMap((s) => s.info?.stun ?? []));
  const turnUrls = new Set(servers.flatMap((s) => s.info?.turn?.urls ?? []));
  const turnHost = (u: string) => turnUrls.has(u);
  return {
    ...net,
    stun: net.stun.filter((u) => !stun.has(u)),
    turn: net.turn.filter((t) => !t.urls.some(turnHost)),
    signaling: net.signaling.filter((s) => !(s.kind === 'relay' && bases.some((b) => s.url === b || s.url.startsWith(`${b}/`)))),
  };
}
