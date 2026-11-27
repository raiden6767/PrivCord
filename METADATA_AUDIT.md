# Metadata audit: what leaks, to whom

Content is end-to-end encrypted. This page is about everything else: information that is not encrypted, or that tells an
observer something about a person even though it is. It was written from the code, not from the design documents, and
each finding says how sure it is.

**Confidence:** **V** = read in the code and confirmed. **I** = follows from how the pieces work, not directly tested.
**D** = stated in the project's own documents.

This is an internal review, not an independent audit (see `AUDIT.md`).

## Who might be watching

| Observer | Can see by design |
|---|---|
| A relay operator (the three default public Nostr relays, or your own) | Your IP address, what you subscribe to, what is posted, and when |
| STUN and TURN servers | Your IP address and when you start connecting |
| GitHub | Your IP address, app version and platform, when you check for updates or download |
| Anyone on the same network, or your ISP | That you connect to those hosts, when, and how much |
| A contact or past contact | Your public ID and name, plus your IP address on a direct connection |
| Anyone who has ever had your ID (an invite you showed, a group you were in) | Your public ID |
| Someone nearby | Bluetooth and local-network broadcasts |
| Anyone looking at your unlocked phone or PC | Notifications, window titles |

## Status after the fixes in this change

| # | Status |
|---|---|
| M1 | **Fixed** for relay posts when the other app has handed over its keys (anonymous envelope, version 3: no `to`, `from` or signature on the outside). First contact and apps that do not support it still use the old shape. |
| M2 | **Mostly fixed.** Once keys are exchanged, posts go to a private inbox derived from the receiver's own secret, which nobody can work out from the public ID. It now **rotates when a contact is removed** (the last two older ones are still listened to, so nobody is left behind), and a device whose keys arrived under the account address is now filed under its own address (that was why one device never got the private inbox). **Still open:** until a contact's keys arrive, the first handover post goes to the public mailbox, and older apps never use the private inbox. |
| M3 | Mitigated by M1 and M2 only. |
| M4 | **Not fixed** (device fan-out). |
| M5 | **Not fixed, on purpose.** All stored posts already share one 7-day expiry. Live signals (2 minutes, a separate event kind) cannot be made to look the same without relays keeping connection offers and network addresses for days. |
| M6 | **Fixed**: default STUN is Cloudflare only (no Google). |
| M7 | **Mitigated**: update check every 6 hours instead of more often; still on by default. |
| M8 | **Fixed**: notification previews off by default. |
| M9 | By design. |
| M10 | **Fixed**: a linked phone defaults to the name "My phone". |
| M18 | **Fixed (Windows)**: invites, the six words and recovery phrases are cleared from the clipboard after 30 seconds if still there. Not checked on a real Windows machine; Windows clipboard history is not covered. |
| M11 | **Fixed (Windows)**: a setting turns the unread count off in the window title, tray tooltip and taskbar badge. On by default. |
| M14 | **Fixed (Android)**: crash reports have hosts, URLs, addresses and long ids replaced before they are saved. Class names and the stack stay. |
| M16 | **Already handled**: the recording is in the app's private cache folder, deleted right after it is sent and swept at launch. It is not encrypted while it exists. |
| Invite beacon (found in the capture) | **Partly fixed**: the six-word invite now stops when the Add a friend screen is closed (it used to run up to 15 minutes), and posts every 12 s instead of every 4 s after the first minute. The inviter's public ID is still in the clear in each post (changing that is an invite format change). |
| Self-hosted relay | **Hardened**: only a listener of a mailbox may delete from it; one address may hold at most 100 of a mailbox's stored messages (so it cannot fill it); a connection may listen to 64 mailboxes (was 16); rate limit 600 per minute (was 120). |
| M12, M13, M15, M17 | Not changed. |

## Findings

### High

