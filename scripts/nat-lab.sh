#!/usr/bin/env bash
# A small lab of network namespaces that behaves like the internet in the ways that matter to a TURN server:
#
#   phone A (192.168.1.2) --- NAT A ---+                                    +--- gateway (203.0.113.10) --- server (10.0.0.2)
#                                      +--- the internet (a router) --------+        port forwards, as on a VPS or at home
#   phone B (192.168.2.2) --- NAT B ---+
#
# The NATs are Linux conntrack: "cone" (the common home router), "symmetric" (a new port for every destination, the kind
# that defeats hole punching), "udp-blocked" (an office that blocks UDP) and "tls-only" (only TCP 443 gets out).
# It runs the real server and a real TURN client (examples/turn_probe) and prints what worked.
#
#   sudo scripts/nat-lab.sh          needs: root, iproute2, iptables, python3 (a Linux box or a container with those rights)
#
# This is an emulation: real NAT code, but no real internet, ISP or phone. It shows the logic is right, not that a given
# carrier behaves.
set -u
cd "$(dirname "$0")/.."
[ "$(id -u)" = 0 ] || { echo "run as root (it makes network namespaces)"; exit 1; }
for t in ip iptables python3 curl; do command -v $t >/dev/null || { echo "need $t"; exit 1; }; done
cargo build --release -q -p privcord-server --bin privcord-server --example turn_probe 2>&1 | grep -E "^error" -A5
SRV=${SRV_BIN:-$PWD/target/release/privcord-server}; PROBE=$PWD/target/release/examples/turn_probe
W=$(mktemp -d); NSS="srv gw inet natA natB phA phB"
ex() { local ns=$1; shift; ip netns exec "$ns" "$@"; }
cleanup() { if [ -n "${LAB_KEEP:-}" ]; then echo "lab kept (W=$W). Remove with: for n in srv gw inet natA natB phA phB; do ip netns del \$n; done; pkill -f 'privcord-server -c'; pkill -f rv.py"; return; fi; for p in $(cat "$W"/pids 2>/dev/null); do kill $p 2>/dev/null; done; for n in $NSS; do ip netns del $n 2>/dev/null; done; rm -rf "$W"; }
trap cleanup EXIT
trap 'exit 130' INT TERM
for n in $NSS; do ip netns del $n 2>/dev/null; ip netns add $n; ex $n ip link set lo up; done
link() { # ns1 if1 addr1 ns2 if2 addr2
  ip link add "$2" type veth peer name "$5"; ip link set "$2" netns "$1"; ip link set "$5" netns "$4"
  ex "$1" ip addr add "$3" dev "$2"; ex "$1" ip link set "$2" up; ex "$4" ip addr add "$6" dev "$5"; ex "$4" ip link set "$5" up; }
link inet i-gw 203.0.113.1/24 gw gw-i 203.0.113.10/24
link gw gw-s 10.0.0.1/24 srv s-gw 10.0.0.2/24
link inet i-na 198.51.100.1/24 natA na-i 198.51.100.2/24
link natA na-p 192.168.1.1/24 phA pa-n 192.168.1.2/24
link inet i-nb 192.0.2.1/24 natB nb-i 192.0.2.2/24
link natB nb-p 192.168.2.1/24 phB pb-n 192.168.2.2/24
ex gw ip route add default via 203.0.113.1; ex srv ip route add default via 10.0.0.1
ex natA ip route add default via 198.51.100.1; ex natB ip route add default via 192.0.2.1
ex phA ip route add default via 192.168.1.1; ex phB ip route add default via 192.168.2.1
for n in inet gw natA natB; do ex $n sysctl -qw net.ipv4.ip_forward=1; done
# The gateway: the server's public address is the gateway's; these ports are forwarded to the server.
ex gw iptables -t nat -A POSTROUTING -s 10.0.0.0/24 -o gw-i -j SNAT --to-source 203.0.113.10
for rule in "udp 3478" "tcp 3478" "tcp 443" "tcp 8443" "tcp 8787" "udp 9999"; do set -- $rule
  ex gw iptables -t nat -A PREROUTING -d 203.0.113.10 -p $1 --dport $2 -j DNAT --to-destination 10.0.0.2; done
