#!/usr/bin/env bash
# Free disk on the lab build VM so gates can run (owner-approved 2026-10-14, run it as a govern lane job):
# delete the cargo target dir of finished agents, ~/filmcraft-<name>/target. Checkouts stay; only build output goes.
# usage: vm-free.sh <agent-name>...     Refuses the CI checkout itself and any agent with a running Codex job.
set -euo pipefail
[ $# -gt 0 ] || { echo "usage: vm-free.sh <agent-name>..." >&2; exit 2; }
for n in "$@"; do
  case $n in *[!a-z0-9-]*|'') echo "bad agent name: $n" >&2; exit 2 ;; esac
  if pgrep -af "codex exec .*filmcraft-wt/$n( |$)" >/dev/null; then echo "agent $n is still running; refusing" >&2; exit 2; fi
done
DIRS=$(printf '~/filmcraft-%s/target ' "$@")
HERE=$(cd "$(dirname "$0")" && pwd); . "$HERE/vm.env"
ssh -F /dev/null -o BatchMode=yes "$FC_CTL" "cd ~/pve-lab && ./lab.sh ssh $FC_VM 'df -h / | tail -1; for d in $DIRS; do [ -d \$d ] && du -sh \$d && rm -rf \$d; done; df -h / | tail -1'"