| # | What leaks | To whom | Conf. | Fix |
|---|---|---|---|---|
| **M1** | **Sender and recipient IDs are in the clear in every sealed message** (`to` and `from`, plus the signature), stored or live. The body is encrypted, the addressing is not. | A relay: who talks to whom, how often | **V** (`seal.rs`, `prekeys.rs`) | Move both inside the encrypted part ("sealed sender"). Needs a protocol version bump. |
| **M2** | **The mailbox is a hash of your public ID.** The ID is not secret: every contact, past group member and anyone who saw your invite has it, so any of them can work out your mailbox and watch when something is posted to you. The code comment says hashing keeps the ID out of relay logs; that holds only for someone who does not already have the ID. | Any past contact; relays | **V** (`seal.rs`) | A mailbox per contact pair, derived from a shared secret, rotating. |
| **M3** | **A relay ties your IP address to the mailboxes you listen on.** With M1 and M2 it can build a map of who is connected to whom. Default relays are public. | Relay operators | **I** | Hide M1 and M2 first. Tor mode (exists), your own relay, a random subset of several relays. |

### Medium

| # | What leaks | To whom | Conf. | Fix |
|---|---|---|---|---|
| **M4** | **One post per recipient device at the same moment**, so a relay can count how many devices someone has. | Relay | **V** (`signaling.rs`, one put per address) | One mailbox per contact (M2) removes the per-device fan-out. |
| **M5** | **Message type is visible.** The event kind separates live signals from stored messages, and the expiry tag shows 2 minutes, 10, 15 or 7 days. | Relay | **V** (`nostr.rs`, TTL constants) | One kind and fixed expiry buckets. |
| **M6** | **STUN servers** (Google and Cloudflare by default) see your IP and when connections start. | Google, Cloudflare | **V** (default config), **I** | A STUN-free option, or your own. Already skipped behind a relay and in Tor mode. |
| **M7** | **Update checks** tell GitHub your IP, version and platform. On by default; off behind a relay or Tor. | GitHub | **V** | Documented in the settings text. Keep; consider asking once. |
| **M8** | **Notification previews are on by default**, on both apps. Message text appears on the lock screen and in the system's notification history. | Anyone near the device; the OS | **V** (`notificationPreviews = true`) | Default off, or hide while locked. The privacy checkup already flags it. |
| **M9** | **Public channels**: the name is hashed to a mailbox anyone can guess. Posts show the poster's name. By design public, but not obvious from the UI. | Anyone | **V** (`channel.rs`) | State it on the join screen (it does) and keep channels visually distinct. |

### Low

| # | What leaks | To whom | Conf. | Fix |
|---|---|---|---|---|
| **M10** | **Device name defaults to the phone model** (for example `V2348`), shown in the device list that contacts can see. | Contacts | **V** (`Build.MODEL` default) | Default to "My phone". |
| **M11** | **Window title, tray tooltip and taskbar badge show the unread count.** | Screen-share viewers, anyone nearby | **V** (`useWindowEffects.ts`) | Option to hide the count. |
| **M12** | **Bluetooth**: advertisement tags are pairwise and rotate (good), but the mesh service identifier is fixed, so a scanner learns "a PrivCord phone is nearby". | Anyone nearby | **V** (`ble.rs`), **D** | Documented in the setting text. Off by default. |
| **M13** | **Local-network beacons** are encrypted with a fresh nonce (good); the fixed prefix shows "PrivCord, multi-device" on that network. | Same network | **V** (`selfbox.rs`) | Minor. |
| **M14** | **Crash reports** (shared by hand) hold the phone model and up to 24,000 characters of stack trace; an error message can name a relay host. No messages or names are written on purpose. | Whoever you send it to | **V** (`CrashLog.kt`) | Strip host names from error text before saving. |
| **M15** | **Log lines** record relay messages (host names), never message content. | Someone with a debug connection | **V** | Minor. |
| **M16** | A recorded **voice note** sits in a plain temporary file until sent. | Someone with file access to the phone | **D** (`AUDIT.md`) | Write it encrypted or delete on a timer. |
| **M17** | **Message size.** Current (v2) messages are padded to buckets; older (v1) ones are not, so their length shows. | Relay | **V** for v2 (`pad`), **I** for v1 | Retire v1 for new messages. |
| **M18** | **Windows does not clear the clipboard** after copying an invite, the six words or a recovery phrase, so it stays there for any program to read (and in Windows clipboard history if that is on). | Other programs; clipboard history | **V** (no clear in desktop code) | Clear it after about 30 seconds if unchanged, as on Android. |
| **M19** | **Windows: your local network address reached contacts after the first voice note.** Chromium hides it behind a random `.local` name, unless the page holds a *lasting* microphone permission, which WebView2 saves by default. Measured in Chromium: a one-time grant keeps it hidden, a saved grant exposes it, clearing the grant hides it again. | A contact you connect to | **V** (Chromium test), **I** (WebView2 saving the answer) | The app now answers the microphone request itself without saving it, and clears a saved one (`mic_permission.rs`). Compiles for Windows; **not tried on a real Windows machine**. Android's WebRTC library is untested. |

