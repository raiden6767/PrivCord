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

### Durable mode

By default an event is confirmed as soon as it is in memory and handed to the operating system, and the log is synced to disk
twice a second: a crash of the program loses nothing, a power cut can lose up to half a second. Set `durable = true` and an
event is confirmed (and shown to subscribers) only after it has been synced to disk. Events that arrive together share one
sync ("group commit"), so most of the speed stays: on this machine stored events ran at 28-35k a second against 38-41k
without it. A slow disk will cost more; the benchmark script measures both on yours.

### Compared with nostr-rs-relay

`scripts/bench-nostr.sh` (with `NOSTR_RS_RELAY=/path/to/nostr-rs-relay`) runs the same load against it, this server, and this
server in durable mode. One run on a shared 4-vCPU container, load generator on the same machine, default settings for
nostr-rs-relay and rate limits off here:

| | nostr-rs-relay 0.8.12 | privcord-server | privcord-server, durable |
|---|---|---|---|
| start until it answers | 22-800 ms | 7-33 ms | 8-9 ms |
| idle memory | 24 MB | 5 MB | 5 MB |
| 20,000 live events to 200 rooms | 8.7k deliveries/s | 37-42k/s | 18-39k/s (noisy) |
| 20,000 events stored from 20 connections | 0.5-1.5k/s | 38-41k/s | 28-35k/s |
| 2,000 reads of 50 events, one after another | 19/s | 2.0-2.8k/s | 2.1k/s |
| one event at a time, median / 99th percentile | 0.37 ms / 1.8 ms | 0.39 ms / 1.2 ms | 0.39 ms / 1.7 ms |
| peak memory / CPU time for the whole run | 78-98 MB / 39 s | 46 MB / 4.5 s | 46 MB / 4.6 s |

Read it carefully:
- nostr-rs-relay stores in SQLite, a full database; this server keeps events in memory with a log on disk. Memory use grows
  with the number of stored events (cap them with `max_events` and `retention_days`), and a restart reads the log back.
- **The read figure is mostly a socket setting.** nostr-rs-relay leaves Nagle's algorithm on (checked with `strace`: it sets
  `TCP_NODELAY` to 0), so a burst of 50 small replies waits on the client's delayed ACK. This server turns it off.
- Latency for single events is a tie; the gains are in throughput, memory and CPU.
- Run-to-run variation on this shared machine is large (the same durable run scored 38k and 18k live deliveries); the
  figures show the order of magnitude, not exact ratios.
- strfry, the most used relay, was not measured here. The script runs it too if you give it `STRFRY=/path/to/strfry`.

### What was learned from nostr-rs-relay

I read its source (event handling, storage, limits, configuration) and adopted what fits PrivCord:

| Adopted | |
|---|---|
| Confirm after the write is safe | `durable = true` (group commit) |
| A deletion that arrives before its event still wins | and for addresses (`a` tags): older versions that arrive later are refused |
| Direct messages only to their sender and recipient | `auth_dms = true` (NIP-42; the challenge is sent when a client connects) |
| Cap on how fast one connection opens subscriptions | `reqs_per_minute_per_conn` |
| Cap on events per second for the whole relay | `events_per_second_global` (protects the disk) |
| Prometheus counters | `[server] metrics = true`, at `/metrics` (needs the token if tokens are set) |
| Client address from a chosen header | `[server] real_ip_header = "cf-connecting-ip"` behind Cloudflare, with `trust_proxy` |
| Small compatibility details | subscription ids up to 256 characters, kind 41 replaceable (NIP-28), admin key in the information document, memory-only mode (`persist = false`) |

Left out on purpose: NIP-05 write verification (needs outbound web requests from the server), pay-to-relay, an external
admission service, and logging each client's address and browser (this server keeps no such log).

## TLS (wss, https, and TURN over TLS)

Put a certificate in `[tls]` and the web side can also listen over TLS (`https_listen = "0.0.0.0:443"`), so apps connect with
`wss://` and you need no reverse proxy:

