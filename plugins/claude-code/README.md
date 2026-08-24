# Claude Code plugin

Installed to `.claude/settings.json` as appended hook entries, or shipped as a
plugin with `.claude-plugin/plugin.json`.

`hooks.json` here is **byte-identical** to `plugins/codex/hooks.json`. That is the
point: the two tools accept the same hook schema and the same control protocol
(`hookSpecificOutput`, `additionalContext`, `permissionDecision`), so jedimem
maintains one artifact. See `docs/research/02-codex.md`.

Claude Code specifics that the installer must respect:

- **`async: true`** detaches a command hook for zero-latency capture. Add it to
  every capture entry. Undocumented but verified.
- Hook arrays **concatenate** across all five settings scopes — appending is safe,
  duplicating is not. The installer de-duplicates its own entry by command string.
- **Exit 2 blocks the session.** Capture must always `exit 0`, including on
  failure.
- Injected context is capped at **10,000 characters**; beyond that the model
  receives a file-path stub instead of the content.
- **No hook can inject after compaction** — only `CLAUDE.md` is re-read. This is
  why delivery is a compiled file rather than a hook injection.
