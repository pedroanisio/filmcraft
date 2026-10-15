#!/usr/bin/env bash
# Mirror the local filmcraft working tree (commits + uncommitted + untracked) onto the lab build VM.
# Nothing is pushed: commits beyond the public base travel as a git bundle via pve-ctl + lab.sh scp.
# FC_SYNC_HEAD_ONLY=1 ships the commits alone (gating exactly HEAD); FC_VM_DIR picks the checkout on the VM
# (default filmcraft) so a work-in-progress tree can build next to a running gate.
# FC_SYNC_REF=<rev> gates that commit instead of HEAD (it must be an ancestor of HEAD; implies FC_SYNC_HEAD_ONLY=1).
# FC_REPO picks the local tree (default the main checkout; agents pass their worktree). -F /dev/null skips the
# root-owned system ssh config, which the Codex sandbox cannot read.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=${FC_REPO:-/home/admin/src/filmcraft}
. "$HERE/vm.env"; CTL=$FC_CTL
VM=$FC_VM
DIR=${FC_VM_DIR:-filmcraft}
BASE=cee1feb185e675a108f78067f71ad86a640ace3d
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT

cd "$REPO"
HEAD=$(git rev-parse HEAD)
GATE=$HEAD
if [ -n "${FC_SYNC_REF:-}" ]; then
  GATE=$(git rev-parse --verify "$FC_SYNC_REF^{commit}")
  git merge-base --is-ancestor "$GATE" "$HEAD" || { echo "FC_SYNC_REF $FC_SYNC_REF is not an ancestor of HEAD" >&2; exit 2; }
  FC_SYNC_HEAD_ONLY=1
fi
if [ "$HEAD" != "$BASE" ]; then
  git bundle create "$OUT/fc.bundle" "$BASE..HEAD" 2>/dev/null
else
  : > "$OUT/fc.bundle"
fi
if [ "${FC_SYNC_HEAD_ONLY:-0}" = 1 ]; then
  : > "$OUT/wip.patch"
  tar -czf "$OUT/untracked.tgz" -T /dev/null
else
  git diff --binary HEAD > "$OUT/wip.patch"
  git ls-files -o --exclude-standard -z | tar --null -T - -czf "$OUT/untracked.tgz"
fi
cp "$HERE/vm-apply.sh" "$OUT/"

SHIP=$DIR-ship
ssh -F /dev/null -o BatchMode=yes "$CTL" "mkdir -p ~/$SHIP"
scp -F /dev/null -q -o BatchMode=yes "$OUT"/* "$CTL:$SHIP/"
ssh -F /dev/null -o BatchMode=yes "$CTL" "cd ~/pve-lab && ./lab.sh ssh $VM 'mkdir -p ~/$SHIP' && ./lab.sh scp ~/$SHIP/fc.bundle ~/$SHIP/wip.patch ~/$SHIP/untracked.tgz ~/$SHIP/vm-apply.sh $VM:~/$SHIP/ >/dev/null && ./lab.sh ssh $VM 'bash -l ~/$SHIP/vm-apply.sh $BASE $GATE $DIR'"
