# Cryptography baseline (internal review)

This is a code read and test run by the developers' assistant, written to give an independent auditor a starting point.
**It is not an independent audit.** `AUDIT.md` still says no independent audit has been done, and that remains true.

## Is it real or a placeholder?

Real. Every primitive comes from a maintained library; nothing is hand-rolled.

| Need | What is used | Where |
|---|---|---|
| Authenticated encryption | AES-256-GCM (`aes-gcm`) | `aead.rs` |
| Key agreement | ECDH P-256 (`p256`) and ML-KEM-768 (`ml-kem`), combined | `pq.rs`, `pq_channel.rs` |
| Signatures | ECDSA P-256, deterministic (RFC 6979) | `identity.rs` |
| Key derivation | HKDF-SHA-256 (`hkdf`); Argon2id for backups (`argon2`); BIP-39 for the phrase | `kdf.rs`, `backup.rs`, `identity.rs` |
| Randomness | The operating system's generator (`getrandom`) | `rng.rs` |

`placeholder_epk` in `seal.rs` is not a crypto stand-in: it fills the public-key field of envelopes that carry no key
exchange (invites, channels, self-sync), so every blob has one shape.

## What was checked, and the result

- **Hybrid handshake** (`pq_channel.rs`, `pq.rs`): both sides send a signed hello with a fresh P-256 key, a fresh
  ML-KEM-768 key and a nonce; each checks the signature against the pinned contact; both encapsulate to the other. The
  root key is HKDF over the ECDH secret and both ML-KEM secrets, salted with a length-prefixed hash of the whole
  transcript (both hellos and both ciphertexts) in an order both sides agree on. Breaking it needs both P-256 and ML-KEM.
- **Frames:** a one-way HKDF chain per direction gives every frame its own key (old keys erased); the nonce is the frame
  number; associated data binds the number; plaintext is padded to buckets. A nonce is never reused under a key.
- **Randomness:** only the OS generator in release builds. The deterministic test generator exists only behind the
  `test-support` feature, which is enabled only in dev-dependencies (and the separate fuzz workspace), never in a build
  that ships. There is no `unsafe` in the crate.
- **Official test vectors, run on our wrappers** (`crates/privcord-crypto/tests/known_answers.rs`):
  HKDF-SHA-256 (RFC 5869 test case 1), AES-256-GCM (McGrew and Viega test case 16, with associated data, plus tamper
  checks), and **ML-KEM-768 key generation and encapsulation against NIST ACVP vectors** (first 8 of each; data in
  `tests/data/mlkem768-acvp.json`).
- **Cross-implementation agreement:** AES-GCM, ECDH, ECDSA and HKDF agree byte for byte with WebCrypto (TypeScript) and
  the JCA (Kotlin) through `docs/*-vectors.json`, three independent implementations of the standards.
- **Backups:** Argon2id with 64 MiB, 3 passes, 1 lane (v2). The older PBKDF2 (300,000 rounds) form is read for
  compatibility and only written for test vectors.

## Known limits and open questions (for the auditor)

1. **`ml-kem` is pre-1.0 and its own README says it "has never been independently audited".** Our NIST vector run shows
   it is functionally correct for key generation and encapsulation; it says nothing about side channels (timing). This
   is the single most important item for an outside reviewer.
2. **Not vector-tested yet:** ML-KEM decapsulation from NIST's full-key form (our API stores the 64-byte seed), ECDSA
   P-256 against RFC 6979's vectors through our own signing code, and the hybrid combiner (no external vectors exist for
   our specific composition; only our own).
3. **Symmetric ratchet only.** Forward secrecy within a session holds (old frame keys are erased). There is no
   Diffie-Hellman ratchet, so a key leaked mid-session exposes later frames of that session until the next handshake
   (post-compromise security is limited to reconnecting).
4. **Frame skip allowance.** A frame may be up to 50,000 ahead of the last; each costs two HKDF steps to skip. Only an
   already authenticated contact can send frames, so this is a minor cost, not an open door.
5. **The composition is ours.** The protocol is built from standard parts, but the way they are joined (labels,
   transcript binding, prekey lifecycle, downgrade refusal) has had no outside review. That composition is what an
   audit should spend most time on.
6. **Labels still say "duocord".** Domain-separation strings carry the old project name. Harmless, but changing them
   would break compatibility with existing links, so it needs a version bump if wanted.
7. **Metadata** (who talks to whom, when, roughly how much) is visible to relays and TURN servers by design;
   see `docs/KNOWN_LIMITATIONS.md`.

## How to re-run

```
cargo test -p privcord-crypto            # all crypto tests, including the official vectors
cargo test -p privcord-crypto --test known_answers
```
