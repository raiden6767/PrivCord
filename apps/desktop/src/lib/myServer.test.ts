import { describe, expect, it } from 'vitest';
import { DEFAULT_NETWORK_CONFIG } from '@duochat/protocol';
import { candidates, discover, needsRefresh, overlay, parseSavedServers, refresh, relayUrl, strip, stunFromTurn, type FetchLike } from './myServer';

// Taken from a running privcord-server (GET /.well-known/privcord-server and GET /turn).
const ABOUT = {
  name: 'privcord-server',
  requires_token: true,
  relay: { path: '/' },
  turn: { urls: ['turn:203.0.113.7:13578', 'turn:203.0.113.7:13579?transport=tcp'], credentials: '/turn', ttl_secs: 3600 },
};
const TURN = { urls: ABOUT.turn.urls, username: '1795810950:48e2296c', credential: 'c2VjcmV0', ttl: 3600 };

function server(opts: { token?: string; about?: unknown } = {}): FetchLike {
  return async (url, init) => {
    const auth = init?.headers?.Authorization ?? '';
    const ok = !opts.token || auth === `Bearer ${opts.token}`;
    const path = new URL(url).pathname;
    if (!ok) return { ok: false, status: 401, json: async () => ({}) };
    if (path === '/.well-known/privcord-server') return { ok: true, status: 200, json: async () => opts.about ?? ABOUT };
    if (path === '/turn') return { ok: true, status: 200, json: async () => TURN };
    return { ok: false, status: 404, json: async () => ({}) };
  };
}

describe('my server', () => {
  it('tries https then http for a bare address, and only what was typed otherwise', () => {
    expect(candidates('relay.example.com')).toEqual(['https://relay.example.com', 'http://relay.example.com']);
    expect(candidates('http://10.0.0.5:8787/x')).toEqual(['http://10.0.0.5:8787']);
    expect(candidates('wss://r.example.com/?a=1')).toEqual(['https://r.example.com']);
    expect(candidates('ftp://x')).toEqual([]);
    expect(candidates('a b')).toEqual([]);
    expect(candidates('user@host')).toEqual([]);
  });

  it('derives STUN from the plain TURN address only', () => {
    expect(stunFromTurn(TURN.urls)).toEqual(['stun:203.0.113.7:13578']);
  });

  it('finds the server and its login', async () => {
    const { saved, info } = await discover('203.0.113.7:8787', 'tok', server({ token: 'tok' }), 1000);
    expect(saved.base).toBe('https://203.0.113.7:8787');
    expect(info.turn?.username).toBe(TURN.username);
    expect(info.turn?.expiresAt).toBe(1000 + 3600_000);
    expect(info.stun).toEqual(['stun:203.0.113.7:13578']);
  });

  it('says so when the token is missing or wrong', async () => {
    await expect(discover('r.example.com', '', server({ token: 'tok' }))).rejects.toMatchObject({ kind: 'needs-token' });
    await expect(discover('r.example.com', 'bad', server({ token: 'tok' }))).rejects.toMatchObject({ kind: 'unauthorized' });
  });

  it('says so when it is not a PrivCord server or does not answer', async () => {
    await expect(discover('r.example.com', '', server({ about: { name: 'nginx' } }))).rejects.toMatchObject({ kind: 'not-privcord' });
    const down: FetchLike = async () => {
      throw new Error('offline');
    };
    await expect(discover('r.example.com', '', down)).rejects.toMatchObject({ kind: 'unreachable' });
  });

  it('keeps the old answer when a refresh fails', async () => {
    const { saved, info } = await discover('r.example.com', 'tok', server({ token: 'tok' }), 1000);
    const down: FetchLike = async () => {
      throw new Error('offline');
    };
    const again = await refresh(saved, info, down, 2000);
    expect(again.turn).toEqual(info.turn);
    expect(again.error).toMatch(/reach/);
  });

  it('asks again before the login runs out', async () => {
    const { info } = await discover('r.example.com', 'tok', server({ token: 'tok' }), 0);
    expect(needsRefresh(info, 60_000)).toBe(false);
    expect(needsRefresh(info, 3600_000 - 60_000)).toBe(true);
    expect(needsRefresh(null)).toBe(true);
  });

  it('lays the server over the settings and takes it out again', async () => {
    const { saved, info } = await discover('r.example.com', 'tok', server({ token: 'tok' }), Date.now());
    const mine = DEFAULT_NETWORK_CONFIG;
    const merged = overlay(mine, [{ saved, info }]);
    expect(merged.signaling.some((s) => s.kind === 'relay' && s.url === relayUrl(saved))).toBe(true);
    expect(merged.turn.length).toBe(mine.turn.length + 1);
    expect(strip(merged, [{ saved, info }])).toEqual(mine);
    expect(overlay(mine, [])).toBe(mine);
  });

  it('drops bad saved entries and repeats', () => {
    expect(parseSavedServers([{ base: 'r.example.com', token: 1 }, { base: 'https://r.example.com' }, 5, null])).toHaveLength(1);
    expect(parseSavedServers('x')).toEqual([]);
  });
});
