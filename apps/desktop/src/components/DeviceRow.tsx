import { useState } from 'react';
import type { DeviceInfo } from '../lib/engine';
import type { ChatEngineApi } from '../lib/engineApi';

function when(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/**
 * One device of the account in Settings › Devices, with removal (docs/design/syncengine.md §2): asked for from any
 * device, it happens in 24 hours unless taken back; with the recovery phrase, at once.
 */
export function DeviceRow({ device, engine }: { device: DeviceInfo; engine: ChatEngineApi | null }) {
  const [open, setOpen] = useState(false);
  const [phrase, setPhrase] = useState('');
  const [passphrase, setPassphrase] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const canRemove = !device.thisDevice && !!engine?.requestDeviceRemoval;

  const run = async (action: () => Promise<void> | undefined) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      setOpen(false);
      setPhrase('');
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <li className="device-row">
      <div className="device-row-main">
        <span>
          {device.name}
          {device.thisDevice ? <span className="muted small"> (this device)</span> : null}
        </span>
        <span className="muted small">since {new Date(device.addedAt).toLocaleDateString()}</span>
      </div>
      {device.removalAt ? (
        <div className="device-row-note">
          <span className="small">Will be removed {when(device.removalAt)}.</span>
          <button
            type="button"
            className="button ghost small"
            disabled={busy}
            onClick={() => void run(() => engine?.cancelDeviceRemoval?.(device.id))}
          >
            Keep it
          </button>
        </div>
      ) : device.stale && !device.thisDevice ? (
        <p className="muted small device-row-note">
          Not seen for 90 days{device.lastSeen ? ` (last ${new Date(device.lastSeen).toLocaleDateString()})` : ''}. Remove
          it, or open the app on it to reconnect.
        </p>
      ) : null}
      {canRemove && !device.removalAt && !open ? (
        <div className="button-row">
          <button type="button" className="button ghost small" onClick={() => setOpen(true)}>
            Remove…
          </button>
        </div>
      ) : null}
      {open ? (
        <div className="device-remove">
          <p className="small">
            {device.name} stops getting your messages and is told to erase everything on it the next time it connects
            (that cannot be guaranteed if it never does). Your chats on your other devices stay as they are.
          </p>
          <div className="button-row">
            <button
              type="button"
              className="button danger"
              disabled={busy}
              onClick={() => void run(() => engine?.requestDeviceRemoval?.(device.id))}
            >
              Remove in 24 hours
            </button>
            <button type="button" className="button ghost" disabled={busy} onClick={() => setOpen(false)}>
              Cancel
            </button>
          </div>
          <p className="muted small">
            Every device sees the request and can take it back for a day, so a stolen device cannot quietly remove the
            others. To remove it right away, enter your recovery phrase (it is not kept).
          </p>
          <label className="field">
            <span className="small">Recovery phrase</span>
            <textarea
              rows={2}
              value={phrase}
              autoComplete="off"
              spellCheck={false}
              onChange={(e) => setPhrase(e.target.value)}
            />
          </label>
          <label className="field">
            <span className="small">Passphrase (only if your account was set up with one)</span>
            <input
              type="password"
              value={passphrase}
              autoComplete="off"
              onChange={(e) => setPassphrase(e.target.value)}
            />
          </label>
          <div className="button-row">
            <button
              type="button"
              className="button danger"
              disabled={busy || phrase.trim().split(/\s+/).length < 12}
              onClick={() => void run(() => engine?.removeDeviceNow?.(device.id, phrase.trim(), passphrase))}
            >
              Remove now
            </button>
          </div>
          {error ? <p className="error-text small">{error}</p> : null}
        </div>
      ) : null}
      {error && !open ? <p className="error-text small">{error}</p> : null}
    </li>
  );
}
