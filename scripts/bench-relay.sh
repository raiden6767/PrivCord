#!/usr/bin/env bash
# Same load against the Node relay and the Rust relay: start-up time, idle and peak memory, throughput, CPU time.
#   scripts/bench-relay.sh [pairs] [msgs-per-pair]     (needs: cargo, node, relay/node_modules)
set -eu
cd "$(dirname "$0")/.."
PAIRS=${1:-1000}; MSGS=${2:-20}; PORT=18787
cargo build --release --locked -p privcord-server >/dev/null 2>&1
cargo test --release --locked -p privcord-server --test load --no-run >/dev/null 2>&1
LOAD=$(ls -t target/release/deps/load-* | grep -v '\.d$' | head -1)
TMP=$(mktemp -d); trap 'kill $PID 2>/dev/null || true; rm -rf "$TMP"' EXIT
cat > "$TMP/s.toml" <<T
[server]
listen = "127.0.0.1:$PORT"
data_dir = "$TMP/data"
[relay]
store_and_forward = false
rate_limit = 1000000
max_conns_per_ip = 100000
max_per_mailbox_per_ip = 100000
max_queued = 10000000
T
kv() { awk -v k="$2" '$1==k":"{print $2}' /proc/$1/status; }
run() {
  name=$1; shift
  t0=$(date +%s%N); "$@" >/dev/null 2>&1 & PID=$!
  until curl -sf "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; do sleep 0.005; done
  ms=$(( ($(date +%s%N) - t0) / 1000000 )); sleep 0.5; idle=$(kv $PID VmRSS)
  out=$(LOAD_URL="ws://127.0.0.1:$PORT" LOAD_PAIRS=$PAIRS LOAD_MSGS=$MSGS "$LOAD" --ignored --nocapture 2>&1 | grep delivered || echo "FAILED")
  peak=$(kv $PID VmHWM); cpu=$(awk '{printf "%.2f", ($14+$15)/100}' /proc/$PID/stat)
  printf '%-6s start %4d ms | idle %6d kB | peak %7d kB | cpu %5s s | %s\n' "$name" "$ms" "$idle" "$peak" "$cpu" "$out"
  kill $PID; wait $PID 2>/dev/null || true
}
run node env PORT=$PORT RATE_LIMIT=1000000 MAX_CONNS_PER_IP=100000 MAX_PER_MAILBOX_PER_IP=100000 MAX_QUEUED=10000000 node relay/index.js
run rust target/release/privcord-server -c "$TMP/s.toml" serve
