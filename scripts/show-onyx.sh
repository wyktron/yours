#!/usr/bin/env bash
# show-onyx.sh — launch (or restart) Yours on onyx's own Plasma session.
#
# onyx is the development, test and display host. tron is the owner's working
# machine and this script never touches it: it runs no command on tron, reads
# and writes no file there, and never restarts or screenshots tron's app.
#
# Run on onyx, from the canonical checkout ~/flectar-build:
#
#   scripts/show-onyx.sh                      # restart Yours, wait for first_frame
#   scripts/show-onyx.sh --screenshot         # ... and save /tmp/yours-onyx-<ts>.png
#   scripts/show-onyx.sh --screenshot /tmp/yours.png
#   scripts/show-onyx.sh --no-restart         # start only if nothing is running
#   scripts/show-onyx.sh --bin target/release/yours
#   scripts/show-onyx.sh --timeout 180
#
# From tron or a laptop the same command forwards itself to onyx over ssh; the
# launch, pgrep/kill and screenshot all happen on onyx. Nothing runs on tron.
#
# What it does:
#   1. resolves the checkout and target/release/yours
#   2. stops a running Yours owned by onyx's uid (SIGTERM, then SIGKILL)
#   3. starts it detached in onyx's Plasma session environment, logging to
#      /tmp/yours-onyx.log (YOURS_ONYX_LOG overrides)
#   4. prints the pid and the startup-phase lines, waiting for first_frame
#   5. optionally takes a full-screen screenshot with spectacle
#
# Plasma session environment used (onyx, uid 1000):
#   WAYLAND_DISPLAY=wayland-1
#   XDG_RUNTIME_DIR=/run/user/1000
#   DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus
#   DISPLAY=:0
#   XAUTHORITY=/run/user/1000/xauth_<random>   (discovered from kwin_wayland)
set -euo pipefail

HOST="${ONYX_HOST:-onyx}"
REMOTE_DIR="${ONYX_DIR:-flectar-build}"
REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

die() { printf 'show-onyx: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'USAGE'
usage: scripts/show-onyx.sh [--screenshot[=PATH]] [--no-restart] [--bin PATH] [--timeout SECONDS]

Launch (or restart) Yours on onyx's Plasma session, print the pid and the
startup-phase timings, and never touch tron.

  --screenshot[=PATH]  also save a full-screen screenshot (default
                       /tmp/yours-onyx-<timestamp>.png); a separate PATH argument
                       is accepted too
  --no-restart         start only if nothing is running
  --bin PATH           use this binary instead of target/release/yours
  --timeout SECONDS    how long to wait for first_frame (default 120)

Environment: ONYX_HOST (onyx), ONYX_DIR (flectar-build), YOURS_ONYX_LOG
(/tmp/yours-onyx.log), FIRST_FRAME_TIMEOUT (120).

From tron or a laptop the script forwards itself to onyx over ssh; the launch,
pgrep/kill and screenshot all happen on onyx.
USAGE
}

# ---------------------------------------------------------------------------
# Everything below acts on onyx's Plasma session. Anywhere else (tron, a
# laptop) re-run the onyx copy of this script there instead, so no process,
# pgrep, kill or screenshot ever happens on the local machine.
# ---------------------------------------------------------------------------
if [ "$(hostname -s 2>/dev/null || hostname)" != "onyx" ]; then
  if [ "${ONYX_SHOW_REMOTE:-}" = "1" ]; then
    die "forwarded but $(hostname -s 2>/dev/null || hostname) is not onyx; refusing to guess"
  fi
  echo "==> forwarding to $HOST:$REMOTE_DIR/scripts/show-onyx.sh (nothing runs on $(hostname -s 2>/dev/null || hostname))"
  exec ssh -o BatchMode=yes "$HOST" \
    "cd ~/$REMOTE_DIR && ONYX_SHOW_REMOTE=1 exec scripts/show-onyx.sh $(printf '%q ' "$@")"
fi

RESTART=1
SHOT=""
BIN=""
TIMEOUT="${FIRST_FRAME_TIMEOUT:-120}"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --screenshot)
      if [ "$#" -ge 2 ] && [ "${2#-}" = "$2" ]; then
        SHOT="$2"
        shift
      else
        SHOT="${YOURS_ONYX_SHOT:-/tmp/yours-onyx-$(date +%Y%m%d-%H%M%S).png}"
      fi
      ;;
    --screenshot=*) SHOT="${1#*=}" ;;
    --no-restart) RESTART=0 ;;
    --bin)
      [ "$#" -ge 2 ] || die "--bin needs a path"
      BIN="$2"
      shift
      ;;
    --bin=*) BIN="${1#*=}" ;;
    --timeout)
      [ "$#" -ge 2 ] || die "--timeout needs seconds"
      TIMEOUT="$2"
      shift
      ;;
    --timeout=*) TIMEOUT="${1#*=}" ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
  shift
done

[ "$(id -u)" -eq 1000 ] || die "run as onyx (uid 1000), not uid $(id -u)"

# onyx's Plasma session. A plain ssh login does not inherit these, so set them
# here; overrides from the caller win.
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/1000}"
export WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-1}"
export DISPLAY="${DISPLAY:-:0}"
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=/run/user/1000/bus}"

