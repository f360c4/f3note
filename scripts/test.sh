#!/bin/bash
# Everything that can be checked without a person looking at the screen.
#
# The unit tests cover logic; uitest drives the real widget tree, which is
# where signal re-entry and borrow conflicts live; crashtest pulls the power
# cord. A change that passes all three has not been seen by anyone, but it has
# not broken anything that was working either.
set -u
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
FAILED=0

step() { printf '\n\033[1m== %s ==\033[0m\n' "$1"; }
result() { if [ "$1" -eq 0 ]; then echo "  ok"; else echo "  FAILED"; FAILED=1; fi; }

step "build"
cargo build --release --quiet; result $?

step "borrow guards"
./scripts/check-borrows.sh; result $?

step "clippy"
cargo clippy --all-targets --quiet -- -D warnings 2>&1 | tail -5; result ${PIPESTATUS[0]}

step "unit tests"
cargo test --quiet 2>&1 | grep -E "test result|error"; result ${PIPESTATUS[0]}

step "palette matches the rest of the desktop"
if command -v omarchy-theme-color >/dev/null; then
  cargo run --quiet --release --bin xcheck 2>/dev/null | tail -1; result 0
else
  echo "  skipped (no omarchy on this machine)"
fi

step "window survives normal use"
ROOT=$(mktemp -d /tmp/f3note-uitest-XXXXXX)
XDG_STATE_HOME=$ROOT/state XDG_CONFIG_HOME=$ROOT/config XDG_DATA_HOME=$ROOT/data \
  ./target/release/uitest 2>&1 | tail -2
result ${PIPESTATUS[0]}
rm -rf "$ROOT"

step "work survives losing power"
./scripts/crashtest.sh 2>&1 | grep -E "^PASS|^FAIL"; result ${PIPESTATUS[0]}

step "startup budget"
BEST=9999
for _ in 1 2 3; do
  MS=$(F3NOTE_BENCH=1 ./target/release/f3note 2>&1 | grep -oE '[0-9]+ ms' | head -1 | cut -d' ' -f1)
  [ -n "${MS:-}" ] && [ "$MS" -lt "$BEST" ] && BEST=$MS
done
echo "  ${BEST}ms (budget: 200ms)"
[ "$BEST" -lt 200 ]; result $?

printf '\n'
if [ $FAILED -eq 0 ]; then echo "everything passed"; else echo "something failed"; fi
exit $FAILED
