#!/usr/bin/env bash
# Run cargo (check/test/clippy/build) on onyx instead of this workstation.
# The release build lives in scripts/build-onyx.sh; this is the iterative
# companion so a debug compile never eats tron's RAM and disk.
#
#   scripts/dev-onyx.sh test --lib services
#   scripts/dev-onyx.sh check --all-targets
#   scripts/dev-onyx.sh clippy -- -D warnings
set -euo pipefail

HOST="${ONYON_HOST:-onyx}"
REMOTE_DIR="${ONYON_DIR:-flectar-build}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <cargo subcommand> [args...]" >&2
  exit 2
fi

# Keep the remote tree in step with this one, without the build directory.
rsync -a \
  --exclude 'target/' --exclude '.git/' --exclude '.flatpak-builder/' \
  --exclude 'target-ci/' --exclude '*.db' --exclude '*.db-wal' --exclude '*.db-shm' \
  "$ROOT/" "$HOST:~/$REMOTE_DIR/"

status=0
ssh "$HOST" "cd ~/$REMOTE_DIR && RUST_MIN_STACK=67108864 cargo $*" || status=$?
exit "$status"
