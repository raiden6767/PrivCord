# Connecting, reconnecting and syncing: what was found in the real-device tests

Reports from three phones and a laptop: a scan that reads the code but then nothing starts; a link that works once and
does not come back after the app is closed; direct messages that stop while public chat still works; Bluetooth never
finding a friend; a public channel missing on the laptop.

## Found in the code and fixed
- **A subscription only asked relays for the last two minutes.** Anything sent while the other phone was closed, asleep or
  offline for longer than that was never handed over, even with store-and-forward on. It now reaches back a week (the most a
  stored message lives); signals that old are expired and dropped. (Android, Windows and the browser build.)
- **A friend request lived in memory only.** If the person scanned, and the other phone was closed or restarted before the
  tap, the request was gone. Requests waiting for a tap, and the "please add me back" asks being sent for half an hour after
  a scan, are now kept across restarts.
- **Messages handed over again are not handled twice.** Because of the longer reach, the list of what was already handled is
  now kept across restarts too.
- **A joined public channel never reached the account's other devices.** It is now kept like the other settings that travel
  between devices, and channels joined earlier are put in the log once.
- **The QR code was small and dense.** An invite is about 90 squares across, shown 3 cm wide with a 1-square margin on a
  possibly dim screen. It is now larger (most of the screen's width), has the standard margin, and the phone's screen goes
  to full brightness while it is shown. The Windows code is larger too.
- **The scanner's capture button sat under the navigation bar** on some phones; its room at the bottom and its size now
  follow the screen.
- **The Connection page said different things on the two apps.** Android now opens with the same status sentence and the
  same "N of M friends connected" line as Windows.

## Not fixed, and why
- **Bluetooth.** The radio code says it was never run on real hardware, and it needs two friends to search and advertise at
  the same moment. It cannot be debugged without a device, so Settings now shows "What Bluetooth did lately" (advertising
  started or refused, scans, a friend's tag seen, connection status codes). Next test: open it on both phones and read it.
  It links friends you already have; it does not discover strangers.
- **Many relay connections.** Every mailbox the app listens on (yours, your private inbox, each channel, each open invite)
  opens its own set of sockets to each public relay. A busy phone can hold a dozen. Public relays may refuse that. Sharing one
  socket per relay is a larger change; do it if the diagnostics show refusals.
- **Direct messages across networks.** A direct link between phones on different mobile networks needs a TURN server; without
  one, and with store-and-forward off, messages wait until the friend is reachable. Store-and-forward is off by default
  (a privacy choice); turning it on in Connection makes messages wait on relays for friends who are offline.
- A first connection in the browser build sometimes stalls about 40 seconds (3 of 6 runs of the end-to-end test, on the
  unchanged code too). The apps handle a lost offer sooner (the friend says it is ready and the offer is sent again), so
  this is the development build's, but it has not been looked into further.
