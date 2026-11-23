import about from '../../../docs/about.json';

const env = import.meta.env;

function value(name: keyof ImportMetaEnv): string {
  return (env[name] ?? '').trim();
}

export const config = {
  conversationId: value('VITE_CONVERSATION_ID') || 'duo',
  /** The page's own copy of the version (package.json). The release number of the installed app is read from the app. */
  appVersion: __APP_VERSION__,
  /** Who made it and under what licence: one file for both apps (`docs/about.json`), checked by a test. */
  about,
} as const;

export function isTauri(): boolean {
  return typeof window !== 'undefined' && Boolean(window.__TAURI_INTERNALS__);
}
