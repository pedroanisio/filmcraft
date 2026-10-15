#!/usr/bin/env bash
# Run $WIP_CMD (a shell command line, may use &&) in the VM's filmcraft-wip checkout of the uncommitted tree.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
C=${WIP_CMD:-cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange}
echo "wip: $C"
FC_VM_DIR=filmcraft-wip exec "$HERE/vm-run.sh" sh -c "$C"
