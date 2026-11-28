#!/usr/bin/env bash
# build-and-show.sh — the one command that goes from source to a visible app.
#
# Run on onyx (the canonical checkout), from ~/flectar-build:
#   scripts/build-and-show.sh
#
# Steps:
#   1. cargo build --release -j 16 in this checkout, reported through g2job so
#      the G2 hub ETA board and the watch see it (plain cargo if g2job is absent)
#   2. stream scripts/deploy-tron.sh to tron and run it there: it fetches the new
#      binary, verifies the sha256, replaces it atomically and restarts the app
#   3. relay tron's pid and startup-phase timings (first_frame is the one that
#      proves the window is visible)
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
JOBS="${ONYX_JOBS:-16}"
# onyx has no mDNS name for tron and its ssh alias "tron" points at the
# home-server, so default to tron's LAN address. Override with TRON_HOST.
TRON_HOST="${TRON_HOST:-tron@192.168.50.15}"
CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
BUILD_LOG="${ONYX_BUILD_LOG:-/tmp/yours-build-show.log}"

if [ ! -f "$REPO_ROOT/Cargo.toml" ]; then
  echo "build-and-show: $REPO_ROOT is not the Yours checkout" >&2
  exit 2
fi
if [ ! -d "$HOME/flectar-build/.git" ] && [ "$REPO_ROOT" != "$HOME/flectar-build" ]; then
  echo "build-and-show: run this on onyx (~/flectar-build); tron only displays the app" >&2
  exit 2
fi

cd "$REPO_ROOT"

echo "==> building release on $(hostname -s) (-j $JOBS, log $BUILD_LOG)"
build_command="$CARGO build --release -j $JOBS 2>&1 | tee '$BUILD_LOG'"
if command -v g2job >/dev/null 2>&1; then
  g2job run "yours build+show" -- bash -o pipefail -c "$build_command"
else
  bash -o pipefail -c "$build_command"
fi

bin="$REPO_ROOT/target/release/yours"
[ -x "$bin" ] || { echo "build-and-show: the build produced no $bin" >&2; exit 1; }
echo "==> built $bin ($(stat -c %s "$bin") bytes, sha256 $(sha256sum "$bin" | cut -d' ' -f1))"

echo "==> deploying to $TRON_HOST"
ssh -o BatchMode=yes "$TRON_HOST" \
  'mkdir -p /tmp/yours-deploy && cat > /tmp/yours-deploy/deploy-tron.sh && chmod 755 /tmp/yours-deploy/deploy-tron.sh && exec /tmp/yours-deploy/deploy-tron.sh' \
  < "$REPO_ROOT/scripts/deploy-tron.sh"
