#!/usr/bin/env bash
# One Codex team agent for phase B: runs `codex exec` in the agent's own filmcraft worktree with the prompt in
# agents/<name>/prompt.md. Network is on only so the agent can reach the build VM through vm-agent.sh (tests
# never run on the workstation). The final reply (govern queue-reply schema) lands in agents/<name>/reply.json.
# usage: codex-agent.sh <agent-name>        (run as a govern lane job: no dispatch timeout)
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
NAME=${1:?agent name}
case $NAME in *[!a-z0-9-]*|'') echo "bad agent name: $NAME" >&2; exit 2 ;; esac
WT=/home/admin/src/filmcraft-wt/$NAME
AG=$HERE/agents/$NAME
[ -d "$WT" ] || { echo "no worktree $WT" >&2; exit 2; }
[ -f "$AG/prompt.md" ] || { echo "no prompt $AG/prompt.md" >&2; exit 2; }
rm -f "$AG/reply.json"
exec codex exec \
  -s workspace-write \
  -c sandbox_workspace_write.network_access=true \
  -C "$WT" \
  --add-dir /home/admin/src/filmcraft/.git \
  --add-dir "$AG" \
  --skip-git-repo-check \
  --output-schema /home/admin/src/super-tools/tools/govern/schemas/queue-reply.json \
  -o "$AG/reply.json" \
  - < "$AG/prompt.md"
