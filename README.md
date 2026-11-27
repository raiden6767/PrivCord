<p align="center">
  <img src="docs/brand/privcord-icon.png" alt="PrivCord logo: a teal letter P shaped like a chat bubble, on a dark navy background" width="128">
</p>

# PrivCord: private, end-to-end encrypted messenger with no account, no phone number and no server that reads your chats

[![CI](https://github.com/raiden6767/PrivCord/actions/workflows/ci.yml/badge.svg)](https://github.com/raiden6767/PrivCord/actions/workflows/ci.yml)
![Platforms: Windows and Android](https://img.shields.io/badge/platforms-Windows%20%7C%20Android-blue)
![Rust engine](https://img.shields.io/badge/engine-Rust-orange)
![End-to-end encrypted](https://img.shields.io/badge/chats-end--to--end%20encrypted-brightgreen)
![License: PC-NC](https://img.shields.io/badge/license-PC--NC%20(source--available)-lightgrey)

**Website: [PrivCord, the private messenger with no account and no phone number](https://raiden6767.github.io/PrivCord/)**
 · [Features](https://raiden6767.github.io/PrivCord/features/)
 · [How it works](https://raiden6767.github.io/PrivCord/how/)
 · [Architecture, an interactive map](https://raiden6767.github.io/PrivCord/architecture/)
 · [FAQ](https://raiden6767.github.io/PrivCord/faq/)
 · [Download for Windows and Android](https://raiden6767.github.io/PrivCord/download/)
 · [Support PrivCord](https://raiden6767.github.io/PrivCord/donate/)

**PrivCord is a secure, peer-to-peer messaging app for Windows and Android.** Chats are end-to-end encrypted and go
**straight from device to device** over WebRTC or Bluetooth, so there is **no account to create, no phone number or
email to hand over, and no company server that can read, store or sell your messages**. Add friends with a QR code
or six words, chat one-to-one or in groups of up to 10, send files, join public channels, and keep chatting
**with no internet at all** over a Bluetooth mesh. Use the same account on your phone and your PC with
**multi-device sync** that never lets your account key leave your first device.

If you are looking for a **private chat app without a phone number**, an **anonymous encrypted messenger**, a
**decentralized, serverless-style alternative to Signal, WhatsApp, Telegram, Session or Discord**, or a
**Bluetooth mesh messenger that works offline**, this is built for that. The core is a memory-safe **Rust** engine
shared by both apps.

<p align="center">
  <img src="docs/screenshots/group-chat-dark.png" alt="PrivCord desktop: an encrypted group chat and the chat list, in the dark theme" width="760">
</p>

**Quick links:** [Download](#get-it) · [Features](#what-you-can-do) · [How it connects](#how-it-connects-and-what-it-needs) ·
[Compare](#how-privcord-compares) · [FAQ](#frequently-asked-questions) · [Security and algorithms](#algorithms-and-constants) ·
[For developers](#for-developers) · [License](#license-and-credit)

## What you can do

- **Add a friend in seconds.** Show a QR code or six words; your friend scans the code or types the words, and you tap *Add* when they answer. No long strings to copy.
- **Chat one-to-one** with end-to-end encryption, including a post-quantum hybrid handshake (ML-KEM-768 with P-256). Messages go straight from device to device.
- **Use one account on several devices.** Link your phone, laptop or tablet with a QR code; chats, contacts and read state stay in step with no server in between, and the account key stays on the first device. Remove a lost device in 24 hours (or at once with your recovery phrase). See [multi-device sync](#multi-device-sync-one-account-on-your-phone-and-your-pc).
- **Reply** to any message. The quote shows for everyone, in chats and groups.
- **Send files up to 50 MB.** Photos and videos show a preview you can open; every file can be saved.
  They go straight to the other device, free, with no upload to anyone's server.
- **Send later.** Write a message now and schedule it for a time you pick; reply to a message from its notification (Android).
- **Join public channels** like #general (no account, anyone who types the same name is in the same room). Messages are signed, so nobody can speak as someone else, but anyone who knows the name can read them.
- **Reach people with no internet (Android).** An optional Bluetooth mesh passes messages along through nearby phones, up to 7 hops. Experimental: simulated, never run on real radios.
- **Make it yours.** 16 built-in color themes (Midnight, AMOLED black, Nord, Dracula, Solarized, Blossom pink, Rose, Paper, Sky, Mint and more) plus the classic light/dark look, on both Windows and Android. Settings > Appearance.
- **Lock the app.** Android asks for your fingerprint, face or screen lock; Windows asks for Windows Hello
  (a passkey). Turn it on or off in Settings > Security.
- **Make groups of up to 10.** Create a group, add friends, and anyone in the group can add more people.
  If you add Ben and Ben adds Cat, you and Cat connect automatically. No codes to swap.
- **Stay private in groups.** Turn on *Stay group-only* (Settings > Security) and people you only meet
  through a group can talk to you in the group but cannot message you directly. If either of you has it
  on, you can still pair by swapping invites.
- **Keep working when things go wrong.** Messages you send while someone is offline wait and are sent
  when you reconnect. Connections recover on their own.
- **Back up your chats.** Passphrase-locked backups, with weekly automatic backups to a folder you choose on Android and Windows.
- **Own your identity.** A 12-word recovery phrase is your account. Write it down: the same words
  restore your identity on a new device.
- **Wipe everything** from Settings > Security if you ever need to.

<p align="center">
  <img src="docs/screenshots/add-friend.png" alt="Add a friend in PrivCord: your invite QR code and a field to paste a friend's invite" width="48%">
  <img src="docs/screenshots/group-details.png" alt="PrivCord group details: the members of an encrypted group and who added them" width="48%">
</p>

## Multi-device sync: one account on your phone and your PC

PrivCord keeps your chats in step across your own devices **without a central server** and **without ever copying your
account key off the first device**.

- **Link a device** by scanning the QR code the new device shows with your phone, then typing the 6 digits your phone shows onto the new device. The new device gets
  its own keys; the account's signed device list says who belongs.
- **Sync** runs through an end-to-end encrypted mailbox on relays that only see padded, sealed blobs, directly between
  your devices over Wi-Fi on the local network, or over Bluetooth. Messages, reactions, read state, contacts, groups
  and files all follow you; old history is condensed into snapshots so new devices catch up fast.
- **Friends see one person.** Friends are told your device list and reach each of your devices; a friend on an older
  app still reaches you.
- **Lost a phone?** Remove it from any device: it takes effect after 24 hours (any device can cancel it), or at once
  with your recovery phrase. The sync key is replaced, the friend's keys for that device are dropped, and the
  removed device is told to erase itself.

The full design is in [docs/design/syncengine.md](docs/design/syncengine.md).

## What people use PrivCord for

- **A private chat app that does not need a phone number.** Create an identity from a 12-word phrase and chat with friends who scan your QR code. No SIM, email, username database or sign-up.
- **An encrypted messenger for Windows and Android** with one account on your phone and your PC, and no cloud account to trust.
- **Peer-to-peer chat and file sharing** over WebRTC: photos, videos and files up to 50 MB go straight to the other device, with no upload to a server.
- **Group chat without a server in the middle.** Groups of up to 10 are kept in step by the members' own devices.
- **Messaging with no internet or in a blackout.** Nearby phones pass sealed messages along over a Bluetooth mesh, up to 7 hops (Android, experimental).
- **A self-hosted, privacy-first alternative to WhatsApp, Telegram, Discord and Signal.** Run your own signaling relay and TURN server, or use none and keep to Bluetooth and the local network.
- **A Rust secure-messaging codebase to study.** Memory-safe core, post-quantum hybrid key exchange (ML-KEM-768 with P-256), shared test vectors and fuzzing in CI.

## How PrivCord compares

| | PrivCord | Signal | Session | Briar |
|---|---|---|---|---|
| Phone number or email needed | **No** | Phone number | No | No |
| Messages go through company servers | **No** (device to device; optional relays see only sealed blobs) | Yes (Signal's servers relay them) | Yes (a decentralized network of nodes relays them) | No (Tor, Wi-Fi or Bluetooth) |
| Works with no internet over Bluetooth | **Yes** (Android mesh) | No | No | Yes |
| One account on several devices | **Yes** (account key stays on the first device) | Yes (linked devices) | Limited | Limited |
| Windows and Android apps | **Yes** | Yes (and more) | Yes (and more) | Yes |
| Independently audited | **No, not yet** | Yes | Yes | Yes |

This is a short, good-faith summary that can go out of date; check each project for its current state. PrivCord is a
young project by one maintainer: if you need a battle-tested, audited messenger today, use one of the audited ones,
and read [Known limitations](docs/KNOWN_LIMITATIONS.md) and the [status table](#status-what-is-tested-and-what-is-not).

## How it connects (and what it needs)

PrivCord has **no account system and no server that reads messages**. It is **not serverless**: to find a
friend across the internet it uses a few optional services. They do different jobs, and you can run your own.

| Service | Job | Sees your messages? | Needed? |
|---|---|---|---|
| **Signaling** (public Nostr relays, or your own) | Helps two devices find each other | No (sealed, signed blobs) | For automatic reconnect |
| **STUN** | Tells a device its public address | No | Without it, only same-network links |
| **TURN** | Relays the encrypted stream when a direct link is impossible | No (sees only encrypted data) | Only on strict networks |
| **Store-and-forward relay** | Holds sealed messages for friends who are offline | No | Off by default |
| **Bluetooth LE** | Local link, no internet at all | No | Optional |

The defaults use public third-party STUN and Nostr servers. They can see connection metadata (your IP
address and when you connect), never message content. Change them under *Settings > Advanced*, or host
your own: [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md). The app shows the real connection type
(*direct*, *relayed*, *Bluetooth*), measured from the live connection.

## Get it

The Windows desktop app and the Android apps are built by CI. Open the latest **CI** run on GitHub and
download the artifacts:

| Artifact | What it is |
|---|---|
| `duochat-windows-unsigned` | Windows installer (unsigned, so SmartScreen warns you) |
| `duochat-android` | Android app (`com.duochat.app`), running on the shared Rust engine: install `app-arm64-v8a-release.apk` (almost every phone; `armeabi-v7a` is for old 32-bit phones) |

Android debug builds are signed with one fixed debug key (`apps/android/keystore/debug.keystore`, committed on
purpose, protects nothing), so a newer build updates an older one in place. Building from source:
[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md), [docs/rust/ANDROID.md](docs/rust/ANDROID.md).

## Frequently asked questions

**Does PrivCord need a phone number, email or account?** No. Your identity is a 12-word recovery phrase that you hold.
There is nothing to sign up for.

**Is PrivCord end-to-end encrypted?** Yes. Messages are encrypted on your device and only the friend they are meant for
can open them. Every link is authenticated against the keys you pinned when you added the friend, and the key exchange
uses a post-quantum hybrid (ML-KEM-768 with P-256). The algorithms are listed in full under
[Algorithms and constants](#algorithms-and-constants).

**Is it really serverless?** Not completely, and it says so: to find a friend across the internet it uses optional
signaling relays (public Nostr relays or your own), STUN, and, on strict networks, TURN. None of them can read your
messages. See [How it connects](#how-it-connects-and-what-it-needs) and run your own with [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md).

**Can I use it without internet?** Yes on Android: an optional Bluetooth mesh carries sealed messages through nearby
phones, up to 7 hops. It is experimental and has only been tested in simulation, not on real radios.

**Does it work on iPhone, macOS or Linux?** Not yet. Today there are an Android app and a Windows desktop app.

**Can I use my account on my phone and my computer?** Yes: see [multi-device sync](#multi-device-sync-one-account-on-your-phone-and-your-pc).

**What if I lose my phone?** Remove the device from another device of yours (it takes effect in 24 hours, or at once with
your recovery phrase), and restore your account on a new device with the 12 words.

**Is PrivCord open source?** The full source is here and free for personal, educational and research use. The license
(PC-NC) does not allow commercial use without written permission, so it is *source-available* rather than OSI open
source. See [License and credit](#license-and-credit).

**Has it been audited?** Not by an independent firm yet. The protocol is covered by tests, shared test vectors and
fuzzing in CI, and the security policy is in [SECURITY.md](SECURITY.md).

**Which technologies does it use?** A Rust engine (domain rules, cryptography, storage, state machine) with a Kotlin and
Jetpack Compose app on Android and a Tauri 2 and React app on Windows; WebRTC data channels, Bluetooth LE, Nostr relays,
SQLite, AES-256-GCM, ECDSA and ECDH on P-256, and ML-KEM-768.
**How do I chat privately without giving my phone number?** Install PrivCord, write down your 12-word recovery phrase, and
add a friend by showing a QR code or six words. Nobody is asked for a number, email or name.

**Is there an encrypted messenger that works without internet?** PrivCord's Bluetooth mesh (Android) passes sealed messages
between nearby phones, and linked devices on the same Wi-Fi sync directly with no internet. See
[Channels and mesh](docs/CHANNELS_AND_MESH.md).

**Is PrivCord a Signal, Session, Briar, Element or Telegram alternative?** It covers similar ground (private, encrypted
chat) with a different design: no phone number, device-to-device links instead of a central server, and an offline
Bluetooth mesh. See the [comparison](#how-privcord-compares) and the honest note on audits.

**Can I self-host it?** Yes. The signaling relay is a small Node.js program and the TURN server kit is a Docker Compose
file; see [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) and [relay/README.md](relay/README.md). You can also use no servers at all
on a local network or over Bluetooth.

**Where is my data stored?** Only on your devices, in an encrypted local database (SQLCipher). Your private keys live in
the operating system's key store.

**How do I report a security problem?** Privately, through GitHub's "Report a vulnerability" on this repository; see
[SECURITY.md](SECURITY.md).

## Architecture

PrivCord is one set of rules (identity, contacts, delivery, groups, files, channels, mesh) with thin platform
shells around it. The rules exist in three places today:

| Implementation | Where | Used by |
|---|---|---|
| **Rust engine** (the target) | `crates/` | Android "rust" build and the Windows app (its only engine) |
| TypeScript protocol | `packages/protocol`, `apps/desktop/src/lib` | Desktop app |
| Kotlin protocol | `apps/android/.../{data,security,engine,groups,p2p}` | Nothing at runtime: the Android app runs the Rust engine; kept for its tests and as the reference it was ported from |

The three are kept in step by shared test vectors (`docs/*-vectors.json`): the Rust crates both produce vectors
the TypeScript tests open, and open vectors the TypeScript code produced. CI checks both directions.

### Layers (what each one does)

| Layer | What it does |
|---|---|
| **Identity** | A 12-word BIP-39 phrase derives two P-256 keys: one signs (ECDSA), one agrees on secrets (ECDH). The user id is the SHA-256 fingerprint of the ECDH public key. Same phrase, same identity, on any device. |
| **Contacts** | A friend is a pinned identity card (public keys plus a signature binding them), exchanged by QR, six words or pasted text. `verified` = cards exchanged directly; `introduced` = learned from a group. Safety numbers let two people compare keys out loud. |
| **Links** | One authenticated, encrypted channel per contact: a WebRTC data channel, or Bluetooth LE with its own forward-secret secure channel. After connecting, both sides prove they hold the pinned keys. A ping and a liveness timer drop dead links. |
| **Finding each other** | Sealed (encrypted and signed) connection setup delivered through signaling: Nostr relays or your own relay, or through a mutual friend. Optional store-and-forward for offline friends. |
| **Groups** | A signed event log every member holds and folds the same way. Members can add members (up to 10); an added member's card is the introduction, so everyone connects automatically. Messages go to each member over that member's own link. |
| **Files, channels, mesh** | 50 MB files in 12 KiB chunks over direct links, sent in windows and resumed after a cut-off. Public channels keyed by name. A Bluetooth mesh that floods sealed blobs for up to 7 hops (Android). |
| **Storage** | Local SQLite (schema v3) with message text encrypted at rest on desktop. An outbox keeps unsent messages per recipient and resends when the link opens. |

### Data flow for one message

1. You send. The message is stored and queued once per recipient (one for a friend, one per member for a group).
2. Each recipient with an open link gets it straight over that link and acknowledges it (`SENT` then `DELIVERED`, `READ`).
3. Everyone else stays queued. When a link opens the queue is flushed. With store-and-forward on, a sealed copy can also wait on a relay.

### The Rust engine

```
                    UI (Compose / React)
                           │ typed commands, events, snapshots
                    privcord-ffi  (UniFFI: Kotlin bindings, plain data, stable error codes)
                           │
   privcord-engine ── Core: synchronous, deterministic state machine
   (+ Tokio shell: one thread, bounded command queue → `Busy`, timers, clean shutdown)
        │      │       │
 privcord-domain  privcord-crypto  privcord-ports (traits: Storage, SecureStore, Transport, Clock, Notifier)
 pure rules        keys, seals,        │
                   channels, backup    ├─ privcord-storage (SQLite)
                                       └─ platform adapters: WebRTC, Bluetooth, relay/Nostr sockets, Keystore
```

* The engine never touches an OS API, socket or database directly. Platform code implements the port traits and feeds
  link changes and received frames in (`link_up`, `link_down`, `frame_received`).
* Events are numbered and delivered through bounded queues; a reader that falls behind gets `Lagged` and asks for a
  snapshot, which holds everything a recreated UI needs.
* Signaling clients (relay, Nostr, hub) are sans-IO state machines: the adapter opens sockets and feeds messages in.
* Lints: `unsafe` is forbidden except in the FFI crate, and `unwrap`/`expect`/`panic` are denied in library code.
* Details: [docs/rust/README.md](docs/rust/README.md), [docs/rust/COMPATIBILITY.md](docs/rust/COMPATIBILITY.md).

## Algorithms and constants

Everything below is implemented in `crates/privcord-crypto` (and mirrored in TypeScript and Kotlin).

| Purpose | Construction |
|---|---|
| Recovery phrase | 128-bit CSPRNG entropy → 12 BIP-39 words (132 bits with checksum); optional BIP-39 passphrase |
| Identity keys | BIP-39 seed (PBKDF2-HMAC-SHA-512, 2048 rounds) → HKDF-SHA-256 with a fixed salt and per-purpose info (`identity-sign`, `identity-agreement`) → scalar reduced mod the P-256 order. Signing and agreement keys are never reused for each other |
| User id | SHA-256 fingerprint of the ECDH public key (64 hex characters) |
| Contact card | Public keys plus an identity proof: the signing key signs the agreement key, so nobody can claim an id without holding its keys |
| Signatures | ECDSA P-256 |
| Sealed blob (setup, store-and-forward) | Ephemeral ECDH P-256 to the recipient's agreement key → HKDF-SHA-256 (info `to|from`) → AES-256-GCM (AAD `to|from`, random 96-bit IV) → ECDSA signature over the blob. Opened only if addressed to us, from the pinned sender, and unchanged |
| Mailbox | Hash of the recipient id; relays see sealed blobs addressed to a mailbox, never content |
| Safety number | SHA-256 of both signing-key thumbprints, ordered, shown as six groups of five digits. Detects a swapped key; it is not a substitute for an audited protocol |
| Link authentication (WebRTC) | Challenge-response hello: each side sends keys, an identity proof and a nonce, signs the other's challenge, and checks the keys against the pinned card. 15 s timeout |
| Bluetooth secure channel | Signed ephemeral ECDH hello → HKDF-SHA-256 (salt from both nonces, one label per direction) → AES-256-GCM, 96-bit IV from a counter, AAD `sc|n`. Replays and reordering are rejected. Forward secret |
| Bluetooth tags | `HMAC-SHA-256(contact secret, "…ble-tag-v1|epoch")` cut to 8 bytes, 15-minute epochs; a scanner accepts the previous, current and next epoch. Chunk framing: 3-byte header (flags, sequence), reassembly with ordering checks |
| Six-word invite | Six random BIP-39 words (66 bits) → PBKDF2-SHA-256 (150,000 rounds) → HKDF → rendezvous mailbox and AES-GCM key. Lives 15 minutes; the person who types the words is confirmed by safety numbers |
| Public channel | Normalized name → HKDF key and a hashed mailbox; messages are signed by the sender. Not private: anyone who knows the name can read it |
| Backup | Passphrase (NFKC, at least 8 characters) → PBKDF2-SHA-256 (300,000 rounds) → AES-256-GCM. A wrong passphrase and a damaged file look the same |
| Nostr signaling | BIP-340 Schnorr events on secp256k1 signed with a throwaway key per event; kinds 21733 (live) and 31733 (stored), `t` tag = mailbox, `expiration` tag; only valid, unexpired, unseen events become blobs |
| Group log | Signed events (create, add, remove, rename) that name the events they follow; every device folds them in one deterministic order, so every device reaches the same roster with no server. Events from non-members or breaking a rule (admins only remove others, 10 members) have no effect |
| Mesh | Flooding with a 7-hop limit; seen-set against loops; carry for 30 minutes; at most 30 new messages per minute per sender; 8 KB packets; at most 5 stranger links. Relays carry only sealed or signed blobs and need no keys |
| Files | 50 MB maximum, 12 KiB chunks in windows of 64 answered by the receiver, resumable, SHA-256 checked, at most 2 incoming per peer, direct links only |
| X3DH + Double Ratchet | HKDF-SHA-256 chains, AES-256-GCM with derived IV, bounded skipped keys (1000), envelopes and epoch-keyed group messages. **Ported and tested in both directions, but no app calls them yet** ([ADR-009](docs/rust/adr/ADR-009-x3dh-ratchet.md)) |

Other rules: text up to 4,000 characters; groups up to 10; reactions from a fixed set of six emoji; delete for everyone
leaves a tombstone that survives late copies; signaling reconnect backs off from 1 s, doubling to 30 s; relay puts time
out after 8 s; a link silent for 14 s is dropped and redialed.

## For developers

Want to help? Read [CONTRIBUTING.md](CONTRIBUTING.md) first: how to set up, `scripts/precheck.sh` (run it before every push), and the pull request workflow.

Requires Node.js 20+ for the TypeScript parts and a recent stable Rust (1.82+) for the engine.

```bash
npm install
npm run typecheck
npm test                       # protocol + desktop unit tests
npm run dev:desktop            # UI in the browser at http://127.0.0.1:1420
npm run e2e --workspace @duochat/desktop   # real browsers, relay, Nostr, TURN (needs Playwright)

cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked   # real-relay tests need Node (npm ci) and `pip install nostr-relay`
```

Android (`apps/android`): `./gradlew :app:testDebugUnitTest :app:assembleRelease -Pduochat.ciSign=true` (one APK per phone CPU type; add `-Pduochat.emulator=true` for the x86_64 emulator).
The Rust build needs the native library first: `cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 -o app/src/main/jniLibs build -p privcord-ffi --release`
(Android NDK and `cargo-ndk`). CI does this for you.

The browser build uses a development store that is **not encrypted at rest** and has no Bluetooth; use the Tauri build
(`npm run tauri:dev`, needs Rust) for the real thing. Run your own relay: `cd relay && npm install && node index.js`
([relay/README.md](relay/README.md)).

| Path | What |
|---|---|
| `crates/privcord-domain` | Pure rules and wire shapes: messages, delivery, connection state machine, files, mesh, channels, diagnosis, BLE framing |
| `crates/privcord-crypto` | Identity, signatures, ECDH, sealed blobs, cards, safety numbers, invites, channels, backups, secure channel, BLE tags, Nostr events, X3DH and ratchet |
| `crates/privcord-ports` | Traits the engine needs from the platform, plus in-memory fakes for tests |
| `crates/privcord-engine` | The application engine, the signaling clients and the live-interop peer example |
| `crates/privcord-storage` | SQLite storage: versioned schema, atomic batches, migrations |
| `crates/privcord-import` | Reads the old desktop and Android databases and backups |
| `crates/privcord-ffi` | UniFFI facade and the generated Kotlin bindings |
| `packages/protocol` | Shared TypeScript protocol, unit-tested |
| `apps/desktop` | React + Vite in a Tauri 2 shell (Rust: secure storage, SQLite, Bluetooth), plus the Playwright e2e suite |
| `apps/android` | Kotlin + Jetpack Compose on the Rust engine |
| `relay` | Optional self-hosted signaling / store-and-forward relay (Node.js) |
| `docs` | Architecture, operations and Rust-migration documentation |

## Documentation

[Architecture](docs/ARCHITECTURE.md) · [Groups and introductions](docs/GROUPS.md) ·
[Connectivity](docs/CONNECTIVITY.md) · [Identity and recovery](docs/IDENTITY_AND_RECOVERY.md) ·
[Bluetooth](docs/BLE_TRANSPORT.md) · [Channels and mesh](docs/CHANNELS_AND_MESH.md) · [Deployment](docs/DEPLOYMENT.md) ·
[Rust engine](docs/rust/README.md) · [Rust on Android](docs/rust/ANDROID.md) · [Emergency deletion and panic wipe](docs/EMERGENCY_DELETION.md) · [Relay (TURN) server kit](docs/turn-server/docker-compose.yml) · [Bluetooth mesh field test](docs/MESH_FIELD_TEST.md) · [Known limitations](docs/KNOWN_LIMITATIONS.md) ·
[Android status](docs/ANDROID_STATUS.md) · [Security policy](SECURITY.md)

## Status: what is tested, and what is not

| Area | Tested | Not tested |
|---|---|---|
| Rust engine | Unit and integration tests across all crates (cargo test in CI on Linux and Windows); shared vectors checked in both directions with TypeScript; signaling clients against the real relay and against an independent Nostr relay (`nostr-relay`) | Public Nostr relays |
| Rust engine against the real desktop app | A built desktop app in headless Chromium and a separate Rust engine process talk over a simulated Bluetooth radio: pairing through the app's own dialog, messages both ways, receipts, nothing readable on the air (CI) | WebRTC, groups, files and channels against a live app; a real radio |
| Android "rust" build | Compiles and packages in CI for four CPU types; on a real phone the native library self-test and an engine probe (identity, card, snapshot, Keystore round-trip) pass | **The chat screens on the Rust engine have not been run on a phone.** Messaging, Bluetooth, mesh, manual codes and channels are the likeliest to show bugs |
| Android app | Compiles, unit tests, debug APK builds | The engine and screens on a device or emulator |
| One-to-one messaging over WebRTC | Used on real hardware by the maintainer; automated e2e suite in real browsers | Two devices on different internet connections |
| Groups and introductions | Protocol: 120+ unit tests, a 25-member simulation, offline and forged-event cases. Desktop: real browsers, a three-person introduction and a six-person group | **Android groups on a real phone**, groups larger than 6 on real connections |
| Bluetooth and mesh | Link encryption and framing in code and on a simulated radio; mesh logic in simulation | **Any real Bluetooth radio**. Windows can only search, so a phone must be the other side |
| Importers (old desktop and Android data) | Generated and TypeScript-made fixtures | **Real app data from a device**. Android Room data has never been read from a real phone |
| Windows installer | Built by CI | Run on a clean machine; signing |
| App lock | Desktop: passkey flow against a virtual authenticator. Android: the lock rules | **Real Windows Hello and real Android biometric prompts** |
| Relay | Automated tests | Docker image, production TLS |

Not available: push notifications, files over Bluetooth, and the mesh on Windows
([docs/CHANNELS_AND_MESH.md](docs/CHANNELS_AND_MESH.md), [docs/KNOWN_LIMITATIONS.md](docs/KNOWN_LIMITATIONS.md)).
In the Android Rust build, file progress shows while receiving only, friends' blockers are not explained, and manual
connection codes (`RUST1:`) only work between two Rust builds.

## License and credit

PrivCord is distributed under the **PrivCord Community & Non-Commercial License (PC-NC)**. Personal,
educational and research use is permitted; commercial use needs written permission. See
[LICENSE](LICENSE), [NOTICE](NOTICE), [CREDITS.md](CREDITS.md) and
[COMMERCIAL_LICENSING.md](COMMERCIAL_LICENSING.md).

**Created by [anandkrishna1900](https://github.com/anandkrishna1900)**
([X / Twitter](https://x.com/AnandKrishnaK1)). If you use, modify or redistribute this code, keep the
attribution notices and credit "PrivCord by anandkrishna1900".
