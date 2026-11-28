#!/usr/bin/env bash
# build-and-show.sh — build Yours on onyx and run it on onyx.
#
# onyx is the development, build, test and display host. The default path is
# onyx-only: cargo build --release in ~/flectar-build, then scripts/show-onyx.sh
# launches the binary in onyx's Plasma session (pid + startup phases, optional
# screenshot).
#
#   scripts/build-and-show.sh                   # build + run on onyx
#   scripts/build-and-show.sh --screenshot      # build + run + screenshot
#   scripts/build-and-show.sh --no-restart      # build, run only if not running
#   scripts/build-and-show.sh --help
#
# tron is the owner's working machine and off-limits to agents: the default
# path never touches it. --tron is a deprecated, deliberate deploy that needs
# the owner's explicit request; agents must not pass it on their own.
#
# Running it from tron or a laptop forwards the whole step to onyx over ssh, so
# no cargo command ever runs on tron.
#
# Steps (default):
#   1. cargo build --release -j 16 on onyx, reported through g2job when present
#   2. scripts/show-onyx.sh: restart Yours on onyx, print pid + startup phases
#   3. optional full-screen screenshot with spectacle (--screenshot)
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
HOST="${ONYX_HOST:-onyx}"
REMOTE_DIR="${ONYX_DIR:-flectar-build}"
JOBS="${ONYX_JOBS:-16}"
CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
TRON_HOST="${TRON_HOST:-tron@192.168.50.15}"
BUILD_LOG="${ONYX_BUILD_LOG:-/tmp/yours-build-show.log}"

die() { printf 'build-and-show: %s\n' "$*" >&2; exit 2; }

usage() {
  cat <<'USAGE'
usage: scripts/build-and-show.sh [--screenshot] [--no-restart] [--tron]

Build Yours on onyx and run it on onyx (the default, tron is not touched).

  --screenshot   also save a full-screen screenshot on onyx
  --no-restart   start the app only if nothing is running
  --tron         DEPRECATED deliberate deploy to tron; only for a tron deploy
                 the owner explicitly asked for. Never pass it on your own.

Environment: ONYX_HOST (onyx), ONYX_DIR (flectar-build), ONYX_JOBS (16),
ONYX_BUILD_LOG (/tmp/yours-build-show.log), TRON_HOST (tron@192.168.50.15).
USAGE
}

ORIG_ARGS=("$@")
DEPLOY_TRON=0
SHOW_ARGS=()

while [ "$#" -gt 0 ]; do
  case "$1" in
    --tron) DEPLOY_TRON=1 ;;
    --screenshot) SHOW_ARGS+=(--screenshot) ;;
    --screenshot=*) SHOW_ARGS+=("$1") ;;
    --no-restart) SHOW_ARGS+=(--no-restart) ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
  shift
done

# Run on onyx: forward the whole build there rather than building locally.
if [ "$(hostname -s 2>/dev/null || hostname)" != "onyx" ]; then
  if [ "${ONYX_BUILD_SHOW_REMOTE:-}" = "1" ]; then
    die "forwarded but $(hostname -s 2>/dev/null || hostname) is not onyx; refusing to guess"
  fi
  echo "==> forwarding to $HOST:$REMOTE_DIR (nothing builds or runs on $(hostname -s 2>/dev/null || hostname); tron is not touched)"
  exec ssh -o BatchMode=yes "$HOST" \
    "cd ~/$REMOTE_DIR && ONYX_BUILD_SHOW_REMOTE=1 exec scripts/build-and-show.sh $(printf '%q ' "${ORIG_ARGS[@]}")"
fi

[ -f "$REPO_ROOT/Cargo.toml" ] || die "$REPO_ROOT is not the Yours checkout"
cd "$REPO_ROOT"
if [ "$REPO_ROOT" != "$HOME/$REMOTE_DIR" ]; then
  echo "build-and-show: warning: building $REPO_ROOT instead of the canonical $HOME/$REMOTE_DIR" >&2
fi

if [ "$DEPLOY_TRON" -eq 1 ] && [ "${#SHOW_ARGS[@]}" -gt 0 ]; then
  echo "build-and-show: --tron ignores --screenshot/--no-restart (those apply to onyx)" >&2
fi

echo "==> building release on $(hostname -s) (-j $JOBS, log $BUILD_LOG)"
build_command="$CARGO build --release -j $JOBS 2>&1 | tee '$BUILD_LOG'"
if command -v g2job >/dev/null 2>&1; then
  g2job run "yours build+show" -- bash -o pipefail -c "$build_command"
else
  bash -o pipefail -c "$build_command"
fi

bin="$REPO_ROOT/target/release/yours"
[ -x "$bin" ] || die "the build produced no $bin"
echo "==> built $bin ($(stat -c %s "$bin") bytes, sha256 $(sha256sum "$bin" | cut -d' ' -f1))"

if [ "$DEPLOY_TRON" -eq 0 ]; then
  echo "==> running on onyx (tron is not touched)"
  exec "$REPO_ROOT/scripts/show-onyx.sh" "${SHOW_ARGS[@]}"
fi

# --- deprecated, deliberate tron deploy ------------------------------------
echo
echo "!! --tron: deploying the new binary to tron."
echo "!! tron is the owner's working machine and is off-limits to agents."
echo "!! Only do this on the owner's explicit request; scripts/deploy-tron.sh"
echo "!! itself is deprecated and prints the same warning."
echo
echo "==> deploying to $TRON_HOST"
ssh -o BatchMode=yes "$TRON_HOST" \
  'mkdir -p /tmp/yours-deploy && cat > /tmp/yours-deploy/deploy-tron.sh && chmod 755 /tmp/yours-deploy/deploy-tron.sh && exec /tmp/yours-deploy/deploy-tron.sh' \
  < "$REPO_ROOT/scripts/deploy-tron.sh"
