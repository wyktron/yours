# Development guide

Yours is the Rust/Slint desktop mail app at <https://github.com/wyktron/yours> (branch `main`).

**All development happens on onyx**: editing, building, testing, running and screenshots.
**tron is the owner's working machine and is off-limits to agents.** Nothing in this guide
starts, restarts, screenshots or deploys anything on tron, and the instance the owner is running
there is left exactly as it is.

## Hosts and responsibilities

| Host | Role | Notes |
|------|------|-------|
| **onyx** | Edit / build / test / run / screenshot host (canonical) | `/home/onyx/flectar-build`; Fedora 45, 32 cores, 125 GiB RAM, ~1.7 TB free, warm `target/`. |
| **tron** | The owner's working machine | **Off-limits for agents.** Do not open, restart, screenshot or deploy anything there. |

The canonical checkout is on onyx. Never edit, build, test, run, screenshot or push from tron.

## Where the code lives

Canonical checkout on onyx:

```bash
/home/onyx/flectar-build      # git clone of origin/main, warm target/
```

The helper scripts live in the repo under `scripts/`:

| Script | What it does |
|--------|--------------|
| `scripts/build-and-show.sh` | Build on onyx and run on onyx (the default). `--tron` is a deprecated, deliberate tron deploy. |
| `scripts/show-onyx.sh` | Launch/restart Yours in onyx's Plasma session; print pid + startup phases; optional screenshot. |
| `scripts/test-onyx.sh` | `cargo test` on onyx with the right environment. |
| `scripts/dev-onyx.sh` | Any other cargo command on onyx. |
| `scripts/deploy-tron.sh` | **Deprecated.** Only for a tron deploy the owner explicitly asked for. |

Every onyx script forwards itself over ssh when run from tron or a laptop and runs locally when
run on onyx, so the command is the same from either place. Source only moves through git: commit
and push on onyx, `git pull --ff-only` on onyx to pick up changes. The old
`scripts/build-onyx.sh`, which rsynced a tron tree onto onyx, is retired and refuses to run
unless `ALLOW_LEGACY_BUILD_ONYX=1` is set.

## Getting onto onyx

```bash
ssh onyx
cd ~/flectar-build
git pull --ff-only
```

## Everyday workflow (all on onyx)

```bash
ssh onyx
cd ~/flectar-build
git pull --ff-only
# ... edit with your editor ...
scripts/dev-onyx.sh check --all-targets
scripts/test-onyx.sh
scripts/build-and-show.sh
# once the change is good
git commit -am "..." && git push
```

Long runs (release builds, full test suites) must be reported through `g2job`; the wrapper
scripts do that for you when `g2job` is on `PATH`.

## Build and run on onyx

`scripts/build-and-show.sh` is **the** one command that goes from source to a visible app.
Its default is onyx-only:

1. `cargo build --release -j 16` in `~/flectar-build`, reported through `g2job` when present;
2. `scripts/show-onyx.sh` restarts Yours in onyx's Plasma session and prints the pid plus the
   `startup-phase:` lines (`startup-phase: first_frame` is the one that proves the window is on
   screen);
3. with `--screenshot`, it saves a full-screen screenshot.

```bash
ssh onyx 'cd ~/flectar-build && scripts/build-and-show.sh'
```

### The `--tron` deploy (deprecated)

`--tron` rebuilds on onyx and then deploys the binary to tron, restarting the app there. It is
**off by default**, prints a warning, and exists only for a tron deploy that **the owner has
explicitly requested**. Agents must not pass `--tron` on their own initiative.

```bash
# only on the owner's explicit request
scripts/build-and-show.sh --tron
```

