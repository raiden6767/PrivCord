# Unlinking: what to try on real devices

The engine side is covered by tests (`crates/privcord-engine/tests/sync.rs`, "Unlinking scenarios"); this is what only
real devices can show. Use a phone (holds the account key), a laptop and, if you have one, a third device. Send a
screenshot or the crash text for anything that does not match "Expect".

| # | Do | Expect |
|---|---|---|
| 1 | On the laptop: Settings > Devices > remove the phone-linked third device, no phrase | "Will be removed <date>" shows on every device within a minute; nothing else changes yet |
| 2 | From the phone, take that removal back | The "Will be removed" line disappears on the laptop too, and it does not come back |
| 3 | Ask again; switch the phone off; wait for the date to pass (or ask a friend to set the clock) | Still pending while the phone is off; carried out within a minute of the phone coming back |
| 4 | Remove a device with the recovery phrase, wrong phrase first | A sentence ("That is not your account's recovery phrase..."), never a raw code like `InvalidPhrase`; right phrase removes at once |
| 5 | If the account was restored with a passphrase: remove with phrase only, then with the passphrase field filled | Phrase only is refused with the sentence above; with the passphrase it works (both apps have the field now) |
| 6 | Keep the removed device switched off, then switch it on with internet | It erases itself and starts at the first screen (setup), no crash, no loop; the update notice still shows |
| 7 | Remove a device while another device is off; switch that one on later | It catches up: the removed device is gone from its list and new changes (a new friend) appear on it |
| 8 | After removal, set the same hardware up again and link it | It joins as a new device and syncs; the old entry stays removed |
| 9 | With a friend: remove a device, then message the friend from the removed one before it connects | The friend does not see the message; the friend's other chats with you carry on |
| 10 | Try to remove the device you are using, and the phone (account key holder) from the laptop without the phrase | A clear sentence for each; nothing breaks. The phone needs the phrase to be removed |
| 11 | Rotate the phone (landscape) and switch apps while the "Remove" dialog and the phrase field are open | Dialog and typed text survive; no crash |

Not testable from here: how long a device must be offline before people notice, vendor battery managers, real relays.
