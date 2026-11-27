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

## Docker

```sh
docker build -t privcord-server -f crates/privcord-server/Dockerfile .
docker run -d --network host -v privcord-data:/data privcord-server
```
(`--network host` keeps the TURN port range simple.)
