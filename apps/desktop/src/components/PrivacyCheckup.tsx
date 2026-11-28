import { useEffect, useState } from 'react';
import { loadNetworkConfig } from '../lib/network';
import { privacyCheckup, SECTION_OF, type CheckupItem } from '../lib/privacyCheckup';
import type { ChatEngineApi } from '../lib/engineApi';
import type { AppSettings } from '../lib/settings';
import type { ChatState } from '../lib/chatTypes';
import { isLockEnabled } from '../lib/appLock';
import { hidesIp } from '../lib/insightInputs';
import type { SettingsSection } from './SettingsDialog';
import { AlertIcon, CheckIcon } from './Icons';

export const LAST_BACKUP_KEY = 'duochat.lastBackupAt';

async function lastBackupAt(): Promise<number> {
  let best = 0;
  try {
    best = Number(localStorage.getItem(LAST_BACKUP_KEY)) || 0;
  } catch {
    /* no storage */
  }
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    const s = await invoke<{ lastAt: number }>('auto_backup_status');
    if (s.lastAt > best) best = s.lastAt;
  } catch {
    /* not the desktop app */
  }
  return best;
}

/** A short list of privacy basics, how many hold, and a button to fix each one. Everything is read on this device. */
export function PrivacyCheckup({
  engine,
  settings,
  state,
  go,
}: {
  engine: ChatEngineApi | null;
  settings: AppSettings;
  state: ChatState;
  go: (section: SettingsSection) => void;
}) {
  const [backupAt, setBackupAt] = useState(0);
  useEffect(() => {
    void lastBackupAt().then(setBackupAt);
  }, []);
  const input = {
    app_lock: isLockEnabled(),
    last_backup_at: backupAt,
    hide_ip: hidesIp(loadNetworkConfig()),
    previews_off: !settings.notificationPreviews,
    group_only: settings.groupOnly,
    panic_wipe: settings.panicWipe,
    stale_devices: (state.devices ?? []).filter((d) => d.stale).length,
    now_ms: Date.now(),
  };
  // The packaged app asks the shared engine, so Windows and Android give the same answer. The browser-only development
  // engine has no Rust behind it and uses the TypeScript copy of the same rules.
  const [items, setItems] = useState<CheckupItem[]>(() => privacyCheckup(input).items);
  const key = JSON.stringify({ ...input, now_ms: 0 });
  useEffect(() => {
    let live = true;
    if (!engine?.insight) {
      setItems(privacyCheckup(input).items);
      return;
    }
    void engine.insight('checkup', input).then(
      (raw) => {
        if (!live || !Array.isArray(raw)) return;
        setItems(raw.map((r) => ({ ...(r as Omit<CheckupItem, 'section'>), section: SECTION_OF[(r as { id: string }).id] ?? 'security' })));
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [engine, key]);
  const score = items.filter((x) => x.ok).length;
  const total = items.length;
  return (
    <div className="card-box checkup">
      <div className="card-title">
        <strong>Privacy checkup</strong>
        <span className="checkup-score" aria-label={`${score} of ${total} in place`}>
          {score}/{total}
        </span>
      </div>
      <div className="checkup-bar" role="presentation">
        <span style={{ width: `${(score / total) * 100}%` }} />
      </div>
      <ul className="checkup-list">
        {items.map((item) => (
          <li key={item.id} className={item.ok ? 'ok' : 'todo'}>
            <span className="checkup-mark">{item.ok ? <CheckIcon size={15} /> : <AlertIcon size={15} />}</span>
            <span className="checkup-text">
              <strong>{item.label}</strong>
              <span className="muted small">{item.hint}</span>
            </span>
            {item.ok ? null : (
              <button type="button" className="button ghost small" onClick={() => go(item.section)}>
                Fix
              </button>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
