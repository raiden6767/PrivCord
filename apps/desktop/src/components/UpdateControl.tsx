import { useEffect, useState } from 'react';
import { loadNetworkConfig } from '../lib/network';
import type { AppSettings } from '../lib/settings';
import { RECHECK_MS, SNOOZE_MS, checkForUpdate, dueForCheck, installUpdate, mayCheckForUpdates, snoozeActive, type FoundUpdate } from '../lib/updates';

/** Installs a found update: the app closes and comes back as the new version. */
function InstallButton({ found }: { found: FoundUpdate }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <>
      <button
        type="button"
        className="button primary small"
        disabled={busy}
        onClick={() => {
          setBusy(true);
          setError(null);
          installUpdate().catch((e: unknown) => {
            setBusy(false);
            setError(String((e as Error)?.message ?? e));
          });
        }}
      >
        {busy ? 'Downloading…' : `Install ${found.version}`}
      </button>
      {error ? <p className="muted small">{error}</p> : null}
    </>
  );
}

/** Settings (About): "Check for updates now" with its result. The switch itself lives in the dialog. */
export function UpdateCheck() {
  const allowed = mayCheckForUpdates(loadNetworkConfig());
  const [checking, setChecking] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [found, setFound] = useState<FoundUpdate | null>(null);
  if (!allowed) {
    return <p className="muted small">Turn off Hide my IP and Tor mode to check for updates: a check contacts GitHub directly.</p>;
  }
  return (
    <>
      <div className="button-row">
        <button
          type="button"
          className="button secondary small"
          disabled={checking}
          onClick={() => {
            setChecking(true);
            setStatus(null);
            setFound(null);
            checkForUpdate()
              .then((update) => {
                setFound(update);
                setStatus(update ? `Version ${update.version} is available.` : 'You are up to date.');
              })
              .catch(() => setStatus('Could not check for updates. Try again later.'))
              .finally(() => setChecking(false));
          }}
        >
          {checking ? 'Checking…' : 'Check for updates now'}
        </button>
        {found ? <InstallButton found={found} /> : null}
      </div>
      {status ? <p className="muted small">{status}</p> : null}
    </>
  );
}

/**
 * A quiet banner when a newer build exists. The app usually stays running in the tray, so one look at start is not enough:
 * it looks again whenever the window comes back to the front, and on a timer, at most every half hour. A look that fails
 * (no network yet) is tried again next time. "Later" is remembered for three days.
 */
export function UpdateBanner({ settings, updateSettings }: { settings: AppSettings; updateSettings: (patch: Partial<AppSettings>) => void }) {
  const [found, setFound] = useState<FoundUpdate | null>(null);
  const wanted = settings.updateChecks && !snoozeActive(settings.updateSnoozedUntil);
  useEffect(() => {
    if (!wanted) return;
    let live = true;
    let lastDone = 0;
    const look = () => {
      if (!dueForCheck(lastDone) || !mayCheckForUpdates(loadNetworkConfig())) return;
      const started = Date.now();
      checkForUpdate()
        .then((update) => {
          lastDone = started;
          if (live && update) setFound(update);
        })
        .catch(() => undefined);
    };
    const back = () => {
      if (document.visibilityState === 'visible') look();
    };
    look();
    document.addEventListener('visibilitychange', back);
    window.addEventListener('focus', back);
    const timer = window.setInterval(look, RECHECK_MS);
    return () => {
      live = false;
      document.removeEventListener('visibilitychange', back);
      window.removeEventListener('focus', back);
      window.clearInterval(timer);
    };
  }, [wanted]);
  if (!found) return null;
  return (
    <div className="banner info update-float" role="status">
      <span>PrivCord {found.version} is ready.</span>
      <InstallButton found={found} />
      <button
        type="button"
        className="button secondary small"
        onClick={() => {
          setFound(null);
          updateSettings({ updateSnoozedUntil: Date.now() + SNOOZE_MS });
        }}
      >
        Later
      </button>
    </div>
  );
}
