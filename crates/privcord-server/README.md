# privcord-server

A relay and, if you want one, a TURN server for PrivCord, in one program that you run yourself.

* **Relay**: a mailbox for sealed messages. It carries blobs it cannot read and keeps them (if you allow it) for up to
  seven days, so people who are offline still get their messages.
* **TURN** (optional): carries voice and file traffic between two phones that cannot reach each other directly (two
  different mobile networks, strict routers).

Turn either or both on in the settings file.

> **You are responsible for your server.** Whoever runs this program is the operator of that server, of the communities
> that use it, and of what is done with it. The PrivCord authors do not run it, cannot see it, and are not responsible
> for it. The relay cannot read message contents, but you still hold addresses of connecting clients, timing and sizes:
> follow the privacy and communications rules where you live, and take legal advice before opening a server to the public.

## Quick start

```sh
privcord-server init --turn     # writes privcord-server.toml
privcord-server doctor          # checks ports, your public address, NAT
privcord-server serve
```

Then in the apps, add `ws://YOUR-ADDRESS:8787` as a relay (use `wss://` behind a TLS proxy such as Caddy:
`reverse_proxy 127.0.0.1:8787`, and set `trust_proxy = true`). TURN is `turn:YOUR-IP:3478` with credentials from
`https://YOUR-HOST/turn` (short-lived) or a fixed `name:password` in `turn.users`.

## Nostr

Set `[nostr] enabled = true` and the same port also speaks the Nostr relay protocol (a client that sends a JSON array is
talking Nostr; one that sends an object is talking PrivCord). The PrivCord apps already use Nostr events (kinds 21733 and
31733) on public relays, so this server works as one of them too, and any other Nostr client can use it.

| | |
|---|---|
| Supported | NIP-01 (events, REQ/CLOSE/EOSE, filters, replaceable, ephemeral and addressable kinds), NIP-02, NIP-09 deletion, NIP-11 information document, NIP-12 tag queries, NIP-13 proof of work, NIP-15, NIP-16, NIP-20, NIP-22, NIP-33, NIP-40 expiration, NIP-42 authentication, NIP-45 COUNT |
| Checks | every event's id and Schnorr signature, size, tag count, `created_at` window, expiry; hex must be lowercase |
| Rules you set | allowed and blocked public keys, allowed and blocked kinds, minimum proof of work, authentication required, per-address publish rate, retention (days), event cap |
| Not yet | NIP-50 search, NIP-86 management API, NIP-98 HTTP auth, NIP-77 sync, NIP-70 protected events, NIP-05 write checks |
| Storage | in memory with indexes by time, author, kind and tag, plus an append-only log in `data_dir` (synced to disk twice a second), compacted when it doubles |

Whoever publishes to a Nostr relay publishes readable content: unlike PrivCord's sealed messages, you can read what is stored
here. That is your responsibility as the operator.

### Compared with nostr-rs-relay

`scripts/bench-nostr.sh` (with `NOSTR_RS_RELAY=/path/to/nostr-rs-relay`) runs the same load against both. One run on a
shared 4-vCPU container, load generator on the same machine, default settings for both (rate limits off on this server):

| | nostr-rs-relay 0.8.12 | privcord-server |
|---|---|---|
| start until it answers | 22 ms | 9-13 ms |
| idle memory | 25 MB | 5 MB |
| 20,000 live events to 200 rooms (200 listeners): time | 2.6 s (7.7k deliveries/s) | 0.55 s (36k deliveries/s) |
| 20,000 events stored from 20 connections | 13.4 s (1.5k events/s) | 0.5 s (40k events/s) |
| 2,000 reads of 50 events each, one after another | 105 s (19/s) | 0.8 s (2.5k/s) |
| one event at a time, median / 99th percentile | 0.35 ms / 0.7-1.8 ms | 0.37 ms / 0.8-1.8 ms |
| peak memory / CPU time for the whole run | 98 MB / 38-40 s | 46 MB / 4.4 s |