ex gw iptables -t nat -A PREROUTING -d 203.0.113.10 -p udp --dport 49152:49400 -j DNAT --to-destination 10.0.0.2
nat() { # ns lan wan mode
  local ns=$1 lan=$2 wan=$3 mode=$4
  ex $ns iptables -F; ex $ns iptables -t nat -F; ex $ns iptables -P FORWARD DROP
  ex $ns iptables -A FORWARD -i $wan -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
  # A real router does not answer unsolicited traffic aimed at its own WAN address (it would also leave stray state that
  # changes the ports a later outgoing flow gets).
  ex $ns iptables -F INPUT; ex $ns iptables -A INPUT -i $wan -j DROP
  case $mode in
    cone) ex $ns iptables -A FORWARD -i $lan -j ACCEPT; ex $ns iptables -t nat -A POSTROUTING -o $wan -j MASQUERADE;;
    symmetric) ex $ns iptables -A FORWARD -i $lan -j ACCEPT; ex $ns iptables -t nat -A POSTROUTING -o $wan -j MASQUERADE --random-fully;;
    udp-blocked) ex $ns iptables -A FORWARD -i $lan -p udp -j DROP; ex $ns iptables -A FORWARD -i $lan -j ACCEPT
                 ex $ns iptables -t nat -A POSTROUTING -o $wan -j MASQUERADE;;
    tls-only) ex $ns iptables -A FORWARD -i $lan -p tcp --dport 443 -j ACCEPT; ex $ns iptables -t nat -A POSTROUTING -o $wan -j MASQUERADE;;
  esac; }
# The server, and a tiny "what address do you see me at" service for the hole-punching test.
cat > $W/srv.toml <<T
[server]
listen = "0.0.0.0:8787"
data_dir = "$W/data"
[relay]
store_and_forward = false
[tls]
self_signed = true
domain = "turn.lab.test"
https_listen = "0.0.0.0:8443"
[turn]
enabled = true
listen = "0.0.0.0:3478"
tcp_listen = "0.0.0.0:3478"
tls_listen = "0.0.0.0:443"
public_ip = "203.0.113.10"
min_port = 49152
max_port = 49400
T
ex srv $SRV -c $W/srv.toml serve > $W/server.log 2>&1 & echo $! >> $W/pids
cat > $W/rv.py <<'P'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.bind(("0.0.0.0", 9999))
while True:
    d, a = s.recvfrom(100); s.sendto(("%s:%d" % a).encode(), a)
