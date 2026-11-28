# TURN relay for PrivCord

Two devices usually connect directly. Some networks (many mobile carriers, office and school Wi-Fi, two strict home
routers) make that impossible; then a TURN server forwards the packets. They are already end-to-end encrypted (DTLS),
so the server sees IP addresses and amounts of traffic, never chats. The apps still prefer a direct path whenever one
works; TURN is only the last resort.

## Run it

On any small VPS with a public IP (about 1 GB RAM is plenty):

1. Copy this folder to the server. In `turnserver.conf`, set `external-ip` to the server's public IP and the password
   in `user=privcord:...` to a long random value (`openssl rand -hex 24`).
2. `docker compose up -d`
3. Open UDP and TCP 3478 and UDP 49152-49252 in the server's firewall.

## Use it

Either way below works; you can do both.

**Built into the apps (everyone gets it, nothing to paste).** In the GitHub repository, add three secrets. CI then bakes
the server into the Android APKs and the Windows app:

| Secret | Value |
|---|---|
| `PRIVCORD_TURN_URLS` | `turn:YOUR_HOST:3478,turn:YOUR_HOST:3478?transport=tcp` |
| `PRIVCORD_TURN_USERNAME` | `privcord` |
| `PRIVCORD_TURN_CREDENTIAL` | the password from `turnserver.conf` |

People can switch it off in Settings > Connection ("PrivCord relay when a direct link fails").

**Shared by hand.** `node make-share-line.mjs YOUR_HOST PASSWORD` prints a `PRIVCORD-TURN1:` line to paste into
Settings > Connection on each device.

## What it costs and limits

The server only carries traffic for pairs that cannot connect directly. Each user is capped at 12 relayed connections
and 1.5 Mbit/s, the whole server at 300 connections and 50 Mbit/s (`turnserver.conf`). Relaying to private and loopback
addresses is refused, so the server cannot be used to reach its own network. A password built into the apps can be
extracted by anyone who has the app; if your server gets used by strangers, change the password and rebuild.

## Several regions (recommended for friends on different continents)

One TURN server far from both friends adds the whole detour to every message and call. Run the same server in two or three
places and list them all: the apps hand the whole list to WebRTC, which tries a direct path first, then every relay, and
keeps the best one that works. Nothing else needs to change, and if one server goes down the others carry on.

```
PRIVCORD_TURN_URLS = turn:sg.example.com:3478,turn:in.example.com:3478,turn:ph.example.com:3478
```

(use the same `PRIVCORD_TURN_USERNAME` and `PRIVCORD_TURN_CREDENTIAL` on every server, or make each server's `user=` line match.)

- **The two friends do not need the same server.** If each end gets a relay address from a different server, the path is
  friend A, the first server, the second server, friend B: that is the "hop" between regions, and it happens by itself.
- **Pick well-connected places.** For India and the Philippines, Singapore sits in the middle with short, stable links to
  both (Mumbai also works for the Indian side). Places with unreliable international links (Myanmar, for example) make a poor
  relay even though they are close on the map.
- **Keep the list short (three at most).** Each device asks every listed server for a relay address when a link starts, so a
  long list costs every server an allocation per device, and delays the first connection.
- **The apps do not yet pick the nearest server by measuring.** They try all of them; ordering by measured round-trip time is
  not built.
