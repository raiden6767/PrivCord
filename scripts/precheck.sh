#!/usr/bin/env bash
# The fast checks CI runs, run locally in a minute or two, so a push does not burn CI minutes on something a laptop would
# have caught: formatting, the migration map, types, unit tests, the Rust lints. Kotlin only compiles in CI (it needs the
# Android SDK), so those changes are checked there; this cannot see them.
#   scripts/precheck.sh          everything
#   scripts/precheck.sh --quick  skip the Rust lints (the slow part)
set -u
cd "$(dirname "$0")/.."
fail=0
step() { printf '\n== %s\n' "$1"; shift; if "$@"; then echo "ok"; else echo "FAILED: $*"; fail=1; fi; }

step "cargo fmt" cargo fmt --all --check
migration_map() {
  cp docs/rust/MIGRATION_MAP.md /tmp/map.before
  python3 docs/rust/tools/gen_migration_map.py > /tmp/map.out 2>&1
  if grep -q UNMAPPED /tmp/map.out; then grep -A5 UNMAPPED /tmp/map.out; echo "add a rule for these in docs/rust/tools/gen_migration_map.py"; return 1; fi
  cmp -s /tmp/map.before docs/rust/MIGRATION_MAP.md || echo "the map was out of date: regenerated, remember to commit docs/rust/MIGRATION_MAP.md"
}
step "migration map is complete" migration_map
step "TypeScript types" bash -c 'cd apps/desktop && npx tsc --noEmit'
step "desktop unit tests" bash -c 'cd apps/desktop && npx vitest run 2>&1 | tail -6'
step "Rust tests that need no network (domain, crypto)" cargo test -q -p privcord-domain -p privcord-crypto
if [ "${1:-}" != "--quick" ]; then
  step "cargo clippy (warnings are errors)" cargo clippy --workspace --all-targets --locked -- -D warnings
fi
echo
[ $fail -eq 0 ] && echo "all checks passed" || { echo "some checks failed"; exit 1; }
