import { useEffect, useState } from 'react';
import type { ChatState } from '../lib/engine';
import { NO_PREFS } from '../lib/engine';
import type { ChatEngineApi } from '../lib/engineApi';
import { activeMutes, engineSignaling, markWhatsNewSeen, quietNow } from '../lib/insightInputs';
import { loadNetworkConfig, torModeActive } from '../lib/network';
import { config, isTauri } from '../config';
import { Modal } from './Modal';
import { AlertIcon, CheckIcon, LockIcon, XIcon } from './Icons';
import type { SettingsSection } from './SettingsDialog';

/** Ask a shared-engine insight; null while loading, when the engine has none (browser dev engine) or when it fails. */
function useInsight<T>(engine: ChatEngineApi | null, name: string, input: unknown, enabled = true): T | null {
  const [value, setValue] = useState<T | null>(null);
  const key = JSON.stringify(input);
  useEffect(() => {
    let live = true;
    if (!enabled || !engine?.insight) return;
    void engine.insight(name, input).then(
      (raw) => live && setValue(raw as T),
      () => undefined,
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [engine, name, key, enabled]);
  return value;
}

interface Lines {
  headline: string;
  lines: string[];
}

/** The small lock chip in the chat header: what protects this kind of chat, in the engine's words. */
export function SealedBadge({ engine, kind }: { engine: ChatEngineApi | null; kind: 'direct' | 'group' | 'channel' }) {
  const [open, setOpen] = useState(false);
  const info = useInsight<Lines>(engine, 'chat_security', kind, open);
  if (!engine?.insight) return null;
  const label = kind === 'direct' ? 'Person' : kind === 'group' ? 'Group' : 'Channel';
  return (
    <>
      <button type="button" className="sealed-chip" onClick={() => setOpen(true)} aria-label={`${label} chat: how it is protected`}>
        <LockIcon size={12} />
        {label}
      </button>
      <Modal open={open} onClose={() => setOpen(false)} title="How this chat is protected">
        {info ? (
          <div className="settings-section">
            <p><strong>{info.headline}</strong></p>
            <ul className="plain-list">
              {info.lines.map((l) => (
                <li key={l} className="muted small">{l}</li>
              ))}
            </ul>
          </div>
        ) : (
          <p className="muted small">Loading…</p>
        )}
      </Modal>
    </>
  );
}

interface Health {
  items: Array<{ id: string; ok: boolean; title: string; hint: string; fix?: string | null }>;
  verdict: { ok: boolean; id: string; text: string; fix?: string | null };
}

/** Settings > Notifications: is everything in place for messages to reach this device? */
export function DeliveryHealth({
  engine,
  state,
  notificationsEnabled,
  go,
}: {
  engine: ChatEngineApi | null;
  state: ChatState;
  notificationsEnabled: boolean;
  go: (section: SettingsSection) => void;
}) {
  const [background, setBackground] = useState<boolean | null>(null);
  const [why, setWhy] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    void import('@tauri-apps/api/core')
      .then(({ invoke }) => invoke<{ closeToTray: boolean }>('background_settings'))
      .then((v) => setBackground(v.closeToTray))
      .catch(() => undefined);
  }, []);
  const prefs = state.prefs ?? NO_PREFS;
  const input = {
    platform: 'windows',
    signaling: engineSignaling(state.signaling),
    notifications_enabled: notificationsEnabled,
    notification_permission: null,
    battery_unrestricted: null,
    background_allowed: background,
    quiet_hours_now: quietNow(prefs.quiet),
    muted_chats: activeMutes(prefs.muteUntil),
  };
  const health = useInsight<Health>(engine, 'health', input);
  if (!health) return null;
  const fix = (id: string | null | undefined) => {
    if (id === 'background_settings') document.getElementById('keep-running')?.scrollIntoView({ block: 'center', behavior: 'smooth' });
    else if (id === 'connection_settings') go('connection');
  };
  return (
    <div className="card-box checkup">
      <div className="card-title">
        <strong>Delivery health</strong>
      </div>
      <ul className="checkup-list">
        {health.items.map((item) => (
          <li key={item.id} className={item.ok ? 'ok' : 'todo'}>
            <span className="checkup-mark">{item.ok ? <CheckIcon size={15} /> : <AlertIcon size={15} />}</span>
            <span className="checkup-text">
              <strong>{item.title}</strong>
              <span className="muted small">{item.hint}</span>
            </span>
            {!item.ok && item.fix ? (
              <button type="button" className="button ghost small" onClick={() => fix(item.fix)}>
                Fix
              </button>
            ) : null}
          </li>
        ))}
      </ul>
      <div className="button-row">
        <button type="button" className="button secondary small" aria-expanded={why} onClick={() => setWhy(!why)}>
          Why am I not receiving?
        </button>
      </div>
      {why ? (
        <p className="small" role="status">
          {health.verdict.text}{' '}
          {!health.verdict.ok && health.verdict.fix ? (
            <button type="button" className="button ghost small" onClick={() => fix(health.verdict.fix)}>
              Fix
            </button>
          ) : null}
        </p>
      ) : null}
    </div>
  );
}