P
ex srv python3 $W/rv.py > /dev/null 2>&1 & echo $! >> $W/pids
for i in $(seq 1 50); do ex srv curl -sf http://10.0.0.2:8787/healthz >/dev/null 2>&1 && break; sleep 0.2; done
CREDS=$(ex srv curl -s http://10.0.0.2:8787/turn); U=$(echo "$CREDS" | python3 -c 'import sys,json;print(json.load(sys.stdin)["username"])'); P=$(echo "$CREDS" | python3 -c 'import sys,json;print(json.load(sys.stdin)["credential"])')
CA=$W/data/tls/selfsigned.crt
echo "server: $(head -n 1 $W/server.log)"; echo

# --- can the two phones reach each other directly (UDP hole punching)? ---
cat > $W/punch.py <<'P'
import socket, sys, time, os
me, wait = sys.argv[1], sys.argv[2]
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.bind(("0.0.0.0", 0)); s.settimeout(0.3)
for _ in range(10):
    s.sendto(b"who", ("203.0.113.10", 9999))
    try: mapped, _ = s.recvfrom(100); break
    except socket.timeout: pass
else: print("NOMAP"); sys.exit(2)
open(me, "w").write(mapped.decode()); print("MAPPED", mapped.decode(), flush=True)
for _ in range(100):
    if os.path.exists(wait): break
    time.sleep(0.1)
peer = open(wait).read().split(":"); peer = (peer[0], int(peer[1]))
end = time.time() + 8; got = None
while time.time() < end:
    s.sendto(b"punch", peer)
    try:
        d, a = s.recvfrom(100)
        if d == b"punch" and got is None: got = time.time()
    except socket.timeout: pass
    # keep sending a little after hearing from the other side, so it can hear us too
    if got and time.time() > got + 2.5: print("PUNCHED"); sys.exit(0)
print("NOPUNCH"); sys.exit(1)
P
direct() { # modeA modeB -> yes / no (can two phones reach each other directly?)
  nat natA na-p na-i $1; nat natB nb-p nb-i $2; rm -f $W/a.addr $W/b.addr
  ex phA python3 $W/punch.py $W/a.addr $W/b.addr > $W/pa.out 2>&1 & pa=$!
  ex phB python3 $W/punch.py $W/b.addr $W/a.addr > $W/pb.out 2>&1 & pb=$!
  wait $pa; ra=$?; wait $pb; rb=$?
  if [ -n "${LAB_DEBUG:-}" ]; then
    for n in natA natB; do echo "  --- $n conntrack (udp):" >&2; ex $n cat /proc/net/nf_conntrack 2>&1 | grep udp | cut -c1-190 | sed 's/^/  | /' >&2; done
  fi
  [ $ra = 0 ] && [ $rb = 0 ] && echo yes || echo no; }
# --- can they relay through the server? ---
relay() { # modeA modeB transport port -> ok / FAIL
  nat natA na-p na-i $1; nat natB nb-p nb-i $2
  local extra=""; [ $3 = tls ] && extra="--ca $CA --sni turn.lab.test"
  ex phB $PROBE --transport $3 --server 203.0.113.10:$4 --user "$U" --pass "$P" --peer-ip 203.0.113.10 $extra listen > $W/rb.out 2>&1 & local pb=$!
  for i in $(seq 1 60); do grep -q "^RELAY\|^FAIL" $W/rb.out 2>/dev/null && break; sleep 0.25; done
  local to; to=$(sed -n 's/^RELAY \([0-9.:]*\) .*/\1/p' $W/rb.out)
  if [ -z "$to" ]; then kill $pb 2>/dev/null; echo "FAIL (phone B: $(head -n 1 $W/rb.out | cut -c1-70))"; return; fi
  ex phA $PROBE --transport $3 --server 203.0.113.10:$4 --user "$U" --pass "$P" --peer-ip 203.0.113.10 $extra send --to $to > $W/ra.out 2>&1
  local rc=$?; wait $pb 2>/dev/null
  if [ $rc = 0 ]; then echo "ok  ($(grep '^OK' $W/ra.out | sed 's/OK pong from [0-9.:]* in //'), $(grep -o 'allocated in [^,]*' $W/ra.out | head -n 1))"
  else echo "FAIL (phone A: $(grep -m1 '^FAIL' $W/ra.out | cut -c1-70))"; fi; }

pass=0; fail=0
scen() { # label expected command... (LAB_ONLY=word runs only scenarios whose label contains it; LAB_DEBUG=1 shows the logs)
  local label=$1 want=$2; shift 2
  [ -n "${LAB_ONLY:-}" ] && case "$label" in *"$LAB_ONLY"*) ;; *) return;; esac
  local got ok=FAIL; got=$("$@")
  case "$got" in $want*) ok=pass;; esac
  [ $ok = pass ] && pass=$((pass+1)) || fail=$((fail+1))
  printf '%-62s %-6s %s\n' "$label" "[$ok]" "$got"
  if [ -n "${LAB_DEBUG:-}" ]; then
    for f in pa pb ra rb; do
      if [ -s $W/$f.out ]; then echo "  --- $f:"; sed 's/^/  | /' $W/$f.out | head -n 12; fi
    done
  fi
}
printf '%-62s %-6s %s\n' "SCENARIO" "" "RESULT"
scen "two home routers (cone NAT): direct UDP hole punch" yes direct cone cone
scen "two symmetric NATs: direct UDP hole punch (cannot work)" no direct symmetric symmetric
scen "two home routers: TURN over UDP via the forwarded server" ok relay cone cone udp 3478
scen "two symmetric NATs: TURN over UDP" ok relay symmetric symmetric udp 3478
scen "office blocks UDP on both sides: TURN over UDP (blocked)" FAIL relay udp-blocked udp-blocked udp 3478
scen "office blocks UDP on both sides: TURN over TCP 3478" ok relay udp-blocked udp-blocked tcp 3478
scen "only TCP 443 gets out: TURN over TCP 3478 (blocked)" FAIL relay tls-only tls-only tcp 3478
scen "only TCP 443 gets out: TURN over TLS 443" ok relay tls-only tls-only tls 443
scen "mixed: one phone UDP-blocked, one behind a symmetric NAT (TCP)" ok relay udp-blocked symmetric tcp 3478
echo; echo "$pass as expected, $fail not"
[ $fail = 0 ]