## Sweep for leaks and critical issues (2026-11-27)

Fixed here: Android release builds no longer write dial, signal, relay-put or relay-notice lines to logcat (they said when
something happened and what relays answered; readable over USB debugging). The `cookie` crate in the desktop build is
updated to 0.18.3 (RUSTSEC-2026-0594; it was not reachable on Windows, which is not in the advisory's list of affected systems).

Open, by design or needing a decision:
- **TURN credentials in public builds.** If a release is built with `PRIVCORD_TURN_*` set (CI secrets), the username and
  credential sit in the app and in the web bundle as plain text, so anyone can extract and use that TURN server, and its
  operator sees the traffic of everyone relayed through it. Fix: short-lived credentials issued per session. Not done.
- `glib 0.18` unsound iterator (RUSTSEC-2024-0429) and several unmaintained crates (`bincode`, `paste`, `basic-toml`, `chrono`,
  `zbus`/`zvariant`) arrive through Tauri's Linux stack or build tools; nothing to bump without a Tauri upgrade.
- Windows file names such as `CON` or `NUL` are not rejected when a received file is saved (the save dialog still decides the path).

Looked at and found clean: no hard-coded keys or secrets in tracked files; npm audit (production) is clean for the relay,
desktop app, site and protocol package; `cargo audit` on the engine workspace has no vulnerabilities; no `unsafe` in the
engine crates (one block, the Windows microphone handler); secret types print as redacted and are zeroed on drop; SQL is
parameterised; the page cannot read the engine's keys through the Tauri commands and saved settings only accept known
names; file names from other people are cleaned; the engine's memory stayed flat across 2,400 idle ticks, invite
create/stop cycles and contact add/remove cycles (checked in a test run, not a long soak).

## Checked and clean

- No analytics, advertising, crash-reporting or Google Play services libraries in the Android or desktop builds.
- The desktop app's content-security policy allows only itself and secure or plain WebSocket connections; no outside fonts, scripts or images in the app or the website.
- Android: backups off (`allowBackup=false`), plain-text traffic off, settings (including relay and TURN credentials)
  encrypted with the Keystore, database encrypted. Windows: keys in the Credential Manager.
- Each relay post is signed with a **throwaway key**, so events cannot be linked to your identity by key.
- The diagnostics report holds counts only (not host names, names or messages).
- Bluetooth tags are derived per contact and rotate; local-network beacons are encrypted and unlinkable.
- Android clears the clipboard after an invite or connection code is copied (`copySensitive`).

## Not covered

What a relay operator keeps in their own logs; timing-correlation attacks by someone who watches both ends; what the
website's host (GitHub Pages) logs about visitors; the contact's view of your IP on a direct link (inherent; "Hide my IP"
and Tor mode exist for it); operating-system, keyboard and assistant leaks outside the app.

## Suggested order

1. **M1 and M2 together** (sealed sender and per-contact mailboxes). They are the large leak and fix M4, M5 and most of
   M3. Do it in one protocol version, with the other message-format changes.
2. **M8** (previews off by default) and **M10** (generic device name): small, no protocol change.
3. **M11**, **M14**, **M16**, **M17**, **M18**: small clean-ups.
4. **M6**: decide whether a STUN-free default is acceptable for connection success.
