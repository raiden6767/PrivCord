import { refresh, needsRefresh } from './myServer';
import { loadMyServers, serverInfo, setServerInfo } from './network';

/** Asks every saved server for what it offers when that is due. Returns whether anything changed. */
export async function refreshMyServers(force = false): Promise<boolean> {
  let changed = false;
  for (const saved of loadMyServers()) {
    const before = serverInfo(saved.base);
    if (!force && !needsRefresh(before)) continue;
    const info = await refresh(saved, before, (url, init) => fetch(url, init));
    setServerInfo(saved.base, info);
    changed = true;
  }
  return changed;
}

/** Looks now and then every few minutes; `onChange` runs after a new answer so the engine can pick it up. */
export function watchMyServers(onChange: () => void): () => void {
  let stopped = false;
  const tick = async () => {
    if (!stopped && (await refreshMyServers())) onChange();
  };
  void tick();
  const id = setInterval(() => void tick(), 5 * 60_000);
  return () => {
    stopped = true;
    clearInterval(id);
  };
}
