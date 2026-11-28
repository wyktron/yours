# Development guide

Yours is the Rust/Slint desktop mail app at <https://github.com/wyktron/yours> (branch `main`).
Development is split across two hosts: **onyx** edits, builds, tests and commits; **tron** only
displays the running app. Everything below assumes that split.

## Hosts and responsibilities

| Host | Role | Checkout | Notes |
|------|------|----------|-------|
| **onyx** | Edit / build / test / commit host (canonical) | `/home/onyx/flectar-build` | Real clone of `origin/main`; warm `target/` (~11 GB of build cache); Fedora 45, 32 cores, ~1.7 TB free. |
| **tron** | Display node (runs the GUI) | `/home/tron/Code/flectar-mail` | Kept as fallback only. It must **not** be used for development any more; it is the machine where the app actually runs. |

The onyx checkout is the source of truth; never edit or push code from tron.

## Where the code lives

Canonical checkout on onyx:

```bash
/home/onyx/flectar-build      # git clone of origin/main, warm target/
```

At migration, tron's `.git` was rsynced to onyx **without** `--delete` and onyx HEAD matched
`origin/main`; the tron checkout was not deleted and remains the fallback copy. The helper
scripts used below are committed in the repo under `scripts/` and work from either host:

```bash
scripts/dev-onyx.sh       # run any cargo command on onyx
scripts/build-and-show.sh # build on onyx, deploy to tron, restart the app
scripts/deploy-tron.sh    # fetch and install the binary on tron (usually called by build-and-show.sh)
```

## Everyday commands

A typical day:

```bash
ssh onyx
cd ~/flectar-build
git pull
# ... edit ...
scripts/dev-onyx.sh check --all-targets
scripts/dev-onyx.sh test --workspace
git commit -am "..." && git push
scripts/build-and-show.sh
```

`scripts/dev-onyx.sh <cargo subcommand...>` runs **any** cargo command on onyx from anywhere.
On tron or a laptop it forwards the command over ssh; on onyx it runs locally in
`~/flectar-build`. It sets `RUST_MIN_STACK=67108864` for you.

```bash
scripts/dev-onyx.sh check --all-targets
scripts/dev-onyx.sh test --workspace
scripts/dev-onyx.sh clippy -- -D warnings
scripts/dev-onyx.sh build --release -j 16
```

Long cargo runs (release builds, full test suites) must be reported with `g2job` so the G2 hub
ETA board and watch see them. Never start them on tron:

```bash
g2job run "yours tests" -- cargo test --workspace
```

## Build and show

`scripts/build-and-show.sh` is **the** one command that goes from source to a visible app. Run
it on onyx. It runs `cargo build --release -j 16` in `~/flectar-build` (reported through
`g2job` when present), then streams `deploy-tron.sh` to tron and runs it there.

```bash
ssh onyx
cd ~/flectar-build
scripts/build-and-show.sh
```

`build-and-show.sh` reaches tron as `tron@192.168.50.15` by default (onyx's plain
`tron` ssh alias points at the home-server, not at tron). Override it with
`TRON_HOST=<user@host> scripts/build-and-show.sh` if tron moves.

`deploy-tron.sh` does the delivery side. It fetches
`onyx:~/flectar-build/target/release/yours` over ssh, writes a same-directory temp file, checks
the sha256 against the onyx side, atomically renames it over `target/release/yours`, restarts the
app, and prints the new pid plus the startup-phase lines from `/tmp/yours-deploy.log`. It never
touches the app lock configuration, `~/.local/share/yours-mail` (DBs, `instance.lock`,
`tuta/webview.lock`) or any config JSON, so the deployed state stays exactly as it was.

App restart environment used by `deploy-tron.sh`:

```bash
DISPLAY=:0
WAYLAND_DISPLAY=wayland-1
XDG_RUNTIME_DIR=/run/user/1000
DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus
```

Verify the visible result in the deploy log with `grep 'startup-phase: first_frame'
/tmp/yours-deploy.log`; the log also carries the other phases (`window_shown`, `core_opened`, ...)
that help when `first_frame` never arrives.

The old `scripts/build-onyx.sh` (which rsynced a tron tree to onyx) is **retired**: it now
refuses to run unless `ALLOW_LEGACY_BUILD_ONYX=1` is set, because rsyncing a tron tree over the
canonical onyx checkout would overwrite the source of truth with an older tree.

## Running tests

From tron or a laptop:

```bash
scripts/dev-onyx.sh test --workspace
```

Directly on onyx:

```bash
cd ~/flectar-build
cargo test -j 16
cargo test -p flectar-mail-core <name>   # single test
```

Full suites are long: report them with `g2job run "<name>" -- <command>` and keep them on onyx.

## What must never be synced

Nothing secret or local ever leaves tron via rsync. Never sync:

- `.env` files (secrets and API keys);
- `*.db`, `*.db-wal`, `*.db-shm` (multi-GB mail and calendar stores);
- `~/.local/share/yours-mail`;
- `node_modules`;
- `target/` build caches.

The repo's tracked source is the only thing that moves; builds are done from source on onyx.
onyx has the rustup stable toolchain, and `pkg-config` finds `webkit2gtk-4.1` (2.54.3) and
`gtk+-3.0` (3.24.52).

## Fallback and rollback

- The tron checkout at `/home/tron/Code/flectar-mail` stays in place as the fallback copy. It is
  not deleted, and it is where the app runs.
- To roll back a bad deploy, rebuild and redeploy the last good commit from onyx with
  `scripts/build-and-show.sh`. `deploy-tron.sh` only swaps the binary and restarts, so app data
  and lock state are untouched.
- `deploy-tron.sh` can be run directly on tron when only the delivery step needs repeating.

Do not develop on tron, and do not push code from tron. onyx is the build/test/commit host.
