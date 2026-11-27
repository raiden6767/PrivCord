#!/usr/bin/env bash
# The same Nostr load against privcord-server and nostr-rs-relay (and strfry, if STRFRY=/path/to/strfry is set):
# start-up time, memory, CPU time, live fan-out, stored events, queries and one-at-a-time latency.
#   NOSTR_RS_RELAY=/path/to/nostr-rs-relay scripts/bench-nostr.sh        (cargo install nostr-rs-relay needs protoc)
set -eu
cd "$(dirname "$0")/.."
PORT=18790
cargo build --release --locked -p privcord-server >/dev/null 2>&1
cargo test --release --locked -p privcord-server --test nostr_load --no-run >/dev/null 2>&1
LOAD=$(ls -t target/release/deps/nostr_load-* | grep -v '\.d$' | head -1)
TMP=$(mktemp -d); trap 'kill $PID 2>/dev/null || true; rm -rf "$TMP"' EXIT
kv() { awk -v k="$2" '$1==k":"{print $2}' /proc/$1/status; }
run() {
  name=$1; shift
  t0=$(date +%s%N); "$@" >"$TMP/$name.log" 2>&1 & PID=$!
  until curl -sf -H 'Accept: application/nostr+json' "http://127.0.0.1:$PORT/" >/dev/null 2>&1; do sleep 0.005; kill -0 $PID 2>/dev/null || { echo "$name: did not start"; cat "$TMP/$name.log" | tail -5; return; }; done
  ms=$(( ($(date +%s%N) - t0) / 1000000 )); sleep 0.5; idle=$(kv $PID VmRSS)
  out=$(NOSTR_URL="ws://127.0.0.1:$PORT" "$LOAD" --ignored --nocapture 2>&1 | grep -E "^(fan-out|store|query|latency)" || echo FAILED)
  peak=$(kv $PID VmHWM); cpu=$(awk '{printf "%.1f", ($14+$15)/100}' /proc/$PID/stat)
  printf '== %s: start %d ms | idle %d kB | peak %d kB | cpu %s s\n%s\n' "$name" "$ms" "$idle" "$peak" "$cpu" "$out"
  kill $PID; wait $PID 2>/dev/null || true
}
cat > "$TMP/p.toml" <<T
[server]
listen = "127.0.0.1:$PORT"
data_dir = "$TMP/p"
[relay]
enabled = false
max_conns_per_ip = 100000
[nostr]
enabled = true
events_per_minute_per_ip = 0
T
run privcord target/release/privcord-server -c "$TMP/p.toml" serve
# The same, confirming each event only after it is safely on disk (what SQLite-based relays do).
sed "s|data_dir = .*|data_dir = \"$TMP/pd\"|; s|^events_per_minute_per_ip = 0|events_per_minute_per_ip = 0\ndurable = true|" "$TMP/p.toml" > "$TMP/pd.toml"
run privcord-durable target/release/privcord-server -c "$TMP/pd.toml" serve
if [ -n "${NOSTR_RS_RELAY:-}" ]; then
  mkdir -p "$TMP/n"
  cat > "$TMP/n.toml" <<T
[info]
relay_url = "ws://127.0.0.1:$PORT/"
[database]
data_directory = "$TMP/n"
[network]
address = "127.0.0.1"
port = $PORT
T
  run nostr-rs-relay "$NOSTR_RS_RELAY" --config "$TMP/n.toml"
fi
if [ -n "${STRFRY:-}" ]; then
  mkdir -p "$TMP/s"
  sed "s|^db = .*|db = \"$TMP/s\"|; s|port = 7777|port = $PORT|; s|bind = \"127.0.0.1\"|bind = \"127.0.0.1\"|" "$(dirname "$STRFRY")/strfry.conf" > "$TMP/s.conf"
  run strfry "$STRFRY" --config "$TMP/s.conf" relay
fi