Read it carefully:
- **Durability is not identical.** nostr-rs-relay stores in SQLite, which is a database with queries and indexes of its own;
  this server keeps events in memory with a log that is synced to disk twice a second, so a power cut can lose up to half a
  second of events (a crash of the program loses none). Memory use grows with the number of stored events (cap them with
  `max_events` and `retention_days`).
- **The read figure is mostly a socket setting.** `nostr-rs-relay` leaves Nagle's algorithm on (checked with `strace`: it
  sets `TCP_NODELAY` to 0), so a burst of 50 small replies waits on the client's delayed ACK, about 50 ms a query. This server
  turns it off. Different clients and networks change this figure.
- Latency for single events is a tie; the gains are in throughput, memory and CPU.
- strfry, the most used relay, was not measured here (building it needs system packages I did not install). The script runs
  it too if you give it `STRFRY=/path/to/strfry`.

## Behind a router, a home connection, or a cloud NAT

TURN is the part that is hard to run behind NAT, because phones must be able to send UDP to it from the internet.

1. **Ports.** Forward (or allow in your cloud firewall) **TCP 8787** (or 443 via the proxy) for the relay, **UDP 3478**
   and the **UDP range `min_port`–`max_port`** (default 49152–49400, one port per relayed connection) for TURN.
2. **Public address.** `public_ip = "auto"` asks a public STUN server what address the internet sees, and tells phones to
   use it. On a cloud VM with a private address and a 1:1 public one, this is what you want. If your address changes,
   restart the server (or use a dynamic-DNS name and set the IP by hand).
3. **Carrier-grade NAT.** If your provider shares one public address between customers (`doctor` says so, or your router's
   WAN address starts 100.64–100.127), nothing can reach you from outside. Run the server on a small VPS instead, or put a
   tunnel (a VPS with the ports forwarded over WireGuard) in front.
4. **Testing from your own network.** Many routers cannot loop traffic back to their own public address ("hairpin NAT"),
   so test with a phone on **mobile data**, not Wi-Fi.
5. **TURN is UDP only** in this version. A phone on a network that blocks all UDP cannot use it (TCP/TLS TURN comes later).

## Safety defaults

* TURN refuses to relay to loopback, private, link-local, carrier-grade-NAT and multicast addresses, so it cannot be used
  to reach the machine it runs on or the network behind it (`allow_private_peers = false`).
* TURN credentials expire (default 1 h); there are no accounts to leak. Fixed accounts exist only if you add them.
* `access_tokens` makes the server private: clients must present a token.
* Per-address limits on connections, mailboxes, subscriptions, message rate; caps on total messages and bytes stored.
* `GET /healthz` shows counters only; `GET /.well-known/privcord-server` tells apps what the server offers.

## Capacity

Everything is sharded and lock-light, with no per-message disk wait. On a laptop (client and server sharing the same
machine), 1,000 listeners and 1,000 senders moved 20,000 messages of 2 KiB in 1.3 s (about 15,000 messages/s), every one
delivered, slowest 0.23 s. Run it on your own hardware:
`cargo test -p privcord-server --release --test load -- --ignored --nocapture` (`LOAD_PAIRS`, `LOAD_MSGS` set the size).
Raise the open-files limit (`ulimit -n 65536`, or `LimitNOFILE=` in systemd) for many thousands of connections. TURN
capacity is bounded by your network bandwidth; use `max_kbps` to cap each relayed connection.

### Against the Node relay

`scripts/bench-relay.sh [pairs] [messages]` runs the same load (same limits, 2 KiB messages) against the Node relay and
this one, and prints start-up time, memory, CPU time and throughput. One run on a shared 4-vCPU container, the load
generator on the same machine:

| | Node 22 | Rust |
|---|---|---|
| start until it answers | 74-103 ms | 9-10 ms |
| idle memory | 63 MB | 5 MB |
| 200,000 messages (200 listeners, 200 senders): time | 5.9 s (34k msg/s) | 1.15 s (174k msg/s) |
| the same run: CPU time / peak memory | 6.1 s / 94 MB | 2.7 s / 61 MB |
| 20,000 messages across 2,000 connections: CPU time / peak memory | 1.55 s / 94 MB | 0.58 s / 25 MB |
| the same run: time (the load generator is the limit) | 14k msg/s | 16k msg/s |
| one message at a time, median / 99th percentile (separate Node client) | 0.12 ms / 0.97 ms | 0.13 ms / 0.67 ms |

