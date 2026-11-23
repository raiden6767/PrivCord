/** How many friends a device keeps live connections to at once. Must equal `MAX_LINKS` in the Rust engine
 * (`crates/privcord-domain/src/link_policy.rs`); a test in `contracts.test.ts` fails if they drift. */
export const MAX_LIVE_LINKS = 30;
