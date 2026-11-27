import { useEffect, useRef, useState } from 'react';
import { disableLock, enableLock, isLockEnabled, lockSupported, unlock } from '../lib/appLock';
import type { ChatState } from '../lib/engine';
import type { ChatEngineApi } from '../lib/engineApi';
import type { AppSettings, ThemeMode } from '../lib/settings';
import { ACCENTS } from '../lib/settings';
import { THEMES, themeVars } from '../lib/themes';
import { ensureNotificationPermission } from '../lib/notifications';
import { config, isTauri } from '../config';
import { Modal } from './Modal';
import { NetworkSettings } from './NetworkSettings';
import { DeviceLinkDialog } from './DeviceLinkDialog';
import { DeviceRow } from './DeviceRow';
import { AutoBackupControl } from './AutoBackupControl';
import { LAST_BACKUP_KEY, PrivacyCheckup } from './PrivacyCheckup';
import { NetworkTransparency } from './NetworkTransparency';
import { UpdateCheck } from './UpdateControl';
import { StorageByChat } from './StorageByChat';
import { NO_PREFS } from '../lib/engine';
import { formatBytes } from '../lib/format';
import { DeliveryHealth, DevicesStrip, DiagLogCard, SupportTools, WhatsNew } from './Insights';
import { AlertIcon, CheckIcon, KeyIcon, TrashIcon } from './Icons';
import { MAX_LIVE_LINKS, MIN_BACKUP_PASSPHRASE, PANIC_PRESSES, PANIC_WINDOW_MS } from '../lib/limits';
import { RECHECK_MS } from '../lib/updates';
import { useAppVersion } from '../lib/appVersion';

export type SettingsSection = 'profile' | 'devices' | 'appearance' | 'notifications' | 'connection' | 'security' | 'backup' | 'about';

const SECTIONS: Array<{ id: SettingsSection; label: string }> = [
  { id: 'profile', label: 'Profile' },
  { id: 'devices', label: 'Devices' },
  { id: 'appearance', label: 'Appearance' },
  { id: 'notifications', label: 'Notifications' },
  { id: 'connection', label: 'Connection' },
  { id: 'security', label: 'Security' },
  { id: 'backup', label: 'Backup' },
  { id: 'about', label: 'About' },
];

interface BackgroundSettings {
  closeToTray: boolean;
  startAtLogin: boolean;
}

/** Desktop only: keep running in the tray when the window closes, and start at sign-in (in the tray). */
function BackgroundToggles() {
  const [value, setValue] = useState<BackgroundSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    void import('@tauri-apps/api/core')
      .then(({ invoke }) => invoke<BackgroundSettings>('background_settings'))
      .then(setValue)
      .catch(() => setValue(null));
  }, []);
  if (!value) return null;
  const change = (command: string, key: keyof BackgroundSettings, on: boolean) => {
    setError(null);
    void import('@tauri-apps/api/core')
      .then(({ invoke }) => invoke(command, { on }))
      .then(() => setValue({ ...value, [key]: on }))
      .catch((e: unknown) => setError(String((e as Error)?.message ?? e)));
  };
  return (
    <>
      <Toggle
        label="Keep running in the tray"
        description="Closing the window keeps PrivCord running in the tray, so messages still arrive and notify you. Quit from the tray icon."
        checked={value.closeToTray}
        onChange={(on) => change('set_close_to_tray', 'closeToTray', on)}
      />
      <Toggle
        label="Start when you sign in"
        description="Starts PrivCord in the tray when you sign in to your computer."
        checked={value.startAtLogin}
        onChange={(on) => change('set_start_at_login', 'startAtLogin', on)}
      />
      {error ? <p className="small" role="alert">{error}</p> : null}
    </>
  );
}

function Toggle({
  checked,
  onChange,
  label,
  description,
  disabled,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  label: string;
  description: string;
  disabled?: boolean;
}) {
  return (
    <div className="toggle-row">
      <div>
        <strong>{label}</strong>
        <p className="muted small">{description}</p>
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={label}
        className={`switch ${checked ? 'on' : ''}`}
        onClick={() => onChange(!checked)}
        disabled={disabled}
      >
        <span />
      </button>
    </div>
  );
}