/** One line at the top of Settings > Devices. */
export function DevicesStrip({ engine, state }: { engine: ChatEngineApi | null; state: ChatState }) {
  const devices = (state.devices ?? []).map((d) => ({ name: d.name, last_seen: d.lastSeen ?? null, this_device: d.thisDevice, stale: d.stale === true }));
  const sum = useInsight<{ line: string }>(engine, 'devices_summary', { devices, now_ms: Math.floor(Date.now() / 60_000) * 60_000 }, devices.length > 0);
  return sum ? <p className="devices-strip">{sum.line}</p> : null;
}

export function WhatsNew({ engine, onDismiss }: { engine: ChatEngineApi | null; onDismiss?: () => void }) {
  const w = useInsight<{ version: string; lines: string[] }>(engine, 'whats_new', config.appVersion);
  if (!w || w.lines.length === 0) return null;
  return (
    <div className={onDismiss ? 'whats-new banner' : 'card-box'}>
      <div className="card-title">
        <strong>What's new</strong>
        {onDismiss ? (
          <button type="button" className="icon-button" aria-label="Dismiss" onClick={onDismiss}>
            <XIcon size={14} />
          </button>
        ) : null}
      </div>
      <ul className="plain-list">
        {w.lines.map((l) => (
          <li key={l} className="muted small">{l}</li>
        ))}
      </ul>
    </div>
  );
}

/** Banner at the top of the chat list, once after an update. */
export function WhatsNewBanner({ engine }: { engine: ChatEngineApi | null }) {
  const [shown, setShown] = useState(true);
  if (!shown || !engine?.insight) return null;
  return (
    <WhatsNew
      engine={engine}
      onDismiss={() => {
        markWhatsNewSeen(config.appVersion);
        setShown(false);
      }}
    />
  );
}

/** Settings > About: diagnostics file and the self-test. Both hidden when the engine cannot do them. */
export function SupportTools({ engine }: { engine: ChatEngineApi | null }) {
  const [tests, setTests] = useState<Array<{ name: string; ok: boolean; detail: string }> | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  if (!engine?.diagnostics && !engine?.selfTest) return null;

  async function save() {
    setNote(null);
    try {
      const net = loadNetworkConfig();
      const text = await engine!.diagnostics!({
        version: config.appVersion,
        platform: 'windows',
        transport_mode: net.transportMode ?? 'AUTOMATIC',
        relay_count: net.signaling.reduce((n, s) => n + (s.kind === 'relay' ? 1 : s.kind === 'nostr' ? s.relays.length : 0), 0),
        tor: torModeActive(),
      });
      const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
      const a = document.createElement('a');
      a.href = url;
      a.download = `duocord-diagnostics-${new Date().toISOString().slice(0, 10)}.json`;
      document.body.appendChild(a);
      a.click();
      a.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 10_000);
      setNote('Report saved.');
    } catch (e) {
      setNote(`Could not make the report: ${String((e as Error)?.message ?? e)}`);
    }
  }

  async function run() {
    setBusy(true);
    try {
      setTests(await engine!.selfTest!());
    } catch {
      setTests([{ name: 'Self-test', ok: false, detail: 'Could not run.' }]);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card-box">
      <div className="card-title">
        <strong>Support</strong>
      </div>
      <div className="button-row">
        {engine.diagnostics ? (
          <button type="button" className="button secondary small" onClick={() => void save()}>
            Save a diagnostics report
          </button>
        ) : null}
        {engine.selfTest ? (
          <button type="button" className="button secondary small" disabled={busy} onClick={() => void run()}>
            Run a self-test
          </button>
        ) : null}
      </div>
      <p className="muted small">The report contains counts only: no messages, names or keys.</p>
      {note ? <p className="small" role="status">{note}</p> : null}
      {tests ? (
        <ul className="checkup-list" aria-label="Self-test results">
          {tests.map((t) => (
            <li key={t.name} className={t.ok ? 'ok' : 'todo'}>
              <span className="checkup-mark">{t.ok ? <CheckIcon size={15} /> : <AlertIcon size={15} />}</span>
              <span className="checkup-text">
                <strong>{t.name}</strong>
                <span className="muted small">{t.detail}</span>
              </span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