`scripts/deploy-tron.sh` is the delivery half of `--tron` and is **deprecated**: it prints a
warning on every run and agents must not call it. It reaches tron as `tron@192.168.50.15` by
default (onyx's plain `tron` ssh alias points at the home-server, not at tron); override it with
`TRON_HOST=<user@host>`.

### `scripts/show-onyx.sh`

```bash
scripts/show-onyx.sh                       # restart, wait for first_frame, print phases
scripts/show-onyx.sh --screenshot          # ... and save /tmp/yours-onyx-<timestamp>.png
scripts/show-onyx.sh --screenshot=/tmp/yours.png
scripts/show-onyx.sh --no-restart          # start only if nothing is running
scripts/show-onyx.sh --bin target/release/yours --timeout 180
```

It starts the app detached with onyx's Plasma session environment (below), writes the output to
`/tmp/yours-onyx.log` (`YOURS_ONYX_LOG` overrides) and waits for `startup-phase: first_frame`
(120 s default; `--timeout` or `FIRST_FRAME_TIMEOUT`). On restart it stops the running instance
owned by the onyx user first (SIGTERM, then SIGKILL).

## onyx's Plasma session

Yours is a GUI app, and the environment of a plain ssh login does not carry the desktop session's
variables, so the scripts set them explicitly:

| Variable | Value on onyx |
|----------|---------------|
| `XDG_RUNTIME_DIR` | `/run/user/1000` |
| `WAYLAND_DISPLAY` | `wayland-1` (Plasma's Wayland socket) |
| `DISPLAY` | `:0` (XWayland) |
| `DBUS_SESSION_BUS_ADDRESS` | `unix:path=/run/user/1000/bus` |
| `XAUTHORITY` | `/run/user/1000/xauth_<random>` (discovered, see below) |

### Display sockets

The Wayland socket is the Plasma session; the X11 socket is XWayland (needed by the X11 paths and
by XWayland clients):

```bash
ls -l /run/user/1000/wayland-*
# srwxr-xr-x ... /run/user/1000/wayland-0
# srwxr-xr-x ... /run/user/1000/wayland-1     <- Plasma session, WAYLAND_DISPLAY=wayland-1

ls -l /tmp/.X11-unix/
# srwxr-xr-x ... X0                           <- DISPLAY=:0 (XWayland)
# srwxr-xr-x ... X1
```

### XAUTHORITY

onyx has **no `~/.Xauthority`**. The Plasma session's X authorisation file is written by KWin into
`/run/user/1000/` with a random suffix:

```bash
ls -l /run/user/1000/xauth_*
# -rw------- ... /run/user/1000/xauth_lfCSfB

# the live value, as KWin passed it to XWayland:
ps -o args= -C kwin_wayland | grep -o -- '--xwayland-xauthority [^ ]*'
# --xwayland-xauthority /run/user/1000/xauth_lfCSfB
```

The suffix changes at login, so never hard-code it: `scripts/show-onyx.sh` reads it from
`kwin_wayland` and falls back to the single `/run/user/1000/xauth_*` file. X clients started over
ssh (including the app's XWayland paths and `spectacle`) need both `XAUTHORITY` and `DISPLAY`.

## Screenshots on onyx

Take screenshots **on onyx**, with the Plasma session environment, using `spectacle`:

```bash
export XDG_RUNTIME_DIR=/run/user/1000
export WAYLAND_DISPLAY=wayland-1
export DISPLAY=:0
export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus
export XAUTHORITY="$(ls /run/user/1000/xauth_* | head -1)"

spectacle -f -b -n -o /tmp/yours-onyx.png    # full screen, background, no notification
file /tmp/yours-onyx.png                     # PNG image data, <width> x <height>
```

`-b -n -o <file>` keeps it headless (no GUI, no notification) and saves the shot; the other useful
captures are `-f` full screen (the default), `-m` current monitor, `-a` active window, `-r` region
and `-d <ms>` for a delay. The Plasma session bus in `DBUS_SESSION_BUS_ADDRESS` is what lets
`spectacle` reach KWin's screenshot API over ssh. `scripts/show-onyx.sh --screenshot` runs exactly
this after `first_frame`.

One line to build, run on onyx and screenshot:

```bash
ssh onyx 'cd ~/flectar-build && scripts/build-and-show.sh --screenshot=/tmp/yours-onyx.png'
```

## Stopping the app on onyx

Only ever stop the app **on onyx**, and only processes owned by `onyx` (uid 1000):

```bash
pgrep -a -u 1000 -x yours      # find it
pkill -u 1000 -x yours         # SIGTERM (graceful)
pkill -9 -u 1000 -x yours      # SIGKILL only if SIGTERM is ignored
```

`scripts/show-onyx.sh` does this before restarting (SIGTERM, wait, then SIGKILL), and
`--no-restart` leaves an already running instance alone. **Never stop the app on tron.**

## Running tests on onyx

Run every `cargo test` on onyx, never on tron. `scripts/test-onyx.sh` is the wrapper:

```bash
scripts/test-onyx.sh                          # cargo test --workspace
scripts/test-onyx.sh -p flectar-mail-core      # one package
scripts/test-onyx.sh my_test_name              # workspace-wide filter
scripts/test-onyx.sh -p flectar-mail-core -- --nocapture
```

It sets the environment the test harness needs:

- `RUST_MIN_STACK=67108864` — Slint's generated test harness needs a large main-thread stack;
- `RUST_BACKTRACE=1`;
- `-j 16` build jobs (`ONYX_JOBS` overrides; onyx has 32 cores);
- `g2job run "yours tests"` when `g2job` is present, so long suites show on the G2 hub.

For CI parity pass the CI flags explicitly: `scripts/test-onyx.sh --workspace --all-targets --locked`.
Directly on onyx the wrapper is just `cargo test` in `~/flectar-build` with the variables above.

## What must never be synced

Nothing secret or local ever moves between hosts. Never copy:

- `.env` files (secrets and API keys);
- `*.db`, `*.db-wal`, `*.db-shm` (multi-GB mail and calendar stores);
- `~/.local/share/yours-mail`;
- `node_modules`;
- `target/` build caches.

Source moves through git only.

## Fallback and rollback

- If a good build is replaced by a bad one, check out the last good commit **on onyx** and rebuild:
  `git checkout <commit> && scripts/build-and-show.sh`.
- App data and lock state (`~/.local/share/yours-mail`, `instance.lock`, `tuta/webview.lock`, the
  config JSON) are untouched by the scripts: they only replace `target/release/yours` and restart
  the process on onyx.
- A tron rollback is only ever done at the owner's explicit request, with
  `scripts/build-and-show.sh --tron` from onyx.
