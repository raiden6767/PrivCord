import { useState } from 'react';
import { discover, MAX_SERVERS, MyServerError, type SavedServer } from '../lib/myServer';
import { loadMyServers, saveMyServers, serverInfo, setServerInfo } from '../lib/network';
import { refreshMyServers } from '../lib/myServerLoop';
import type { ChatEngineApi } from '../lib/engineApi';

/** One address (and a token if the server wants one): the relay, STUN and TURN come from the server itself. */
export function MyServerSettings({ engine }: { engine: ChatEngineApi | null }) {
  const [servers, setServers] = useState<SavedServer[]>(loadMyServers);
  const [address, setAddress] = useState('');
  const [token, setToken] = useState('');
  const [needsToken, setNeedsToken] = useState(false);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [, bump] = useState(0);

  async function add(e: React.FormEvent) {
    e.preventDefault();
    setError('');
    setBusy(true);
    try {
      const found = await discover(address, token.trim(), (url, init) => fetch(url, init));
      const next = [...servers.filter((s) => s.base !== found.saved.base), found.saved].slice(-MAX_SERVERS);
      saveMyServers(next);
      setServerInfo(found.saved.base, found.info);
      setServers(next);
      setAddress('');
      setToken('');
      setNeedsToken(false);
      engine?.reloadNetwork();
    } catch (err) {
      if (err instanceof MyServerError && (err.kind === 'needs-token' || err.kind === 'unauthorized')) setNeedsToken(true);
      setError(err instanceof Error ? err.message : 'Could not add that server.');
    } finally {
      setBusy(false);
    }
  }

  function remove(base: string) {
    const next = servers.filter((s) => s.base !== base);
    saveMyServers(next);
    setServers(next);
    engine?.reloadNetwork();
  }

  async function check() {
    setBusy(true);
    await refreshMyServers(true);
    engine?.reloadNetwork();
    bump((n) => n + 1);
    setBusy(false);
  }

  return (
    <div className="field">
      <strong>Use my server</strong>
      <p className="muted small">
        Type the address of a PrivCord server you or a friend runs. The relay and call servers come from it, so there is
        nothing else to fill in. Your messages stay sealed: the server only carries them.
      </p>
      {servers.map((s) => {
        const info = serverInfo(s.base);
        return (
          <div key={s.base} className="toggle-row">
            <div>
              <strong>{s.name}</strong>
              <p className="muted small" role="status">
                {info?.error ? info.error : info ? `Connected. ${info.turn ? 'Calls can use its relay.' : 'Messages only.'}` : 'Checking…'}
              </p>
            </div>
            <button type="button" className="button secondary small" onClick={() => remove(s.base)} aria-label={`Remove ${s.name}`}>
              Remove
            </button>
          </div>
        );
      })}
      {servers.length > 0 && (
        <button type="button" className="button secondary small" onClick={() => void check()} disabled={busy}>
          Check again
        </button>
      )}
      {servers.length < MAX_SERVERS && (
        <form onSubmit={(e) => void add(e)}>
          <label className="field">
            <span>Server address</span>
            <input value={address} onChange={(e) => setAddress(e.target.value)} placeholder="relay.example.com" autoCapitalize="off" spellCheck={false} aria-invalid={Boolean(error)} />
          </label>
          {(needsToken || token) && (
            <label className="field">
              <span>Access token</span>
              <input type="password" value={token} onChange={(e) => setToken(e.target.value)} autoComplete="off" spellCheck={false} />
            </label>
          )}
          {error && (
            <p className="error small" role="alert">
              {error}
            </p>
          )}
          <button type="submit" className="button primary small" disabled={busy || !address.trim()}>
            {busy ? 'Looking…' : 'Add server'}
          </button>
        </form>
      )}
    </div>
  );
}
