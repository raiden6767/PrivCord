import type { ChatEngineApi } from './engineApi';

/**
 * The diagnostic log, from the page's side: what the screens did, written to the engine's log (kept on this computer
 * only, nothing uploaded). Never put message text, names, keys or addresses in an entry; the engine also blanks what looks
 * like an identifier or an address, but do not rely on that.
 */

type Entry = [string, string, string];
let sink: ((category: string, event: string, detail: string) => void) | null = null;
let on = true;
const early: Entry[] = [];

export function diag(category: string, event: string, detail = ''): void {
  if (!on) return;
  if (sink) {
    try {
      sink(category, event, detail);
    } catch {
      /* the log must never break a screen */
    }
  } else if (early.length < 300) {
    early.push([category, event, detail]);
  }
}

/** Starts passing entries to the engine (and passes on those that waited for it). */
export function attachDiag(engine: ChatEngineApi): void {
  sink = (c, e, d) => engine.diagLog?.(c, e, d);
  for (const [c, e, d] of early.splice(0)) diag(c, e, d);
  void engine.diagStatus?.().then((s) => {
    if (s && s.enabled === false) on = false;
  });
}

export function detachDiag(): void {
  sink = null;
}

export function setDiagOn(value: boolean): void {
  on = value;
}

/** What the app says about itself at the top of a saved log (no names, no ids). */
export function aboutThisApp(version: string): string {
  const w = typeof window === 'undefined' ? null : window;
  return [
    `PrivCord ${version}`,
    `Windows app · ${w?.navigator.userAgent ?? 'unknown'}`,
    `Window ${w?.innerWidth ?? '?'}x${w?.innerHeight ?? '?'} px, pixel ratio ${w?.devicePixelRatio ?? '?'}`,
    `Language ${w?.navigator.language ?? '?'}, time zone ${Intl.DateTimeFormat().resolvedOptions().timeZone}`,
  ].join('\n');
}

/** Methods that run all the time or only carry the log itself: not worth an entry each. */
const QUIET = new Set([
  'subscribe',
  'getState',
  'start',
  'stop',
  'refresh',
  'diagLog',
  'diagExport',
  'diagStatus',
  'diagClear',
  'setDiagEnabled',
  'insight',
  'storageByChat',
  'quickTimes',
  'messageReceipts',
  'meshHops',
  'diagnostics',
  'selfTest',
  'ownCard',
  'search',
  'loadEarlier',
  'getFile',
]);

/** A call's arguments as safe particulars: numbers and switches as they are, text and lists only by size. */
function describeArgs(args: unknown[]): string {
  return args
    .map((a) => {
      if (typeof a === 'number' || typeof a === 'boolean') return String(a);
      if (typeof a === 'string') return `chars=${a.length}`;
      if (Array.isArray(a)) return `items=${a.length}`;
      if (a instanceof Uint8Array) return `bytes=${a.length}`;
      return a === null || a === undefined ? '-' : typeof a;
    })
    .join(' ');
}

function failure(e: unknown): string {
  const err = e as { name?: string; message?: string } | undefined;
  return `${err?.name ?? 'Error'}: ${String(err?.message ?? e).slice(0, 120)}`;
}

/** The engine as the screens use it, with every action a person takes written to the log: what, how long, how it ended. */
export function withDiag(engine: ChatEngineApi): ChatEngineApi {
  return new Proxy(engine, {
    get(target, prop) {
      const value = Reflect.get(target, prop, target) as unknown;
      if (typeof value !== 'function' || typeof prop !== 'string') return value;
      const bound = (value as (...a: unknown[]) => unknown).bind(target);
      if (QUIET.has(prop) || prop.startsWith('get') || prop.startsWith('on')) return bound;
      return (...args: unknown[]) => {
        const started = performance.now();
        diag('call', `${prop}: asked`, describeArgs(args));
        const done = () => {
          const ms = Math.round(performance.now() - started);
          diag('call', `${prop}: done`, `${ms} ms${ms > 2000 ? ' (slow)' : ''}`);
        };
        try {
          const result = bound(...args);
          if (result && typeof (result as Promise<unknown>).then === 'function') {
            return (result as Promise<unknown>).then(
              (v) => {
                done();
                return v;
              },
              (e) => {
                diag('error', `${prop}: failed`, failure(e));
                throw e;
              },
            );
          }
          done();
          return result;
        } catch (e) {
          diag('error', `${prop}: threw`, failure(e));
          throw e;
        }
      };
    },
  });
}

/** Titles of dialogs that are the same for everyone; a dialog named after a friend or group is logged as "a dialog". */
const PLAIN_TITLES = new Set([
  'Add a friend',
  'Clear Everything',
  'Connect without internet',
  'Delivery details',
  'Erase everything on this device',
  'How this chat is protected',
  'Link',
  'New group',
  'Public channels',
  'Settings',
  'Link a new device',
  'Link to your other device',
  'Your 12-word recovery phrase',
  'Restore your identity',
]);

export function plainTitle(title: string): string {
  return PLAIN_TITLES.has(title) ? title : 'a dialog';
}

/** A click as something safe to write down: the kind of control, its place, and a label only where labels are fixed words. */
function describeClick(target: EventTarget | null): string | null {
  const el = (target as Element | null)?.closest?.('button, [role="button"], [role="tab"], a, input, select, summary, [role="switch"]');
  if (!el) return null;
  const tag = el.tagName.toLowerCase();
  const classes = Array.from(el.classList)
    .filter((c) => /^[a-z-]{2,24}$/.test(c))
    .slice(0, 3)
    .join('.');
  const role = el.getAttribute('role');
  // Tabs, section summaries and settings switches carry fixed words; a button's own text can be a friend's name.
  const fixed = role === 'tab' || tag === 'summary' || role === 'switch' ? (el.textContent ?? '').trim().slice(0, 40) : '';
  const dialog = el.closest('.modal')?.querySelector('h2')?.textContent?.trim() ?? '';
  const where = dialog ? ` in "${plainTitle(dialog)}"` : '';
  return `${tag}${classes ? '.' + classes : ''}${role ? `[${role}]` : ''}${fixed ? ` "${fixed}"` : ''}${where}`;
}

let hooked = false;

/** Writes down what the window itself does: errors, clicks, going to the background, the network coming and going. */
export function installDiagHooks(): void {
  if (hooked || typeof window === 'undefined') return;
  hooked = true;
  window.addEventListener('error', (e) => diag('error', 'window error', `${e.message?.slice(0, 140)} (${(e.filename ?? '').split('/').pop()}:${e.lineno})`));
  window.addEventListener('unhandledrejection', (e) => diag('error', 'unhandled rejection', failure((e as PromiseRejectionEvent).reason)));
  document.addEventListener('visibilitychange', () => diag('app', 'window', document.visibilityState));
  window.addEventListener('online', () => diag('net', 'the computer went online'));
  window.addEventListener('offline', () => diag('net', 'the computer went offline'));
  window.addEventListener('focus', () => diag('app', 'window focus', 'gained'));
  window.addEventListener('blur', () => diag('app', 'window focus', 'lost'));
  document.addEventListener(
    'click',
    (e) => {
      const d = describeClick(e.target);
      if (d) diag('ui', 'click', d);
    },
    true,
  );
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') diag('ui', 'key', 'Escape');
  });
  diag('app', 'page started', '');
}
