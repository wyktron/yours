#!/usr/bin/env bash
# dev-onyx.sh — run ANY cargo command against the canonical Yours checkout on onyx.
#
# Yours is developed on onyx: onyx is the source of truth (edit, build, test,
# commit, push). tron is a display node that runs the GUI.
#
# From tron (or any machine that can ssh to onyx):
#   scripts/dev-onyx.sh check --all-targets
#   scripts/dev-onyx.sh test --workspace
#   scripts/dev-onyx.sh clippy -- -D warnings
#   scripts/dev-onyx.sh build --release -j 16
#
# Run on onyx itself, the same command executes in the local checkout.
# There is no tron->onyx file sync any more: pull on onyx with 'git pull'.
set -euo pipefail

HOST="${ONYX_HOST:-onyx}"
REMOTE_DIR="${ONYX_DIR:-flectar-build}"
LOCAL_CHECKOUT="$HOME/$REMOTE_DIR"

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <cargo subcommand> [args...]" >&2
  exit 2
fi

if [ -d "$LOCAL_CHECKOUT/.git" ]; then
  cd "$LOCAL_CHECKOUT"
  export RUST_MIN_STACK="${RUST_MIN_STACK:-67108864}"
  exec cargo "$@"
fi

# Forward the exact argument vector over ssh; printf %q keeps the quoting sane.
remote_cmd="cd ~/$REMOTE_DIR && export RUST_MIN_STACK=\${RUST_MIN_STACK:-67108864} && exec cargo $(printf '%q ' "$@")"
exec ssh -t "$HOST" "$remote_cmd"