Honest notes: a flood is slowed at the door (a sender's socket is not read while too much is waiting to be written), which
keeps memory flat; a listener that cannot keep up is cut off past a byte limit, as in the Node relay. The load test also
prints delivery times measured at the Rust test client; under a burst they are higher for this server (about 50 ms
median at 2,000 connections against under 1 ms for Node). I believe that is the test client being flooded faster than it can
read, since one-at-a-time delivery is a tie, but I have not proven it. Numbers vary by machine: run it on yours.

## Nostr

Set `[nostr] enabled = true` and the same port also speaks the Nostr relay protocol (a client that sends a JSON array is
talking Nostr; one that sends an object is talking PrivCord). The PrivCord apps already use Nostr events (kinds 21733 and
31733) on public relays, so this server works as one of them too, and any other Nostr client can use it.

| | |
|---|---|
| Supported | NIP-01 (events, REQ/CLOSE/EOSE, filters, replaceable, ephemeral and addressable kinds), NIP-02, NIP-09 deletion, NIP-11 information document, NIP-12 tag queries, NIP-13 proof of work, NIP-15, NIP-16, NIP-20, NIP-22, NIP-33, NIP-40 expiration, NIP-42 authentication, NIP-45 COUNT |
| Checks | every event's id and Schnorr signature, size, tag count, `created_at` window, expiry; hex must be lowercase |
| Rules you set | allowed and blocked public keys, allowed and blocked kinds, minimum proof of work, authentication required, per-address publish rate, retention (days), event cap |
| Not yet | NIP-50 search, NIP-86 management API, NIP-98 HTTP auth, NIP-77 sync, NIP-70 protected events, NIP-05 write checks |
| Storage | in memory with indexes by time, author, kind and tag, plus an append-only log in `data_dir` (synced to disk twice a second), compacted when it doubles |

Whoever publishes to a Nostr relay publishes readable content: unlike PrivCord's sealed messages, you can read what is stored
here. That is your responsibility as the operator.

### Compared with nostr-rs-relay

`scripts/bench-nostr.sh` (with `NOSTR_RS_RELAY=/path/to/nostr-rs-relay`) runs the same load against both. One run on a
shared 4-vCPU container, load generator on the same machine, default settings for both (rate limits off on this server):

| | nostr-rs-relay 0.8.12 | privcord-server |
|---|---|---|
| start until it answers | 22 ms | 9-13 ms |
| idle memory | 25 MB | 5 MB |
| 20,000 live events to 200 rooms (200 listeners): time | 2.6 s (7.7k deliveries/s) | 0.55 s (36k deliveries/s) |
| 20,000 events stored from 20 connections | 13.4 s (1.5k events/s) | 0.5 s (40k events/s) |
| 2,000 reads of 50 events each, one after another | 105 s (19/s) | 0.8 s (2.5k/s) |
| one event at a time, median / 99th percentile | 0.35 ms / 0.7-1.8 ms | 0.37 ms / 0.8-1.8 ms |
| peak memory / CPU time for the whole run | 98 MB / 38-40 s | 46 MB / 4.4 s |

Read it carefully:
- **Durability is not identical.** nostr-rs-relay stores in SQLite, which is a database with queries and indexes of its own;
  this server keeps events in memory with a log that is synced to disk twice a second, so a power cut can lose up to half a
  second of events (a crash of the program loses none). Memory use grows with the number of stored events (cap them with
  `max_events` and `retention_days`).
- **The read figure is mostly a socket setting.** `nostr-rs-relay` leaves Nagle's algorithm on (checked with `strace`: it
  sets `TCP_NODELAY` to 0), so a burst of 50 small replies waits on the client's delayed ACK, about 50 ms a query. This server
  turns it off. Different clients and networks change this figure.
- Latency for single events is a tie; the gains are in throughput, memory and CPU.
- strfry, the most used relay, was not measured here (building it needs system packages I did not install). The script runs
  it too if you give it `STRFRY=/path/to/strfry`.

