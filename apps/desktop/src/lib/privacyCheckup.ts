import type { SettingsSection } from '../components/SettingsDialog';

/**
 * What the privacy checkup looks at (everything is known on this device; nothing is sent anywhere). The packaged app gets
 * the answer from the shared Rust engine (`insights.rs`); this copy is the fallback for the browser-only development
 * engine, and the same rules.
 */
export interface CheckupInput {
  app_lock: boolean;
  /** Milliseconds since 1970 of the last backup, or 0 if none was ever made. */
  last_backup_at: number;
  hide_ip: boolean;
  previews_off: boolean;
  group_only: boolean;
  panic_wipe: boolean;
  /** Devices on the account that were not heard from for 90 days. */
  stale_devices: number;
  now_ms: number;
}

export interface CheckupItem {
  id: string;
  label: string;
  ok: boolean;
  /** What to do (shown when not ok) or what is on (when ok). */
  hint: string;
  /** Where in Settings to fix it. */
  section: SettingsSection;
}

/** Where in Settings each item is fixed (screen navigation, not shared logic). */
export const SECTION_OF: Record<string, SettingsSection> = {
  lock: 'security',
  backup: 'backup',
  ip: 'connection',
  previews: 'notifications',
  grouponly: 'security',
  panic: 'security',
  devices: 'devices',
};

const WEEK = 7 * 24 * 3600 * 1000;

export function privacyCheckup(i: CheckupInput): { items: CheckupItem[]; score: number; total: number } {
  const now = i.now_ms;
  const items: CheckupItem[] = [
    { id: 'lock', label: 'App lock', ok: i.app_lock, hint: i.app_lock ? 'Opening PrivCord asks for your passkey.' : 'Anyone holding your unlocked device can read your chats. Turn on the lock.', section: 'security' },
    {
      id: 'backup',
      label: 'Backup',
      ok: i.last_backup_at > 0 && now - i.last_backup_at < 4 * WEEK,
      hint: i.last_backup_at > 0 && now - i.last_backup_at < 4 * WEEK ? 'A recent backup exists.' : 'No recent backup. Without one, a lost device means lost chats (your 12 words only restore your identity).',
      section: 'backup',
    },
    { id: 'ip', label: 'IP address hidden from friends', ok: i.hide_ip, hint: i.hide_ip ? 'Connections go through a relay or Tor, so friends do not see your address.' : 'Friends can see your IP address on a direct connection. Optional, slower.', section: 'connection' },
    { id: 'previews', label: 'Notifications hide message text', ok: i.previews_off, hint: i.previews_off ? 'Notifications only say a message arrived.' : 'Message text can appear on a locked screen. Turn off message previews.', section: 'notifications' },
    { id: 'grouponly', label: 'Group-only with people met in groups', ok: i.group_only, hint: i.group_only ? 'Strangers from a group cannot message you directly.' : 'People you only know from a group can message you directly.', section: 'security' },
    { id: 'panic', label: 'Panic wipe ready', ok: i.panic_wipe, hint: i.panic_wipe ? 'One gesture erases this device.' : 'Optional: a one-gesture wipe for emergencies.', section: 'security' },
    { id: 'devices', label: 'No forgotten devices', ok: i.stale_devices === 0, hint: i.stale_devices === 0 ? 'Every linked device was seen recently.' : `${i.stale_devices} linked device${i.stale_devices === 1 ? '' : 's'} not seen for 90 days. Remove ${i.stale_devices === 1 ? 'it' : 'them'} if lost.`, section: 'devices' },
  ];
  const score = items.filter((x) => x.ok).length;
  return { items, score, total: items.length };
}