```toml
[tls]
cert_file = "/etc/letsencrypt/live/example.org/fullchain.pem"
key_file  = "/etc/letsencrypt/live/example.org/privkey.pem"
domain    = "example.org"
https_listen = "0.0.0.0:443"
```

The files are read again every 30 seconds when they change, so a renewed certificate is picked up without a restart (a broken
new file is ignored and the old certificate stays). Getting the certificate is yours to do (certbot, your host's tool, or
a reverse proxy such as Caddy that handles it for you); this server does not talk to Let's Encrypt itself yet.
`self_signed = true` makes a certificate for trying things out; apps and browsers will not trust it.

## Behind a router, a home connection, or a cloud NAT

TURN is the part that is hard to run behind NAT, because phones must be able to reach it from the internet.

1. **Ports.** Forward (or allow in your cloud firewall) **TCP 8787** (or 443 via TLS) for the relay, **UDP 3478** and the
   **UDP range `min_port`-`max_port`** (default 49152-49400, one port per relayed connection) for TURN.
2. **Networks that block UDP.** Turn on `tcp_listen` (TURN over TCP, for example `0.0.0.0:3478`) and `tls_listen` (TURN over
   TLS, for example `0.0.0.0:443`, which gets through networks that only allow HTTPS). Forward those TCP ports too. The
   phone's own leg goes over TCP or TLS; what the server sends on to the other phone is still UDP, so the UDP range must
   stay open. Phones are told every way in (`/turn` lists `turn:`, `turn:...?transport=tcp` and `turns:...`).
3. **Public address.** `public_ip = "auto"` asks a public STUN server what address the internet sees, and tells phones to
   use it. On a cloud VM with a private address and a 1:1 public one, this is what you want. If your address changes,
   restart the server (or use a dynamic-DNS name and set the IP by hand).
4. **Two phones on the same server** relay to the server's own public address; many routers cannot loop traffic back to
   themselves, so the server delivers it locally instead (this is tested below).
5. **Carrier-grade NAT.** If your provider shares one public address between customers (`doctor` says so, or your router's
   WAN address starts 100.64-100.127), nothing can reach you from outside. Run the server on a small VPS instead, or put a
   tunnel (a VPS with the ports forwarded over WireGuard) in front.
6. **Testing from your own network.** Many routers cannot loop traffic back to their own public address ("hairpin NAT"),
   so test with a phone on **mobile data**, not Wi-Fi.

### Tested behind NAT (an emulation)

`sudo scripts/nat-lab.sh` builds a small internet out of network namespaces: two phones behind two separate NAT routers
(real Linux conntrack), the internet between them, and this server behind its own gateway with port forwards (a VPS with a
private address, or a home server). It runs the real server and a real TURN client. Result on this machine:

| Scenario | Result |
|---|---|
| two ordinary home routers (cone NAT): can the phones reach each other directly (UDP hole punching)? | yes |
| two symmetric NATs: direct connection | no (this is when TURN is needed) |
| two home routers: TURN over UDP through the forwarded server | works |
| two symmetric NATs: TURN over UDP | works |
| both offices block UDP: TURN over UDP | fails, as it must |
| both offices block UDP: TURN over TCP | works |
| only TCP 443 gets out: TURN over TCP 3478 | fails, as it must |
| only TCP 443 gets out: TURN over TLS 443 | works |
| one phone behind a UDP-blocking office, the other behind a symmetric NAT (TCP) | works |

The same lab against the server as it was before the local-delivery fix (point `SRV_BIN` at an older build): both UDP relay
scenarios fail, so that fix is what makes two phones on one port-forwarded server work.

What this does not show: a real ISP or carrier (some do odd things to UDP), a real phone, the real internet's latency and
loss, IPv6. It shows the server's logic is right in the situations that make TURN hard.

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

## Docker

```sh
docker build -t privcord-server -f crates/privcord-server/Dockerfile .
docker run -d --network host -v privcord-data:/data privcord-server
```
(`--network host` keeps the TURN port range simple.)
