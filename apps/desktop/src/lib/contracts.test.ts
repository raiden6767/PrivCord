/**
 * Contracts between the Rust engine and the screens that talk to it. Several fixed bugs were two sides drifting apart
 * (a stage one side emits and the other never shows; a command one side calls and the other renamed). These read the
 * sources and fail when they drift, so the next one is caught in CI instead of on a device.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { MAX_LIVE_LINKS } from './limits';

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
});