function BackupControl({ engine }: { engine: ChatEngineApi | null }) {
  const [pass, setPass] = useState('');
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  async function backUp() {
    if (!engine) return;
    setBusy(true);
    setMessage(null);
    try {
      const text = await engine.exportBackup(pass);
      const url = URL.createObjectURL(new Blob([text], { type: 'application/octet-stream' }));
      const a = document.createElement('a');
      a.href = url;
      a.download = `privcord-backup-${new Date().toISOString().slice(0, 10)}.privcord`;
      document.body.appendChild(a);
      a.click();
      a.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 10_000);
      try {
        localStorage.setItem(LAST_BACKUP_KEY, String(Date.now()));
      } catch {
        /* no storage */
      }
      setMessage({ text: 'Backup saved. Keep the file and the passphrase safe: you need both, and nobody can recover the passphrase.', error: false });
    } catch (caught) {
      setMessage({ text: caught instanceof Error ? caught.message : 'Could not make the backup.', error: true });
    } finally {
      setBusy(false);
    }
  }

  async function restore(file: File) {
    if (!engine) return;
    setBusy(true);
    setMessage(null);
    try {
      const result = await engine.importBackup(await file.text(), pass);
      setMessage({ text: `Restored ${result.friends} friends, ${result.groups} groups and ${result.messages} messages.`, error: false });
    } catch (caught) {
      setMessage({ text: caught instanceof Error ? caught.message : 'Could not restore.', error: true });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card-box">
      <div className="card-title">
        <KeyIcon size={18} />
        <strong>Back up and restore chats</strong>
      </div>
      <p className="muted small">
        Saves your friends, groups, channels and messages (not photos or files) in one file, locked with a passphrase you
        choose. Restoring adds to what is here and needs the same identity (restore the 12-word phrase first on a new device).
      </p>
      <input
        type="password"
        aria-label="Backup passphrase"
        placeholder={`Passphrase (at least ${MIN_BACKUP_PASSPHRASE} characters)`}
        value={pass}
        onChange={(event) => setPass(event.target.value)}
      />
      <div className="button-row">
        <button type="button" className="button secondary small" disabled={busy || pass.length < MIN_BACKUP_PASSPHRASE} onClick={() => void backUp()}>
          Save a backup
        </button>
        <button type="button" className="button secondary small" disabled={busy || pass.length < MIN_BACKUP_PASSPHRASE} onClick={() => fileRef.current?.click()}>
          Restore from a file
        </button>
        <input
          ref={fileRef}
          type="file"
          hidden
          aria-label="Backup file"
          onChange={(event) => {
            const f = event.target.files?.[0];
            event.target.value = '';
            if (f) void restore(f);
          }}
        />
      </div>
      {message ? (
        <p className={`status-text ${message.error ? 'error' : ''}`} role={message.error ? 'alert' : 'status'}>
          {message.text}
        </p>
      ) : null}
      {isTauri() ? <AutoBackupControl pass={pass} /> : null}
    </div>
  );
}

function AppLockControl() {
  const [on, setOn] = useState(isLockEnabled());
  const [supported, setSupported] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    void lockSupported().then(setSupported);
  }, []);

  async function change(next: boolean) {
    setError(null);
    try {
      if (next) {
        await enableLock();
        setOn(true);
      } else {
        // Turning it off needs the same proof as opening the app.
        if (!(await unlock())) throw new Error('That did not work.');
        disableLock();
        setOn(false);
      }
    } catch (caught) {
      setError(caught instanceof Error && caught.message ? caught.message : 'Cancelled.');
    }
  }

  return (
    <>
      <Toggle
        label="Lock PrivCord with Windows Hello"
        description={
          supported === false
            ? 'Not available: this device has no passkey sign-in (Windows Hello, fingerprint, face or PIN) set up.'
            : 'Ask for your face, fingerprint or PIN to open PrivCord, and again after it has been out of sight for 5 minutes. It keeps other people at your computer out; it does not replace the encryption of the files on disk.'
        }
        checked={on}
        onChange={(value) => void change(value)}
        disabled={supported === false && !on}
      />
      {on ? (
        <button type="button" className="button secondary small" onClick={() => window.dispatchEvent(new Event('privcord:lock'))}>
          Lock now
        </button>
      ) : null}
      {error ? (
        <p className="status-text error" role="alert">
          {error}
        </p>
      ) : null}
    </>
  );
}

