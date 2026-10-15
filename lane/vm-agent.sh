#!/usr/bin/env bash
# Test runner for a Codex team agent: ship the agent's worktree to its own VM checkout and run a command there.
# usage: vm-agent.sh <agent-name> <command…>      e.g. vm-agent.sh proto cargo test -p filmcraft-scenerender
# The worktree is /home/admin/src/filmcraft-wt/<agent-name>; the VM checkout is ~/filmcraft-<agent-name>.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
NAME=${1:?agent name}
shift
case $NAME in *[!a-z0-9-]*|'') echo "bad agent name: $NAME" >&2; exit 2 ;; esac
export FC_REPO=/home/admin/src/filmcraft-wt/$NAME FC_VM_DIR=filmcraft-$NAME
exec "$HERE/vm-run.sh" "$@"
