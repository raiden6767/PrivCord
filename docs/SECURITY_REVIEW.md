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
- Right-to-left override and zero-width characters are removed from received file names.

## Open (not fixed yet)
- Linking a new device by scanning the host's QR: the new device accepts the first grant before the codes are compared. Needs a
  confirm step on the new device (UI change). Anyone who photographed the QR in its 10 minutes could race the host.
- Six words and cards mark a contact "Verified" without the safety-number check; friend requests from strangers show a name
  the sender chose.
- A removed device keeps working for a friend who has not yet seen the newer device list; a phone restored from the phrase
  holds the account key, so removing it is cosmetic.
- Message deletion and "clear chat" leave the text in the sync log inside the database until every device has a snapshot.
- Android's app lock covers the screen, not the data; the Keystore keys do not require an unlocked device; screen-capture
  protection is off by default.
- The relay's `del` only needs a subscription to the mailbox, which anyone who has a contact card can make.
- No total storage cap for files and messages from contacts; no size cap on blobs read from a relay.
- The anonymous envelope falls back to a plain one when the inner blob is over about 34 KB (the first handover is).
- "keys" payloads carry no counter, so a relay can replay an old one (this can undo an inbox rotation).
- The Windows updater's manifest version is not signed (rollback needs write access to the release); test builds use a public
  debug keystore.
- The desktop webview runs any engine command a script could call; there is no XSS sink today, but there is no allowlist either.
- No wire vectors yet for seal v2, anon v3, the post-quantum channel, device lists or the sync log.
