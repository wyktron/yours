#!/usr/bin/env bash
# test-onyx.sh — run cargo test for Yours on onyx, with the environment tests need.
#
# onyx is the testing host. Run every cargo test there, never on tron.
#
#   scripts/test-onyx.sh                             # cargo test --workspace
#   scripts/test-onyx.sh -p flectar-mail-core         # one package
#   scripts/test-onyx.sh my_test_name                 # workspace-wide filter
#   scripts/test-onyx.sh -- --nocapture               # extra args after --
#   scripts/test-onyx.sh -p flectar-mail-core --lib --locked
#
# From tron or a laptop the script forwards itself to onyx over ssh; on onyx it
# runs locally in ~/flectar-build (ONYX_DIR overrides). It sets:
#
#   RUST_MIN_STACK=67108864  Slint's generated test harness needs a big stack
#   RUST_BACKTRACE=1         useful stacks in failure output
#   -j 16                    build jobs (ONYX_JOBS overrides; onyx has 32 cores)
#
# and reports the run through g2job when it is available, so long suites show
# up on the G2 hub.
set -euo pipefail

HOST="${ONYX_HOST:-onyx}"
REMOTE_DIR="${ONYX_DIR:-flectar-build}"
JOBS="${ONYX_JOBS:-16}"
CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
export RUST_MIN_STACK="${RUST_MIN_STACK:-67108864}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

die() { printf 'test-onyx: %s\n' "$*" >&2; exit 2; }

# Default to the full workspace suite when no cargo arguments are given.
if [ "$#" -eq 0 ]; then
  set -- --workspace
fi

# Run on onyx: forward the exact argument vector rather than testing locally.
if [ "$(hostname -s 2>/dev/null || hostname)" != "onyx" ]; then
  if [ "${ONYX_TEST_REMOTE:-}" = "1" ]; then
    die "forwarded but $(hostname -s 2>/dev/null || hostname) is not onyx; refusing to guess"
  fi
  echo "==> running tests on $HOST:$REMOTE_DIR, not on $(hostname -s 2>/dev/null || hostname)"
  exec ssh -o BatchMode=yes "$HOST" \
    "cd ~/$REMOTE_DIR && ONYX_TEST_REMOTE=1 exec scripts/test-onyx.sh $(printf '%q ' "$@")"
fi

[ -d "$HOME/$REMOTE_DIR/.git" ] || die "no checkout at $HOME/$REMOTE_DIR"
cd "$HOME/$REMOTE_DIR"

echo "==> cargo test -j $JOBS on $(hostname -s) (RUST_MIN_STACK=$RUST_MIN_STACK, RUST_BACKTRACE=$RUST_BACKTRACE)"
if command -v g2job >/dev/null 2>&1; then
  g2job run "yours tests" -- "$CARGO" test -j "$JOBS" "$@"
else
  exec "$CARGO" test -j "$JOBS" "$@"
fi
