---
name: fc-build-01-recycled
description: "Lab resources (VMs, disks) are ephemeral unless the user says otherwise; fc-build-01 was recycled 2026-10-15, so the govern lane needs a fresh build VM"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 6f1fd2c8-7212-482c-b7e7-abd3e364c42a
  modified: 2026-10-15T07:54:23.794Z
---

Lab resources are ephemeral unless the user explicitly says a specific one is persistent (user, 2026-10-15). Any VM can be recycled at any time; fc-build-01 (VMID 119), the build host behind the `ops/filmcraft-scene` govern lane, was recycled that day.

**Why:** the lab is shared and gets reclaimed. A build VM is cattle, not a pet.

**How to apply:**
- Never let a VM hold the only copy of anything. Keep work, logs and evidence local: worktrees, the govern job logs, and the scratchpad.
- Don't tag VMs `keep` unless the user asks.
- Make provisioning a script in the lane, so a replacement takes one command; don't hand-run steps.
- When a VM vanishes mid-job, treat its jobs as lost and rerun them on a new one.
- Spawning a dedicated build VM was already authorized ("spawn dedicated VMs with the sizing you need"); size it from the cluster's real free resources.

Related: [[vm-resources-via-govern]].
