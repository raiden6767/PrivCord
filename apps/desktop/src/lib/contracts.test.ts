/**
 * Contracts between the Rust engine and the screens that talk to it. Several fixed bugs were two sides drifting apart
 * (a stage one side emits and the other never shows; a command one side calls and the other renamed). These read the
 * sources and fail when they drift, so the next one is caught in CI instead of on a device.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { MAX_LIVE_LINKS, MIN_BACKUP_PASSPHRASE } from './limits';

const root = join(__dirname, '../../../..');

function files(dir: string, ext: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules' || name === 'target' || name === 'build') continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) files(p, ext, out);
    else if (p.endsWith(ext)) out.push(p);
  }
  return out;
}
const text = (paths: string[]) => paths.map((p) => readFileSync(p, 'utf8')).join('\n');

describe('engine and screens agree', () => {
  const rust = text(files(join(root, 'crates'), '.rs')) + text(files(join(root, 'apps/desktop/src-tauri/src'), '.rs'));

  it('every device-link stage the engine can report is handled by both the desktop and the Android screen', () => {
    const engine = text(files(join(root, 'crates/privcord-engine/src'), '.rs'));
    const stages = new Set<string>();
    for (const m of engine.matchAll(/stage:\s*"([a-z-]+)"/g)) stages.add(m[1] as string);
    for (const m of engine.matchAll(/end_link\([^,]+,\s*"([a-z-]+)"/g)) stages.add(m[1] as string);
    expect(stages.size).toBeGreaterThan(4);
    const desktop = text([join(root, 'apps/desktop/src/components/DeviceLinkDialog.tsx')]);
    const android = text([join(root, 'apps/android/app/src/main/java/com/duochat/app/ui/DeviceLink.kt')]);
    for (const stage of stages) {
      expect(desktop, `desktop does not handle link stage "${stage}"`).toMatch(new RegExp(`['"]${stage}['"]`));
      expect(android, `Android does not handle link stage "${stage}"`).toContain(`"${stage}"`);
    }
  });

  it('every engine or Tauri command the desktop app calls exists in Rust', () => {
    const src = text(files(join(root, 'apps/desktop/src/lib'), '.ts').filter((p) => !p.endsWith('.test.ts')));
    const names = new Set<string>();
    for (const m of src.matchAll(/\b(?:call|invoke|snapshot)(?:<[^>(]*>)?\(\s*'([a-z_]+)'/g)) names.add(m[1] as string);
    expect(names.size).toBeGreaterThan(30);
    for (const name of names) {
      const known = rust.includes(`"${name}"`) || new RegExp(`fn ${name}\\s*[(<]`).test(rust);
      expect(known, `the desktop app calls "${name}" but nothing in Rust defines it`).toBe(true);
    }
  });

  it('every error the Android screens put in words is also in words on the desktop (the two tables must not drift)', () => {
    const android = text([join(root, 'apps/android/app/src/main/java/com/duochat/app/rustcore/RustChatEngine.kt')]);
    const desktop = text([join(root, 'apps/desktop/src/lib/hosted/hostedEngine.ts')]);
    const mapped = new Set<string>();
    for (const m of android.matchAll(/FfiException\.([A-Za-z]+) -> "/g)) mapped.add(m[1] as string);
    expect(mapped.size).toBeGreaterThan(10);
    const table = new Set<string>();
    for (const m of desktop.matchAll(/\['([A-Za-z]+)',/g)) table.add(m[1] as string);
    for (const code of mapped) expect(table.has(code), `Android says "${code}" in words but the desktop shows the raw code`).toBe(true);
    // The ones a person meets while unlinking a device.
    for (const code of ['InvalidPhrase', 'UnknownDevice', 'NeedsAccountKey']) {
      expect(table.has(code), `desktop has no words for ${code}`).toBe(true);
    }
  });

  it('the connection limit the screens state is the one the engine enforces', () => {
    const m = /pub const MAX_LINKS: usize = (\d+);/.exec(rust);
    expect(m, 'MAX_LINKS not found in Rust').not.toBeNull();
    expect(MAX_LIVE_LINKS).toBe(Number(m?.[1]));
    const android = text([join(root, 'apps/android/app/src/main/java/com/duochat/app/ui/SettingsDialog.kt')]);
    expect(android, 'the Android text says a different number of friends').toContain(`at most ${m?.[1]} friends`);
  });

  it('the TypeScript copies of the engine texts (browser development) say what the engine says', () => {
    const engine = text([join(root, 'crates/privcord-engine/src/insights.rs')]);
    for (const file of ['lib/networkHosts.ts', 'lib/privacyCheckup.ts']) {
      const src = text([join(root, 'apps/desktop/src', file)]);
      // Sentences long enough to be user text, in single or double quotes.
      const sentences = [...src.matchAll(/'([^'\n]{40,})'/g)].map((m) => m[1] as string).filter((s) => /^[A-Z]/.test(s) && /[a-z] [a-z]/.test(s) && !s.includes('${'));
      expect(sentences.length, `no sentences found in ${file}`).toBeGreaterThan(3);
      for (const s of sentences) expect(engine, `${file} has text the engine does not: "${s}"`).toContain(s);
    }
  });

  it('the credits and licence are the same in docs/about.json, the Android app and the LICENSE file', () => {
    const about = JSON.parse(text([join(root, 'docs/about.json')])) as Record<string, string>;
    const kt = text([join(root, 'apps/android/app/src/main/java/com/duochat/app/AppConfig.kt')]);
    expect(kt).toContain(`CREATED_BY = "${about.author}"`);
    expect(kt).toContain(`LICENSE_SHORT = "${about.licenseShort}"`);
    expect(text([join(root, 'LICENSE')]).split('\n')[0]).toContain(about.licenseName);
    // The pages must not type these in again.
    const desktop = text([join(root, 'apps/desktop/src/components/SettingsDialog.tsx')]);
    const android = text([join(root, 'apps/android/app/src/main/java/com/duochat/app/ui/SettingsDialog.kt')]);
    for (const value of [about.author, about.authorLink, about.licenseName]) {
      expect(desktop, `desktop settings types in "${value}"`).not.toContain(value);
      expect(android, `Android settings types in "${value}"`).not.toContain(value);
    }
  });

  it('numbers the settings text states (days, hours, minutes, megabytes, characters) are the engine\'s own values', () => {
    // Reads `const NAME: type = 7 * 24 * 3600;` from the Rust sources (products of plain numbers only).
    const rustConst = (name: string): number => {
      const m = new RegExp(`const ${name}: [a-z0-9]+ = ([0-9_]+(?: \\* [0-9_]+)*);`).exec(rust);
      expect(m, `${name} not found in Rust`).not.toBeNull();
      return (m?.[1] ?? '').split('*').reduce((n, part) => n * Number(part.replace(/_/g, '')), 1);
    };
    const kt = (f: string) => text([join(root, 'apps/android/app/src/main/java/com/duochat/app', f)]);
    const tsx = (f: string) => text([join(root, 'apps/desktop/src/components', f)]);
    const DAY = 24 * 3600;
    const relayDays = rustConst('RELAY_TTL_SEC') / DAY;
    expect(kt('ui/SettingsDialog.kt'), 'Android says a different relay wait').toContain(`${relayDays} days`);
    const staleDays = rustConst('STALE_AFTER_MS') / (DAY * 1000);
    expect(kt('ui/DeviceLink.kt')).toContain(`${staleDays} days`);
    expect(tsx('DeviceRow.tsx')).toContain(`${staleDays} days`);
    expect(text([join(root, 'crates/privcord-engine/src/insights.rs')])).toContain(`${staleDays} days`);
    const removeHours = rustConst('REMOVE_AFTER_MS') / (3600 * 1000);
    expect(kt('ui/DeviceLink.kt')).toContain(`${removeHours} hours`);
    expect(tsx('DeviceRow.tsx')).toContain(`${removeHours} hours`);
    expect(rustConst('LINK_TTL_MS')).toBe(10 * 60 * 1000);
    expect(kt('ui/DeviceLink.kt')).toContain('ten minutes');
    expect(tsx('DeviceLinkDialog.tsx')).toContain('ten minutes');
    expect(rustConst('BACKUP_FILE_BUDGET')).toBe(50 * 1024 * 1024);
    expect(kt('ui/BackupSection.kt')).toContain('50 MB');
    const minPass = Number(/pub const BACKUP_MIN_PASSPHRASE: usize = (\d+);/.exec(rust)?.[1]);
    expect(MIN_BACKUP_PASSPHRASE).toBe(minPass);
    expect(kt('ui/BackupSection.kt')).toContain(`MIN_PASS = ${minPass}`);
  });
});
