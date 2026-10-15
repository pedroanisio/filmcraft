#!/usr/bin/env bash
# Sync the working tree to the lab build VM, then run a command in ~/filmcraft there (login shell).
# usage: vm-run.sh <command…>     e.g. vm-run.sh cargo xtask ci
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
"$HERE/vm-sync.sh"
CMD=$(printf '%q ' "$@")
DIR=${FC_VM_DIR:-filmcraft}
PRE=
if [ "$DIR" != filmcraft ]; then
  # Agent checkouts: incremental caches grew to 30G+ per agent on the 99G VM. Build non-incrementally and drop
  # stale caches, unless another cargo is running in this same checkout.
  PRE='export CARGO_INCREMENTAL=0; busy=; for p in $(pgrep -x cargo); do [ "$(readlink /proc/$p/cwd)" = "$PWD" ] && busy=1; done; [ -n "$busy" ] || rm -rf target/debug/incremental; '
fi
. "$HERE/vm.env"
ssh -F /dev/null -o BatchMode=yes "$FC_CTL" "cd ~/pve-lab && ./lab.sh ssh $FC_VM $(printf '%q' "bash -lc $(printf '%q' "cd ~/$DIR && $PRE$CMD")")"
