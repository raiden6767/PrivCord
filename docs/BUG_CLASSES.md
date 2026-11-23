# Bug classes and what guards each

Built from the 91 non-merge fix commits in the history (a commit often fixes several things, so the real count is
higher). Each class says how it was found, what now catches the next one, and what still needs a real device. Add a row
when a bug does not fit a class; add a guard when a class has none.

| Class (fix commits) | Examples | Guard today | Still needs a device or a person |
|---|---|---|---|
| Relay, signaling, connectivity (22) | fresh install had no relay; link requests never reached the phone; Bluetooth chunk loss; many devices | Real-relay and Nostr end-to-end tests; `native_signaling` tests (including "online with nothing subscribed"); Rust-to-Kotlin blob vectors (`docs/blob-wire-vectors.json`); engine/screen contract test | Real public relays, real networks, vendor radio behaviour |
| CI, workflow, permissions (16) | workflows without `permissions`; path checks; builds that only fail in CI | CodeQL (Actions); `scripts/precheck.sh` runs formatting, the migration map, types and unit tests locally before a push | Runner outages, new advisories on unrelated PRs |
| Stuck or stale state, retries (15) | "Waiting for the new device" forever; stale "Linking was cancelled"; transfers aborting on retry | Engine state-machine tests; stage contract test (every stage the engine reports is handled by both screens); update download retries | Screens that hold their own copy of engine state (Android `RustChatEngine` wrapper): no automatic check yet |
| Security hardening (13) | replayed answers, forged cards, floods, zero-key fallbacks, hard-coded values | Fuzz targets for every parser; CodeQL; `cargo audit`; engine tests for replay and forgery | Independent audit (`AUDIT.md`) |
| Crashes, R8, native, launch (11) | instant crash with R8 and WebRTC; foreground service not started; crash loops | Emulator launch check; "native code finds its Java" check; `ManifestRulesTest` (permissions per service type, `startForeground` on every start, Android 15 timeout, receivers survive empty intents, nothing exported) | Vendor-specific behaviour (battery managers), real notification rendering |
| Dependencies and tooling drift (10) | RUSTSEC advisories; KSP, Gradle, AGP versions | `cargo audit` in CI; Dependabot | An advisory landing blocks unrelated PRs until bumped |
| UI layout, accessibility, focus (7) | keyboard covering the composer; contrast; focus jumping to the close button | Robolectric screen renders at several font scales; theme contrast tests; `dialogFocus.test.tsx` | Taste, animation feel, real keyboards |
| Files, media, voice (6) | transfer abort on retry; voice playback crash | Engine file tests (resumable, 50 MB); fuzz targets for media | Microphone and playback on real hardware |
| Wire format parity across languages (6) | Kotlin refusing blobs Rust makes; hybrid (v2) blobs dropped | TypeScript, Rust and Kotlin vectors; `SealWireVectorsTest` | New formats need a vector added with them |

## Rules for new code

- A new Android service, receiver or permission gets covered by `ManifestRulesTest` automatically; check it still reads
  the rule you rely on.
- A new value that crosses the engine/screen boundary (a stage, an event, a command name) is checked by
  `apps/desktop/src/lib/contracts.test.ts`; extend it when the vocabulary grows a new kind.
- A new blob or message shape gets a case in `crates/privcord-domain/tests/blob_wire_vectors.rs`.
- Run `scripts/precheck.sh` before every push. Kotlin only compiles in CI, so say so when an Android change has not run.
