/** How many friends a device keeps live connections to at once. Must equal `MAX_LINKS` in the Rust engine
 * (`crates/privcord-domain/src/link_policy.rs`); a test in `contracts.test.ts` fails if they drift. */
export const MAX_LIVE_LINKS = 30;

/** Shortest backup passphrase. Must equal `BACKUP_MIN_PASSPHRASE` in the Rust crypto crate (a test checks). */
export const MIN_BACKUP_PASSPHRASE = 12;

/** Panic wipe: this many Ctrl+Shift+Backspace presses within this long. The settings text is made from these. */
export const PANIC_PRESSES = 3;
export const PANIC_WINDOW_MS = 2000;
