# The diagnostic log

A log of what the app did, kept on the device only, so that a problem seen on a real phone or computer can be found. Nothing is
uploaded anywhere. It is on by default (Settings > Diagnostic log, on both apps) and can be turned off, which also empties it.

## What is in it
One line per thing that happened, oldest first, in plain words with the time (UTC) and where it came from:

| where | what |
|---|---|
| `engine` | every event the engine published (a message arrived, a status changed, a dial was requested, a request came in) |
| `link` | links going up, down or failing (with the reason and the route), connection setup sent |
| `relay` | what was posted to a relay and whether it worked, what was received (its kind, not its content), mailboxes listened on, a message handed to a relay again |
| `send` | messages sent from the waiting queue, or not |
| `invite` | "please add me back" asks |
| `call` | every action a person took through the app (send, add a friend, remove, link a device...), how long it took and how it ended |
| `ui` | scanner opened/closed/read a code, buttons pressed (the kind of control and the dialog it was in, never its text), Save-log taps |
| `nav` | which screen and dialog is open (a dialog named after a friend or group is just "a dialog") |
| `app` | the app going to the background and back, the window losing focus, the engine starting or stopping |
| `net` | the phone's network arriving (wifi or mobile data) or going; the Windows app going online or offline |
| `perm` | permission answers |
| `ble` | the Bluetooth radio: advertising, scans, a friend's tag seen, connection status codes |
| `error` | anything that failed or threw, and crashes |

The same event repeating within two seconds is counted (`×50`) instead of listed.

## What is never in it
Message text, names, file names or contents, keys, recovery words, six-digit codes, addresses. Friends and mailboxes appear as
short aliases (`id-1`, `id-2`, the same alias for the same one throughout one log); anything that looks like an IPv4 or IPv6
address becomes `<ip>`; long unbroken strings become `<long>`. The screens log sizes (`chars=24`), switches and counts, not text.
This is tested (`crates/privcord-engine/tests/diag.rs`, `apps/desktop/src/lib/diag.test.ts`).

## Where it lives and how big it gets
In the encrypted database, in chunks of 400 lines, with the newest chunk the only one rewritten as it grows. Up to 20,000 lines
are kept; older ones are dropped (and counted at the top of the file). "Erase everything" and the panic wipe remove it with the
rest of the database.

## Saving it
Settings > Diagnostic log: **Save the log (.md)** writes a Markdown file where you choose; **Save it compressed (.md.gz)** does
the same, gzip-compressed, for a long log. To report a problem, send the file with what went wrong and roughly when. The
Markdown file can be read as it is.

## Adding to it
Engine code: `self.diag("category", "what happened", &format!("particulars"))` (`crates/privcord-engine/src/core/diag.rs`).
Android: `Diag.log("ui", "what", "detail")`. Windows: `diag('ui', 'what', 'detail')`. Never put text a person wrote, a name,
a key or an address in an entry.
