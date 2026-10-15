---
name: vm-resources-via-govern
description: "User allows freeing lab build VM resources (finished agents' cargo target dirs) through govern so CI can run"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 6f1fd2c8-7212-482c-b7e7-abd3e364c42a
  modified: 2026-10-14T02:18:47.523Z
---

The user said (2026-10-14): "allowed to use govern to keep vms with the resources freed so it can run" — freeing disk on the lab build VM (fc-build-01) via govern lane jobs is approved so full `cargo xtask ci` fits.

**Why:** release CI (78 ui-egui test binaries, thin LTO) ran out of space on the 100G VM while agents' target dirs held 40G+.

**How to apply:** use the lane command `vm-free` (`vm-free.sh <agent>...`, deletes only `~/filmcraft-<agent>/target` of agents with no running Codex job). Claude Code's built-in rm safety check still blocks the script when it cannot inspect it, so the user may need to run `govern run ops/filmcraft-scene vm-free -- <agents>` themselves. Never free a running agent's dir or the CI checkout. Related: [[sr-core-scene-render-grant]].