const SIGNALING_TEXT: Record<string, string> = {
  online: 'Online. Friends can find you.',
  connecting: 'Connecting…',
  idle: 'No connection service is on. Only Bluetooth and open connections work.',
  error: 'Could not reach the connection service. Retrying.',
};

export function SettingsDialog({
  open,
  onClose,
  initialSection,
  engine,
  state,
  settings,
  updateSettings,
  onOpenNuke,
  onOpenClearChats,
  onOpenRecovery,
}: {
  open: boolean;
  onClose: () => void;
  initialSection: SettingsSection;
  engine: ChatEngineApi | null;
  state: ChatState;
  settings: AppSettings;
  updateSettings: (patch: Partial<AppSettings>) => void;
  onOpenNuke: () => void;
  onOpenClearChats: () => void;
  onOpenRecovery: () => void;
}) {
  const [section, setSection] = useState<SettingsSection>(initialSection);
  const [displayName, setDisplayName] = useState(settings.displayName);
  const [saved, setSaved] = useState(false);
  const [linking, setLinking] = useState(false);
  const quiet = (state.prefs ?? NO_PREFS).quiet;

  useEffect(() => {
    if (!open) return;
    setSection(initialSection);
    setDisplayName(settings.displayName);
    setSaved(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, initialSection]);

  const connected = state.contacts.filter((c) => c.conn === 'connected').length;
  const appVersion = useAppVersion();

  return (
    <Modal open={open} onClose={onClose} title="Settings" wide className="tall">
      <div className="tabs" role="tablist" aria-label="Settings sections">
        {SECTIONS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            aria-selected={section === item.id}
            className={`tab ${section === item.id ? 'active' : ''}`}
            onClick={() => setSection(item.id)}
          >
            {item.label}
          </button>
        ))}
      </div>

      <div className="settings-content">
        {section === 'profile' ? (
          <section className="settings-section">
            <label className="field">
              <span>Your name</span>
              <input
                value={displayName}
                maxLength={40}
                placeholder="What friends see"
                onChange={(event) => {
                  setDisplayName(event.target.value);
                  setSaved(false);
                }}
              />
            </label>
            <div className="button-row">
              <button
                type="button"
                className="button primary"
                disabled={displayName.trim() === settings.displayName}
                onClick={() => {
                  updateSettings({ displayName: displayName.trim() });
                  void engine?.refreshOwnName(displayName);
                  setSaved(true);
                }}
              >
                <CheckIcon size={15} />
                Save
              </button>
            </div>
            {saved ? (
              <p className="status-text">Saved. Friends see this name on invites you share from now on.</p>
            ) : (
              <p className="muted small">Shown on your invite. There are no accounts: your identity lives on this device.</p>
            )}
          </section>
        ) : null}

        {section === 'devices' ? (
          <section className="settings-section">
            <DevicesStrip engine={engine} state={state} />
            {state.devices && state.devices.length > 0 ? (
              <ul className="device-list" aria-label="Your devices">
                {state.devices.map((d) => (
                  <DeviceRow key={d.id} device={d} engine={engine} />
                ))}
              </ul>
            ) : (
              <p className="muted small">Only this device so far.</p>
            )}
            {state.syncLost ? (
              <p className="muted small">
                {state.syncLost === 1
                  ? '1 item from another device could not be recovered (that device was gone before it was sent on).'
                  : `${state.syncLost} items from another device could not be recovered (that device was gone before they were sent on).`}
              </p>
            ) : null}
            {engine?.startDeviceLink && state.holdsAccountKey ? (
              <div className="button-row">
                <button type="button" className="button primary" onClick={() => setLinking(true)}>
                  Link a new device
                </button>
              </div>
            ) : null}
            <p className="muted small">
              {state.holdsAccountKey
                ? 'Your chats stay in step on every device you link, with no server in between. Your account key stays on this device.'
                : 'This device was linked to your account. Linking more devices is done from the device that holds your account key.'}
            </p>
            <DeviceLinkDialog open={linking} mode="host" engine={engine} state={state} onClose={() => setLinking(false)} />
          </section>
        ) : null}

        {section === 'appearance' ? (
          <section className="settings-section">
            <div className="setting-block">
              <span className="setting-label">Color theme</span>
              <div className="theme-grid" role="group" aria-label="Color theme">
                <button
                  type="button"
                  className={`theme-tile ${settings.palette === 'classic' ? 'active' : ''}`}
                  onClick={() => updateSettings({ palette: 'classic' })}
                  aria-pressed={settings.palette === 'classic'}
                >
                  <span className="theme-preview classic" aria-hidden="true" />
                  <span>Classic</span>
                </button>
                {THEMES.map((t) => {
                  const v = themeVars(t);
                  return (
                    <button
                      key={t.id}
                      type="button"
                      className={`theme-tile ${settings.palette === t.id ? 'active' : ''}`}
                      onClick={() => updateSettings({ palette: t.id })}
                      aria-pressed={settings.palette === t.id}
                    >
                      <span className="theme-preview" style={{ background: t.bg, borderColor: v['--border-strong'] }} aria-hidden="true">
                        <span style={{ background: t.surface }} />
                        <span style={{ background: v['--accent-fill'] }} />
                      </span>
                      <span>{t.name}</span>
                    </button>
                  );
                })}
              </div>
            </div>
            {settings.palette === 'classic' ? (
              <>
            <div className="setting-block">
              <span className="setting-label">Theme</span>
              <div className="segmented" role="group" aria-label="Theme">
                {(['system', 'dark', 'light'] as ThemeMode[]).map((theme) => (
                  <button
                    key={theme}
                    type="button"
                    className={settings.theme === theme ? 'active' : ''}
                    onClick={() => updateSettings({ theme })}
                    aria-pressed={settings.theme === theme}
                  >
                    {theme === 'system' ? 'System' : theme === 'dark' ? 'Dark' : 'Light'}
                  </button>
                ))}
              </div>
            </div>
            <div className="setting-block">
              <span className="setting-label">Accent color</span>
              <div className="swatches">
                {ACCENTS.map((accent) => (
                  <button
                    key={accent.value}
                    type="button"
                    className={`swatch ${settings.accent === accent.value ? 'active' : ''}`}
                    style={{ background: accent.value }}
                    onClick={() => updateSettings({ accent: accent.value })}
                    aria-label={accent.name}
                    aria-pressed={settings.accent === accent.value}
                    title={accent.name}
                  >
                    {settings.accent === accent.value ? <CheckIcon size={15} /> : null}
                  </button>
                ))}
              </div>
            </div>
              </>
            ) : (
              <p className="muted small">The color theme sets the light or dark look and the accent color.</p>
            )}
          </section>
        ) : null}

        {section === 'notifications' ? (
          <section className="settings-section">
            <Toggle
              label="Message notifications"
              description="Show a system notification for new messages while PrivCord is running, also from the tray."
              checked={settings.notificationsEnabled}
              onChange={(value) => {
                updateSettings({ notificationsEnabled: value });
                if (value) void ensureNotificationPermission();
              }}
            />
            <Toggle
              label="Message previews"
              description="Turn off to keep message text out of notifications."
              checked={settings.notificationPreviews}
              onChange={(value) => updateSettings({ notificationPreviews: value })}
              disabled={!settings.notificationsEnabled}
            />
            <Toggle
              label="Unread count in the title and taskbar"
              description="Turn off to keep the number of unread messages out of the window title, tray tooltip and taskbar badge, which anyone watching your screen or a screen share can see."
              checked={settings.showUnreadCount}
              onChange={(value) => updateSettings({ showUnreadCount: value })}
            />
            <Toggle
              label="Quiet hours"
              description="No notifications between these times, on all your devices. Messages still arrive and wait in the chat."
              checked={quiet.enabled}
              onChange={(value) => engine?.setQuietHours?.(value, quiet.from, quiet.to)}
              disabled={!settings.notificationsEnabled}
            />
            {quiet.enabled ? (
              <div className="quiet-times">
                <label className="field">
                  <span>From</span>
                  <input type="time" value={minutesToTime(quiet.from)} onChange={(e) => e.target.value && engine?.setQuietHours?.(true, timeToMinutes(e.target.value), quiet.to)} />
                </label>
                <label className="field">
                  <span>Until</span>
                  <input type="time" value={minutesToTime(quiet.to)} onChange={(e) => e.target.value && engine?.setQuietHours?.(true, quiet.from, timeToMinutes(e.target.value))} />
                </label>
              </div>
            ) : null}
            {isTauri() ? <div id="keep-running"><BackgroundToggles /></div> : null}
            <DeliveryHealth engine={engine} state={state} notificationsEnabled={settings.notificationsEnabled} go={setSection} />
          </section>
        ) : null}

        {section === 'connection' ? (
          <section className="settings-section">
            <div className="connection-summary">
              <span className={`connection-dot ${state.signaling === 'online' ? 'on' : ''}`} />
              <div>
                <strong>{SIGNALING_TEXT[state.signaling] ?? state.signaling}</strong>
                <p className="muted small">
                  {connected} of {state.contacts.length} friends connected
                  {state.queued > 0 ? ` · ${state.queued} message${state.queued === 1 ? '' : 's'} waiting` : ''}
                </p>
              </div>
            </div>
            <p className="muted small">
              Friends reconnect by themselves whenever you are both online. Nothing here needs changing for that to work.
            </p>
            <details className="advanced">
              <summary>Connection details and options (for people who want control)</summary>
              <p className="muted small">
                Messages go straight from device to device. A device keeps at most {MAX_LIVE_LINKS} live connections, which
                is plenty for groups of a few dozen.
              </p>
              <NetworkTransparency engine={engine} updateChecks={isTauri() && settings.updateChecks} />
              <NetworkSettings engine={engine} state={state} />
            </details>
          </section>
        ) : null}

        {section === 'security' ? (
          <section className="settings-section">
            <PrivacyCheckup engine={engine} settings={settings} state={state} go={setSection} />
            <AppLockControl />
            {engine?.setSecureScreen ? (
              <Toggle
                label="Hide this app from screenshots and screen recording"
                description="Other programs that capture the screen see a blank window. It applies on every device of the account."
                checked={(state.prefs ?? NO_PREFS).secureScreen}
                onChange={(on) => engine.setSecureScreen!(on)}
              />
            ) : null}
            <Toggle
              label="Stay group-only with people I meet in groups"
              description="You still read and reply in the groups you share, but nobody you only know through a group can message you directly, and you cannot message them, unless you swap invites yourselves. It applies to people you meet from now on and to the ones you already have."
              checked={settings.groupOnly}
              onChange={(value) => {
                updateSettings({ groupOnly: value });
                engine?.setGroupOnly(value);
              }}
            />
            <details className="advanced">
              <summary>More privacy tools</summary>
            <Toggle
              label="Panic wipe"
              description={`Press Ctrl+Shift+Backspace ${PANIC_PRESSES} times within ${PANIC_WINDOW_MS / 1000} seconds and everything on this device is erased immediately, with no confirmation. Your 12 recovery words are the only way back, so keep them somewhere safe first.`}
              checked={settings.panicWipe}
              onChange={(value) => updateSettings({ panicWipe: value })}
            />
            <div className="card-box">
              <div className="card-title">
                <KeyIcon size={18} />
                <strong>Recovery phrase</strong>
              </div>
              <p className="muted small">
                Your 12 words recreate this identity on a new device or after a wipe. PrivCord never stores
                them, so they cannot be shown again.
              </p>
              <button
                type="button"
                className="button secondary small"
                onClick={() => {
                  onClose();
                  onOpenRecovery();
                }}
              >
                Restore from phrase
              </button>
            </div>
            <div className="card-box">
              <div className="card-title">
                <TrashIcon size={18} />
                <strong>Clear all chats</strong>
              </div>
              <p className="muted small">Deletes every message on this device. Friends and groups stay.</p>
              <button
                type="button"
                className="button secondary small"
                onClick={() => {
                  onClose();
                  onOpenClearChats();
                }}
              >
                Clear chats…
              </button>
            </div>
            </details>
            <div className="emergency-block">
              <div>
                <strong>Erase everything</strong>
                <p className="muted small">
                  Deletes chats, friends, groups, keys and settings from this device and starts over. Other
                  people keep what they already received.
                </p>
              </div>
              <button
                type="button"
                className="button danger"
                onClick={() => {
                  onClose();
                  onOpenNuke();
                }}
              >
                <AlertIcon size={15} />
                Erase everything…
              </button>
            </div>
          </section>
        ) : null}

        {section === 'backup' ? (
          <section className="settings-section">
            <BackupControl engine={engine} />
          </section>
        ) : null}

        {section === 'about' ? (
          <section className="settings-section">
            {isTauri() ? (
              <div className="card-box">
                <Toggle
                  checked={settings.updateChecks}
                  onChange={(value) => updateSettings({ updateChecks: value })}
                  label="Check for updates"
                  description={`Looks for a new version when the app starts, when you come back to it, and about every ${RECHECK_MS / 3_600_000} hours while it is open, and offers to install it. This contacts GitHub, which can see your IP address, so it stays off while Hide my IP or Tor mode is on.`}
                />
                <UpdateCheck />
              </div>
            ) : null}
            <WhatsNew engine={engine} />
            <DiagLogCard engine={engine} />
            <StorageByChat state={state} engine={engine} />
            <p>
              <strong>PrivCord</strong> · {isTauri() ? `version ${appVersion}` : 'development build'}
            </p>
            <p className="muted small">
              Private chats and groups with no accounts and no cloud. Messages are end-to-end encrypted and go
              straight between devices.
            </p>
            <p className="muted small">
              Made by {config.about.author} · {config.about.licenseName} ·{' '}
              <a href={config.about.authorLink} target="_blank" rel="noreferrer">
                {config.about.authorLinkLabel}
              </a>
            </p>
            <details className="advanced">
              <summary>Technical details</summary>
              <SupportTools engine={engine} />
            {state.usage ? (
              <div className="card-box">
                <strong>On this device</strong>
                <ul className="usage-list">
                  <li>
                    <span>Messages</span>
                    <span>{state.usage.messages.toLocaleString()}</span>
                  </li>
                  <li>
                    <span>Friends</span>
                    <span>{state.usage.contacts.toLocaleString()}</span>
                  </li>
                  <li>
                    <span>Files and photos</span>
                    <span>
                      {state.usage.files.toLocaleString()} · {formatBytes(state.usage.fileBytes)}
                    </span>
                  </li>
                  {state.usage.syncEntries > 0 ? (
                    <li>
                      <span>Kept for syncing your devices</span>
                      <span>{state.usage.syncEntries.toLocaleString()} entries</span>
                    </li>
                  ) : null}
                </ul>
                <p className="muted small">Everything stays on this device, encrypted. Deleting a chat frees its files.</p>
              </div>
            ) : null}
            <dl className="detail-grid">
              <div>
                <dt>Your ID</dt>
                <dd className="mono selectable">{state.uid}</dd>
              </div>
              <div>
                <dt>Storage</dt>
                <dd>{state.storageBackend === 'sqlite' ? 'Encrypted on this device' : 'Browser (development, not encrypted)'}</dd>
              </div>
            </dl>
            </details>
          </section>
        ) : null}
      </div>
    </Modal>
  );
}



function minutesToTime(m: number): string {
  const h = Math.floor(m / 60) % 24;
  return `${String(h).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
}

function timeToMinutes(t: string): number {
  const [h, m] = t.split(':').map(Number);
  return (h ?? 0) * 60 + (m ?? 0);
}
