# scene-interchange handoff (2026-10-15)

This branch holds no code. It keeps the coordination state of the sr-core scene-interchange work, so the work can be
resumed on a new workstation: the plans, the agent prompts and replies, the reviews, the lane scripts and the
agent memory notes. The workstation and the lab build VM it used were both recycled.

## Branches on this fork

| Branch | What | State |
|---|---|---|
| `scene-interchange` | S0–S11 (scene import/export, corpus) and Phase B (scene-render as an external renderer): 31 commits on top of `main` (cee1feb1) | Every commit was gated on the VM except the head pair. The full `cargo xtask ci` on HEAD 1f8028c4 was cut off by the VM recycle, so **HEAD is not CI-verified**. |
| `b/vpkgengine` | S10 engine commands `file.importVpkg` / `file.exportVpkg`, one WIP commit on 94bea42e | Rounds 0–1 passed the VM gate (1,694 tests). Fix round 2 was interrupted and is **not built or tested**. |
| `handoff/scene-interchange` | this branch | — |

`main` here and upstream `storytold/filmcraft` are untouched. The plan was to fast-forward this fork's `main` to a
CI-green `scene-interchange`.

## Layout

- `lane/PLAN.md`, `lane/PLAN-B.md`: the task plans (Phase A S0–S12, Phase B). The "Design" section is the spec.
- `lane/agents/<name>/`: each Codex agent's `prompt.md` (with appended FIX ROUND sections), `promptN.md`/`replyN.json`
  history and `reply.json`.
- `lane/reviews/<name>/`: the read-only Codex review prompts and verdicts. Raw `codex*.log` traces were left out.
- `lane/scene-render-issues.md`: draft of three scene-render audio bugs found during S11. **Not filed upstream.**
- `lane/*.sh`, `lane/govern.json`, `lane/vm.env`: the govern lane `ops/filmcraft-scene`.
  - `vm-sync.sh` ships the local tree (bundle + patch + untracked) to the VM.
  - `vm-run.sh` and `vm-agent.sh` run commands there.
  - `vm-provision.sh` builds a new ephemeral build VM (it was written but never run).
  - `vm.env` names the VM.
  - Paths assume the old layout `/home/admin/src/{filmcraft,filmcraft-wt,render-frontend-js/PALS-Notes}`.
  - Govern job logs were left out.
- `memory/`: the agent's persistent memory notes for this repo (owner rules and grants; see below).

## Owner rules that are not in the repo

- **No cargo on the workstation.** Every build and test runs on a lab VM, through the lane scripts.
- **Lab resources are ephemeral** unless the owner says otherwise. Never let a VM hold the only copy of anything.
- **Code borrowing:** the owner wrote both sr-core and scene-render and allows borrowing their code into FilmCraft,
  with a provenance row in `ATTRIBUTION.md` (`memory/sr-core-scene-render-grant.md`).
- **Asking first:** ask before any new dependency, push, PR, VM creation, or filing issues upstream. Never push to
  `storytold`.
- **Commits:** one task id per commit, gated on the VM before it lands on `scene-interchange`. Agents work in
  worktrees on `b/<name>`. The integrator cherry-picks, so agent branches are patch-merged, not ancestors.

## Next steps, in order

1. **Build VM.** Provision one:
   `lane/vm-provision.sh <vmid> <name> pve-note-1 14 24576 200G`, after fixing the paths in `lane/vm.env`
   and the scripts. pve-note-1 had the most headroom: 16 cores, 57 GiB of free RAM, 1.2 TB of local-lvm.
2. **CI on `scene-interchange` HEAD.** Run `ci-head`, then check two things:
   - It is green.
   - The pinned sr-core corpus reports 0 known regressions. `KNOWN_REGRESSIONS` in
     `crates/interchange/tests/sr_core_corpus.rs` is empty on purpose.
3. **Finish vpkgengine fix round 2.** The task is in `lane/agents/vpkgengine/prompt.md`, under "FIX ROUND 2", with
   the integrator decisions.
   - Finding 1 (concurrent symlink race) is documented only, not fixed. Closing it fully needs a capability crate
     such as `cap-std`, which is a new dependency and needs owner approval.
   - Then re-review with `lane/reviews/vpkgengine/prompt2.md` as a model.
   - Squash the WIP into one `S10:` commit, cherry-pick it onto `scene-interchange`, and run CI again.
   - Also add the `Cargo.lock` refresh: one line, `"filmcraft-vpkg"` in the engine deps. It is in the WIP commit.
4. **S12 docs.**
   - Write a new `docs/interchange-scene.md`.
   - Update `docs/gaps.md`, `ROADMAP.md` and `docs/file-format-parity.md`.
5. **Push.** Fast-forward this fork's `main` to the green HEAD, with owner approval.
6. **Upstream bugs.** Hand `lane/scene-render-issues.md` to the owner, plus the verifier gaps that fix round 2
   reports for `crates/vpkg` and sr-core `refs.py`: `tiles@src`, `tiles@cache`, `volume@src` and file-valued
   `basemap@mapStyle` are not classified as inputs.
