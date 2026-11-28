#!/usr/bin/env bash
# build-onyx.sh — build the flectar-mail release binary on onyx and fetch it back.
#
#   scripts/build-onyx.sh
#
# Steps:
#   1. rsync this checkout to onyx:~/flectar-build, excluding target/, .git/
#      and local mail databases (no --delete: stale remote files are harmless,
#      the remote target/ is what keeps rebuilds incremental).
#   2. run "cargo build --release -j 16" there, logging to /tmp/onyx-build.log.
#   3. on success copy target/release/yours back to this checkout with mode 755.
#
# Exits non-zero if any step fails and prints the elapsed wall time either way.
# Overrides: ONYX_HOST (onyx), ONYX_BUILD_DIR (flectar-build), ONYX_JOBS (16).
set -euo pipefail

# RETIRED (migration 2026-11-28): onyx is the canonical checkout and build host
# now, so rsyncing a tron working tree over ~/flectar-build would overwrite the
# source of truth with an older tree. Use scripts/build-and-show.sh on onyx, or
# scripts/dev-onyx.sh <cargo cmd>. The implementation below stays behind the
# override only for an emergency rebuild from a tree known to match onyx.
if [ "${ALLOW_LEGACY_BUILD_ONYX:-}" != "1" ]; then
  cat >&2 <<'MSG'
build-onyx.sh is retired: onyx is now the canonical checkout.
  on onyx:   scripts/build-and-show.sh
  any cargo: scripts/dev-onyx.sh <cargo subcommand...>
Set ALLOW_LEGACY_BUILD_ONYX=1 only for an emergency rebuild from a tron tree
that is known to match onyx.
MSG
  exit 2
fi

HOST="${ONYX_HOST:-onyx}"
REMOTE_DIR="${ONYX_BUILD_DIR:-flectar-build}"
JOBS="${ONYX_JOBS:-16}"
REMOTE_LOG="/tmp/onyx-build.log"

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
LOCAL_BIN="$REPO_ROOT/target/release/yours"

START=$(date +%s)
TMP_BIN=""

finish() {
  local status=$?
  local elapsed=$(( $(date +%s) - START ))
  if [[ -n "$TMP_BIN" ]]; then rm -f -- "$TMP_BIN"; fi
  printf 'Elapsed: %dm %02ds (%ds)\n' $((elapsed / 60)) $((elapsed % 60)) "$elapsed"
  if (( status != 0 )); then
    printf 'build-onyx: FAILED (exit %d)\n' "$status" >&2
  fi
  exit "$status"
}
trap finish EXIT

echo "==> syncing $REPO_ROOT to $HOST:$REMOTE_DIR"
ssh -o BatchMode=yes "$HOST" "mkdir -p -- \"\$HOME/$REMOTE_DIR\""
rsync -a --info=stats1 \
  --exclude 'target/' \
  --exclude '.git/' \
  --exclude 'flectar-mail.db*' \
  --exclude '*.db-wal' \
  --exclude '*.db-shm' \
  --exclude '*.db-journal' \
  "$REPO_ROOT/" "$HOST:$REMOTE_DIR/"

# crates/flectar-mail-core/build.rs declares ../../.env as rerun-if-changed.
# While that file is absent Cargo marks the core crate dirty on every build and
# recompiles it (~15 min), even when nothing changed. An empty .env parses
# exactly like a missing one (no OAuth ids), so keep a stable placeholder. A
# real .env is synced by the rsync above; when the checkout has none, empty a
# stale remote one, but never touch an already-empty file (that would bump its
# mtime and defeat the point).
if [[ ! -f "$REPO_ROOT/.env" ]]; then
  ssh -o BatchMode=yes "$HOST" \
    "f=~/$REMOTE_DIR/.env; if [ ! -e \"\$f\" ] || [ -s \"\$f\" ]; then : > \"\$f\"; fi"
fi

echo "==> building on $HOST (release, -j $JOBS, log: $REMOTE_LOG)"
if ! ssh -o BatchMode=yes "$HOST" \
    "set -o pipefail; cd ~/$REMOTE_DIR && ~/.cargo/bin/cargo build --release -j $JOBS 2>&1 | tee $REMOTE_LOG"; then
  echo "--- last 40 lines of $HOST:$REMOTE_LOG ---" >&2
  ssh -o BatchMode=yes "$HOST" "tail -40 $REMOTE_LOG" >&2 || true
  exit 1
fi

echo "==> fetching target/release/yours"
ssh -o BatchMode=yes "$HOST" "test -x ~/$REMOTE_DIR/target/release/yours"

mkdir -p -- "$(dirname -- "$LOCAL_BIN")"
TMP_BIN="$(mktemp "$LOCAL_BIN.XXXXXX")"
rsync -a "$HOST:$REMOTE_DIR/target/release/yours" "$TMP_BIN"
chmod 755 "$TMP_BIN"

if ! file -b "$TMP_BIN" | grep -q 'ELF 64-bit.*x86-64'; then
  echo "build-onyx: fetched binary is not an x86-64 ELF:" >&2
  file -b "$TMP_BIN" >&2
  exit 1
fi

mv -f -- "$TMP_BIN" "$LOCAL_BIN"
TMP_BIN=""
echo "==> installed $LOCAL_BIN ($(stat -c '%s bytes, mode %a' "$LOCAL_BIN"))"
