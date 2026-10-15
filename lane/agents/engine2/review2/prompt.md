You are reviewing commit HEAD (S8) in this FilmCraft worktree. It is read-only: don't edit anything or run cargo. Diff: `git show HEAD`.
It adds the engine scene commands `file.importScene` and `file.exportScene`, structured diagnostics over the control channel,
the CLI and MCP, and hybrid overlay rendering through scene-render (`Services::is_filesystem_backed`).
Rules: AGENTS.md (never crash: no unwrap/expect/panic outside tests, hostile input, bounded allocations), the layering in
docs/architecture.md, everything is a command, and exact Tick time.
Report only real defects, in priority order (P1/P2), each with file:line, a concrete failing scenario and a fix. Leave out style nits.
If you find nothing significant, say so.