## Behind a router, a home connection, or a cloud NAT

TURN is the part that is hard to run behind NAT, because phones must be able to send UDP to it from the internet.

1. **Ports.** Forward (or allow in your cloud firewall) **TCP 8787** (or 443 via the proxy) for the relay, **UDP 3478**
   and the **UDP range `min_port`–`max_port`** (default 49152–49400, one port per relayed connection) for TURN.
2. **Public address.** `public_ip = "auto"` asks a public STUN server what address the internet sees, and tells phones to
   use it. On a cloud VM with a private address and a 1:1 public one, this is what you want. If your address changes,
   restart the server (or use a dynamic-DNS name and set the IP by hand).
3. **Carrier-grade NAT.** If your provider shares one public address between customers (`doctor` says so, or your router's
   WAN address starts 100.64–100.127), nothing can reach you from outside. Run the server on a small VPS instead, or put a
   tunnel (a VPS with the ports forwarded over WireGuard) in front.
4. **Testing from your own network.** Many routers cannot loop traffic back to their own public address ("hairpin NAT"),
   so test with a phone on **mobile data**, not Wi-Fi.
5. **TURN is UDP only** in this version. A phone on a network that blocks all UDP cannot use it (TCP/TLS TURN comes later).

## Safety defaults

* TURN refuses to relay to loopback, private, link-local, carrier-grade-NAT and multicast addresses, so it cannot be used
  to reach the machine it runs on or the network behind it (`allow_private_peers = false`).
* TURN credentials expire (default 1 h); there are no accounts to leak. Fixed accounts exist only if you add them.
* `access_tokens` makes the server private: clients must present a token.
* Per-address limits on connections, mailboxes, subscriptions, message rate; caps on total messages and bytes stored.
* `GET /healthz` shows counters only; `GET /.well-known/privcord-server` tells apps what the server offers.

## Capacity

Everything is sharded and lock-light, with no per-message disk wait. On a laptop (client and server sharing the same
machine), 1,000 listeners and 1,000 senders moved 20,000 messages of 2 KiB in 1.3 s (about 15,000 messages/s), every one
delivered, slowest 0.23 s. Run it on your own hardware:
`cargo test -p privcord-server --release --test load -- --ignored --nocapture` (`LOAD_PAIRS`, `LOAD_MSGS` set the size).
Raise the open-files limit (`ulimit -n 65536`, or `LimitNOFILE=` in systemd) for many thousands of connections. TURN
capacity is bounded by your network bandwidth; use `max_kbps` to cap each relayed connection.

### Against the Node relay

`scripts/bench-relay.sh [pairs] [messages]` runs the same load (same limits, 2 KiB messages) against the Node relay and
this one, and prints start-up time, memory, CPU time and throughput. One run on a shared 4-vCPU container, the load
generator on the same machine:

| | Node 22 | Rust |
|---|---|---|
| start until it answers | 80-105 ms | 8-11 ms |
| idle memory | 63 MB | 5 MB |
| 200,000 messages (200 listeners, 200 senders): time | 4.75 s (42k msg/s) | 1.02 s (195k msg/s) |
| the same run: CPU time used by the server | 5.0 s | 2.3 s |
| 20,000 messages across 2,000 connections: peak memory | 90-109 MB | 30-43 MB |
| one message at a time, median / p99 delivery (separate client) | 0.11 ms / 1.10 ms | 0.13 ms / 0.78 ms |

Honest notes: with very fast senders the Rust relay queues events faster than the listeners drain them, so its peak memory
there (146 MB) was higher than Node's (96 MB); a slow listener is cut off past a byte limit, as in the Node relay. The
"slowest delivery" the load test prints is dominated by the Rust test client sharing the machine's cores, not by the
server; the one-at-a-time figure above, from an independent Node client, is the fair latency comparison. Numbers vary by
machine: run it on yours.

## Docker

```sh
docker build -t privcord-server -f crates/privcord-server/Dockerfile .
docker run -d --network host -v privcord-data:/data privcord-server
```
(`--network host` keeps the TURN port range simple.)
