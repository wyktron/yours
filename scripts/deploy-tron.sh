#!/usr/bin/env bash
# deploy-tron.sh — put the release built on onyx onto tron's screen.
#
# Run on tron (the display node):
#   scripts/deploy-tron.sh
# build-and-show.sh streams this script to tron and runs it from /tmp, so the
# checkout path is taken from $HOME, not from the script location.
#
# What it does:
#   1. fetch onyx:~/flectar-build/target/release/yours into tron's checkout
#   2. verify the local sha256 equals the sha256 on onyx
#   3. atomically rename it over target/release/yours (same-directory temp)
#   4. restart the app with the Wayland/X11 session environment
#   5. print the new pid and the startup-phase timings from the new log
#
# It never touches the app lock configuration: the config JSON, the instance
# lock, the Tuta webview lock and the mail/calendar DBs under
# ~/.local/share/yours-mail all stay exactly as they are. Only one binary is
# replaced and the process is restarted.
set -euo pipefail

HOST="${ONYX_HOST:-onyx}"
REMOTE_DIR="${ONYX_DIR:-flectar-build}"
TRON_REPO="${TRON_REPO:-$HOME/Code/flectar-mail}"
REMOTE_BIN="~/$REMOTE_DIR/target/release/yours"
LOCAL_BIN="$TRON_REPO/target/release/yours"
LOG="${YOURS_LOG:-/tmp/yours-deploy.log}"
FIRST_FRAME_TIMEOUT="${FIRST_FRAME_TIMEOUT:-120}"

die() { printf 'deploy-tron: %s\n' "$*" >&2; exit 1; }

[ -d "$TRON_REPO/.git" ] || die "no tron checkout at $TRON_REPO (set TRON_REPO)"
mkdir -p -- "$TRON_REPO/target/release"
trap 'rm -f -- "${tmp:-}"' EXIT

echo "==> fetching $HOST:$REMOTE_BIN"
ssh -o BatchMode=yes "$HOST" "test -x $REMOTE_BIN" \
  || die "$HOST has no built binary at $REMOTE_BIN (run scripts/build-and-show.sh on onyx)"

tmp="$(mktemp "$TRON_REPO/target/release/.yours.deploy.XXXXXX")"
rsync -a --no-owner --no-group "$HOST:$REMOTE_BIN" "$tmp"
chmod 755 -- "$tmp"

remote_sha="$(ssh -o BatchMode=yes "$HOST" "sha256sum $REMOTE_BIN | cut -d' ' -f1")"
local_sha="$(sha256sum "$tmp" | cut -d' ' -f1)"
[ -n "$remote_sha" ] || die "could not read the sha256 on $HOST"
[ "$remote_sha" = "$local_sha" ] || die "sha256 mismatch: $HOST=$remote_sha local=$local_sha"
echo "==> sha256 $local_sha OK"

mv -f -- "$tmp" "$LOCAL_BIN"
trap - EXIT
echo "==> installed $LOCAL_BIN"

echo "==> restarting the app on tron"
pids="$(pgrep -x yours || true)"
if [ -n "$pids" ]; then
  kill $pids 2>/dev/null || true
  for _ in $(seq 1 50); do pgrep -x yours >/dev/null 2>&1 || break; sleep 0.2; done
  pids="$(pgrep -x yours || true)"
  [ -z "$pids" ] || kill -9 $pids 2>/dev/null || true
fi

: > "$LOG"
(
  cd "$TRON_REPO"
  env \
    DISPLAY=:0 \
    WAYLAND_DISPLAY=wayland-1 \
    XDG_RUNTIME_DIR=/run/user/1000 \
    DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus \
    setsid nohup "$LOCAL_BIN" >>"$LOG" 2>&1 </dev/null &
)
sleep 1
pid="$(pgrep -x yours | head -1 || true)"
if [ -z "$pid" ]; then
  echo "--- last 20 log lines ---" >&2
  tail -20 "$LOG" >&2 || true
  die "the app did not start"
fi
echo "==> pid $pid"

deadline=$(( $(date +%s) + FIRST_FRAME_TIMEOUT ))
while ! grep -aq 'startup-phase: first_frame' "$LOG"; do
  if [ "$(date +%s)" -ge "$deadline" ]; then
    echo "--- last 20 log lines ---" >&2
    tail -20 "$LOG" >&2 || true
    die "no first_frame in $LOG within ${FIRST_FRAME_TIMEOUT}s"
  fi
  sleep 0.5
done

echo "==> startup phases"
grep -a 'startup-phase:' "$LOG" | sed 's/^/    /'
echo "==> app running: pid $pid, log $LOG"
