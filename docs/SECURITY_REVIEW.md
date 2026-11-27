# Security review, November 2026

Four read-only reviews of the code (man-in-the-middle, recovery of data from a device, cryptography, hostile contact or
relay). Nothing critical was found: the link authentication, the key handover, the update signatures and the nonce handling
held. What was found, and what was done:

## Fixed
- Windows "Erase everything" left the database key and the sealed settings files behind, so an undeleted `engine.db` still
  opened. It now removes both keys and the settings files.
- A contact who knew a group id could fill its log with unsigned junk and make every change slow, then permanently refuse
  changes at 5000 events. The ordering step is now a heap, and a contact outside the group is limited to 200 events per group.
- The page can no longer read the settings-file key or the automatic-backup passphrase from the credential store.
- A sealed blob longer than we ever produce (64 KB) is refused before it is decoded, so a hostile relay cannot make us decode huge ones.
- Linking a new device by typing or scanning the first device's code: the new device now holds the grant until the person taps "They match" (desktop and Android). Before, the first grant to arrive was taken, so someone who photographed the QR could race the real device.
- An old "keys" message replayed by a relay can no longer set a friend's inbox back: a message from the same install with an older inbox generation than we hold is ignored.
- The Android recovery-phrase screen blocks screenshots, screen recording and the recent-apps preview whatever the setting says.
- A later batch of one-time keys sent through a relay is 5 keys, not 10, so it fits in the anonymous envelope instead of going out with sender and recipient visible (the first batch to a friend is still plain: we do not yet have their key to wrap it with).
- Files kept on a device are capped at 2 GiB in total: a contact's new file offers are declined past that (not covered by a test).
- The relay no longer deletes on request (anyone who could work out a mailbox name from a contact card could wipe what was waiting there). The current apps never asked for deletes; stored messages leave by expiry, and `del` is answered "ok" and ignored so older clients carry on.
- The Windows updater refuses an update whose signature names a different version than `latest.json` announces, or was made before this build: pairing an old signed installer with a higher version number no longer works. (Unit tested; the real updater path was not run on Windows.)
- The group event signature cache is bounded.
- Right-to-left override and zero-width characters are removed from received file names.

## Open (not fixed yet)
- Six words and cards add a contact as directly trusted; the screens say "Verified" only once the safety numbers have been compared, but the invite still lets whoever knows the words race the real friend; friend requests from strangers show a name
  the sender chose.
- A removed device keeps working for a friend who has not yet seen the newer device list; a phone restored from the phrase
  holds the account key, so removing it is cosmetic.
- Message deletion and "clear chat" leave the text in the sync log inside the database until every device has a snapshot (an account with more than one device; the log entries are signed and hash-chained, so they cannot be edited in place).
- Android's app lock covers the screen, not the data; the Keystore keys do not require an unlocked device (requiring it would stop messages arriving while the phone is locked); screen-capture protection is off by default (the recovery phrase screen is always protected).
- No size cap on blobs read from a relay beyond the 64 KB decode limit and the socket's own frame limit.
- The desktop webview can call any engine command; the page is the engine's front end and uses nearly all of them, so an allowlist would add little. There is no XSS sink today and the CSP blocks inline script.
- No wire vectors yet for seal v2, anon v3, the post-quantum channel, device lists or the sync log.
