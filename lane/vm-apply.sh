#!/usr/bin/env bash
# Runs ON the build VM: rebuild ~/filmcraft as the workstation's tree (public base + bundle + patch + untracked).
# usage: vm-apply.sh <base-sha> <head-sha> [dir]   (<head-sha> may be any commit the bundle contains)   (shipped files sit in ~/<dir>-ship)
set -euo pipefail
BASE=$1 HEAD=$2 DIR=${3:-filmcraft}
SHIP=~/$DIR-ship
if [ ! -d ~/"$DIR"/.git ]; then
  if [ -d ~/filmcraft/.git ]; then git clone -q ~/filmcraft ~/"$DIR"; else git clone -q https://github.com/storytold/filmcraft.git ~/"$DIR"; fi
fi
cd ~/"$DIR"
git cat-file -e "$BASE^{commit}" 2>/dev/null || git fetch -q origin
if [ -s "$SHIP/fc.bundle" ]; then
  git fetch -q "$SHIP/fc.bundle" HEAD
  git checkout -q -f --detach "$HEAD"
else
  git checkout -q -f --detach "$BASE"
fi
git reset -q --hard
git clean -qfd -e target
if [ -s "$SHIP/wip.patch" ]; then git apply --whitespace=nowarn "$SHIP/wip.patch"; fi
tar xzf "$SHIP/untracked.tgz"
[ "$(git rev-parse HEAD)" = "$HEAD" ] || { echo "HEAD mismatch: $(git rev-parse HEAD) != $HEAD" >&2; exit 1; }
echo "synced $HEAD, $(git status --porcelain | wc -l) paths differ from HEAD"