# onyx has no ~/.Xauthority: KWin writes /run/user/1000/xauth_<random> for the
# session's XWayland and hands it to XWayland on --xwayland-xauthority. Discover
# the live value instead of hard-coding the random suffix.
if [ -z "${XAUTHORITY:-}" ]; then
  XAUTHORITY="$(ps -o args= -C kwin_wayland 2>/dev/null \
    | sed -n 's/.*--xwayland-xauthority \([^ ]*\).*/\1/p' | head -1 || true)"
  if [ -z "$XAUTHORITY" ]; then
    for xauth_candidate in "$XDG_RUNTIME_DIR"/xauth_*; do
      if [ -f "$xauth_candidate" ]; then
        XAUTHORITY="$xauth_candidate"
        break
      fi
    done
  fi
fi
if [ -n "${XAUTHORITY:-}" ]; then export XAUTHORITY; fi

[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ] \
  || die "no Wayland socket $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY: is onyx's Plasma session running?"
xdisplay="${DISPLAY#:}"
xdisplay="${xdisplay%%.*}"
if [ ! -S "/tmp/.X11-unix/X$xdisplay" ]; then
  echo "show-onyx: warning: no X11 socket for DISPLAY=$DISPLAY; the Wayland window still works" >&2
fi

take_screenshot() {
  local out="$1"
  command -v spectacle >/dev/null 2>&1 || die "spectacle is not installed; cannot take a screenshot"
  mkdir -p -- "$(dirname -- "$out")"
  rm -f -- "$out"
  echo "==> screenshot (spectacle -f, full screen) -> $out"
  if ! timeout 120 spectacle -f -b -n -d 500 -o "$out" >/tmp/yours-onyx-spectacle.log 2>&1; then
    tail -20 /tmp/yours-onyx-spectacle.log >&2 || true
    die "spectacle failed (see /tmp/yours-onyx-spectacle.log)"
  fi
  [ -s "$out" ] || die "spectacle exited 0 but wrote no $out"
  echo "==> screenshot saved: $out ($(stat -c %s -- "$out") bytes, $(file -b -- "$out"))"
}

BIN="${BIN:-$REPO_ROOT/target/release/yours}"
[ -x "$BIN" ] || die "no executable at $BIN: build first with scripts/build-and-show.sh"
APP="$(basename -- "$BIN")"

running="$(pgrep -u "$(id -u)" -x "$APP" || true)"
if [ -n "$running" ]; then
  if [ "$RESTART" -eq 0 ]; then
    echo "==> $APP already running on $(hostname -s) (pid $(printf '%s ' $running)); --no-restart leaves it alone"
    if [ -n "$SHOT" ]; then take_screenshot "$SHOT"; fi
    exit 0
  fi
  echo "==> stopping running $APP (pid $(printf '%s ' $running))"
  kill $running 2>/dev/null || true
  for _ in $(seq 1 50); do
    pgrep -u "$(id -u)" -x "$APP" >/dev/null 2>&1 || break
    sleep 0.2
  done
  running="$(pgrep -u "$(id -u)" -x "$APP" || true)"
  if [ -n "$running" ]; then
    echo "==> still running after SIGTERM, sending SIGKILL (pid $(printf '%s ' $running))"
    kill -9 $running 2>/dev/null || true
    for _ in $(seq 1 30); do
      pgrep -u "$(id -u)" -x "$APP" >/dev/null 2>&1 || break
      sleep 0.1
    done
    running="$(pgrep -u "$(id -u)" -x "$APP" || true)"
    [ -z "$running" ] || die "could not stop pid $running"
  fi
fi

LOG="${YOURS_ONYX_LOG:-/tmp/yours-onyx.log}"
: > "$LOG"
echo "==> starting $BIN on $(hostname -s) (log $LOG)"
(
  cd "$REPO_ROOT"
  setsid nohup "$BIN" >>"$LOG" 2>&1 </dev/null &
)
sleep 1
pid="$(pgrep -u "$(id -u)" -x "$APP" | head -1 || true)"
if [ -z "$pid" ]; then
  echo "--- last 20 log lines ---" >&2
  tail -20 "$LOG" >&2 || true
  die "$APP exited during startup"
fi
echo "==> pid $pid"

deadline=$(( $(date +%s) + TIMEOUT ))
while ! grep -aq 'startup-phase: first_frame' "$LOG" 2>/dev/null; do
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "--- last 20 log lines ---" >&2
    tail -20 "$LOG" >&2 || true
    die "$APP exited before first_frame"
  fi
  if [ "$(date +%s)" -ge "$deadline" ]; then
    echo "--- last 20 log lines ---" >&2
    tail -20 "$LOG" >&2 || true
    die "no first_frame in $LOG within ${TIMEOUT}s"
  fi
  sleep 0.5
done

echo "==> startup phases:"
grep -a 'startup-phase:' "$LOG" | sed 's/^/    /'

if [ -n "$SHOT" ]; then take_screenshot "$SHOT"; fi
echo "==> Yours is running on onyx: pid $pid, log $LOG"
