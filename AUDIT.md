# PrivCord: package for an independent security audit

This is the file to hand to an auditor, together with a pinned commit. It says what to look at, what we promise, what we
already did, and what we already know is weak. It is written to be read without talking to us first.

Status: **no independent audit has been done yet.** An internal baseline of the cryptography (what it uses, what was tested against official vectors, and the open
questions) is in `docs/CRYPTO_BASELINE.md`; start there. `SECURITY.md` says so too. An earlier internal five-reviewer pass
and its fixes are in the git history; that is not independent.

## 1. What PrivCord is

A private messenger for friends and small groups. There are no accounts and no server that reads messages. Devices
link to each other directly (WebRTC, Bluetooth) and use signaling relays only to find each other. One Rust engine runs
on both apps (Android through uniffi, Windows through Tauri).

## 2. What to audit (priority order)

| # | Area | Where | Rust lines (approx.) |
|---|------|-------|----------------------|
| 1 | Cryptography: hybrid key agreement, per-frame ratchet, sealing, backups, identity | `crates/privcord-crypto` | 10,800 |
| 2 | Engine: handshake, pairing, groups, sync log, files, deletion | `crates/privcord-engine`, `crates/privcord-domain` | 19,000 |
| 3 | Storage and key handling | `crates/privcord-storage`, `apps/android` (Keystore), `apps/desktop/src-tauri` (Credential Manager) | 1,000 + platform code |
| 4 | Untrusted input parsing (frames, cards, files, sync log, Bluetooth reassembly) | `fuzz/fuzz_targets`, the parsers they call | |
| 5 | Platform boundary: uniffi (Android) and Tauri commands (Windows) | `crates/privcord-ffi`, `crates/privcord-desktop-host`, `apps/desktop/src-tauri` | 4,200 |
| 6 | Relay (optional, self-hosted) | `relay/` | about 330 JS |
| 7 | Update mechanism: Windows signed `latest.json` (`apps/desktop/src-tauri/src/updater.rs`, `tauri.conf.json`) and Android in-app installer (`AppUpdater.kt`, `AppUpdate.kt`), plus the publishing job `.github/workflows/test-builds.yml` | | |

Out of scope unless you want them: the landing page (`site/`), the legacy Kotlin "classic" flavour, the third-party libraries themselves (WebRTC, SQLCipher,
RustCrypto crates), and the operating system.

## 3. What we promise (the claims to test)

From `SECURITY.md`; each is a claim an attacker should fail to break:

1. **Hybrid key agreement.** P-256 ECDH plus ML-KEM-768 on every key agreement, so both must be broken.
2. **Per-link channel with forward secrecy.** A new key per frame; old keys erased; frames padded.
3. **Offline messages** are sealed to one-time hybrid keys erased after use. Classic sealing is refused once two people
   have linked (no downgrade).
4. **Relays, STUN and TURN never see plaintext.**
5. **Identity.** 12-word phrase; keys in the OS key store; a paired contact's key is pinned and a different key for the
   same contact is refused.
6. **Encrypted at rest.** SQLCipher (AES-256 over every page), 256-bit random key held only in the OS key store.
7. **Sealed in memory.** Message bodies, file chunks in flight, sync-log state and settings are sealed with a per-run
   random key (AES-256-GCM, labelled as additional data). See `crates/privcord-crypto/src/ramseal.rs` and
   `docs/KNOWN_LIMITATIONS.md`.

## 4. Threat model

**Attackers we design against**

- A network observer or a malicious signaling relay, STUN or TURN server (can see addresses, timing, sizes).
- A malicious peer or group member (can send malformed data, try impersonation through introductions).
- Someone who steals a locked phone or copies the app's files without the OS key store.
- A future attacker with a quantum computer who recorded traffic earlier.

**Not defended** (stated in `SECURITY.md` and `docs/KNOWN_LIMITATIONS.md`)

- A compromised or unlocked device, screenshots, keyboards, OS backups.
- Metadata at relays and TURN (addresses, timing, approximate sizes).
- Deleting data on one device does not delete it on the other.

**Assets:** message and file contents, the identity key and recovery phrase, the contact list and who talks to whom,
the database key.

## 5. Cryptographic inventory

RustCrypto crates (see `crates/privcord-crypto/Cargo.toml`): `aes-gcm`, `p256`, `k256`, `ml-kem` (ML-KEM-768), `hkdf`,
`sha2`, `argon2` (backups). Design documents: `docs/rust/adr/ADR-007` to `ADR-011` (compatibility, X3DH and ratchet,
unsafe code and errors, protocol v2), `docs/IDENTITY_AND_RECOVERY.md`, `docs/GROUPS.md`, `docs/CONNECTIVITY.md`.
Known-answer test vectors: `docs/*-vectors.json`.

We wrote the protocol composition ourselves from these primitives. That composition is the part most in need of review.

## 6. Existing evidence

- Unit and integration tests across the Rust workspace, plus the desktop page driving two real Rust engine processes (`hosted/e2e.test.ts`) and component tests of the screens.
- cargo-fuzz targets for every parser that reads untrusted input (`fuzz/fuzz_targets`: parsers, engine, sync log,
  media, text decoders, Bluetooth reassembly), run in CI with seeds from the test vectors.
- CodeQL (Rust, TypeScript, Python, Actions), `cargo audit`, `npm audit` and Dependabot on every change; `deny.toml`
  holds a `cargo deny` policy.
- Clippy with warnings denied, and `unsafe` rules in ADR-010.

## 7. Known weak spots (so you do not spend time rediscovering them)

- The message index (ids, times, state) and contact and group names are not sealed in memory.
- A whole file is plain in memory briefly while it is read from or written to the database, and while a user picks it.
  On Android, a voice note the user records sits in a temporary file until it is sent.
- Relays and TURN servers see metadata.
- The post-quantum composition has had no independent cryptographic review.
- Group introductions rely on trust in the introducer.
- Real-device behaviour (50 MB transfers, background delivery, 10-member groups, Bluetooth mesh) is only tested in
  simulation and CI, not on a spread of phones.
- Test Android builds are signed with a public debug key; release signing keys are not set up yet.

## 8. How to build and test

```
# Rust workspace
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings

# Fuzzing (nightly): one target for a minute
cd fuzz && cargo +nightly fuzz run parsers -- -max_total_time=60

# Desktop and Android builds: see docs/DEPLOYMENT.md and .github/workflows/ci.yml
```

The desktop app (`apps/desktop/src-tauri`) has its own `Cargo.lock` and is built only in CI. Android's Kotlin compiles in
CI as well.

## 9. What we would like from the audit

1. A written report we may publish, with severities and a fix recommendation for each finding.
2. Specific review of: the hybrid handshake and ratchet composition, nonce and key reuse, the one-time prekey lifecycle
   and downgrade refusal, the backup format and Argon2id parameters, identity derivation from the phrase, parser
   robustness, key handling across the FFI boundary, and the memory-sealing design.
3. A re-check of our fixes, so the final report shows what was fixed.

## 10. For the maintainers: before sending this

- [ ] Pin a commit or tag and tell the auditor which one.
- [ ] Make sure `main` builds and CI is green at that commit.
- [ ] Re-read `SECURITY.md` and `docs/KNOWN_LIMITATIONS.md` so they match what the code does today.
- [ ] Decide the contact address for findings (a private channel, never a public issue).
- [ ] Agree up front whether the report will be published and when (usually after fixes ship).
- [ ] Ask for quotes from at least two firms; the Open Technology Fund's audit programme is worth applying to as well.
