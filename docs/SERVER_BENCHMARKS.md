# Server benchmarks: what was measured, what was not

One shared 4-vCPU container, load generator on the same machine, release build. Run it yourself:
`scripts/bench-relay.sh [pairs] [msgs]` (`SERVER_PIN=0 LOAD_PIN=1,2,3` pins the server to one core, roughly a Raspberry Pi class
of single-core speed; it is not a Pi: memory bandwidth and storage differ).

## Measured

| | PrivCord server (Rust) | Node relay (this repo) | coturn 4.6 (apt) |
|---|---|---|---|
| binary on disk | 8.8 MB, one file, no libraries | needs Node 22 (~100 MB) | 0.5 MB + shared libraries |
| start until it answers | 7-11 ms | 72-150 ms | not timed |
| idle memory (relay) | 7.4-8 MB | 62-67 MB | 16.9 MB (TURN only) |
| 20,000 x 2 KiB messages, 200 pairs, server on all cores: CPU time / peak memory | 0.28 s / 35 MB | 0.89 s / 94 MB | n/a |
| the same, server pinned to one core: CPU time / peak memory | 0.22 s / 49-54 MB (after write batching; 0.37 s before) | 0.90 s / 77 MB | n/a |
| the same run: wall time | 0.2-1.05 s (19k-96k msg/s, varies run to run) | 0.65-0.75 s (27k-31k msg/s) | n/a |
| delivery time under that burst, median | 40-133 ms | 2-14 ms | n/a |
| one message at a time, median | 0.13 ms | 0.12 ms | n/a |

## What this says, honestly

* Footprint and CPU cost: the Rust server wins clearly (a quarter to a half of the CPU, a tenth of the idle memory, starts in
  under 10 ms, one file to copy). That is what matters on a Pi or an old laptop.
* Burst latency is **not** a win. When 400 connections flood 20,000 messages at once, this server's median delivery time is
  tens to over a hundred milliseconds, against a few milliseconds for the Node relay, although the server is not CPU-bound
  (0.22 s of CPU in about 1 s). One message at a time they tie. The cause is not found. TCP_NODELAY is on, backpressure only starts at 32 MiB, and
  flushing once per message was one cost (fixed: frames waiting behind one now go out together), but the median did not move.
  The README's earlier guess that the test client is the limit is unproven. This needs a measurement by a client that is not
  this test (for example `tcpdump` timestamps, or another client). Treat "faster than Node" as true for CPU and memory only.
* The earlier "FAILED" in the pinned run was the benchmark's own settings: the per-address connection cap added with the
  hardening (100 HTTP connections from one address) dropped the load test's 400 connections. The script now lifts it.

## Not measured (do not claim)

* Other Nostr relays (strfry, nostr-rs-relay): not built or run here; the sandbox did not allow building third-party code.
  Run the same load at them with `NOSTR_URL=ws://host:port cargo test -p privcord-server --release --test nostr_load -- --ignored --nocapture`.
* TURN throughput and latency against coturn: no TURN load generator exists yet; only start-up and idle memory of coturn were looked at.
* A real Raspberry Pi, a Chromebook, SD-card or eMMC storage, Wi-Fi, long runs (days), memory growth over time.
* Behaviour under real attack traffic (`tests/hardening.rs` covers slow-header, silent and many-connection cases only).
