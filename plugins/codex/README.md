# Codex plugin

Installed as `<repo>/.codex/hooks.json`. Codex reads in-repo hooks per project and
pins each by SHA-256 in the *user's* global `~/.codex/config.toml`:

```toml
[hooks.state."<repo>/.codex/hooks.json:post_tool_use:0:0"]
trusted_hash = "sha256:91b57a3b…"
```

Editing the file revokes trust until the user re-approves. **This is the security
model jedimem adopts rather than invents** — it is the best available answer to
repo-borne hook execution, and it already ships.

Codex specifics:

- `AGENTS.md` is size-capped by `project_doc_max_bytes`, and it truncates
  **silently**. The compiler enforces its own budget and demotes rather than
  truncates.
- Nested `AGENTS.md` files apply within their subtree — the monorepo scoping
  mechanism.
- `codex exec` is the non-interactive extraction runtime.
- Codex ships its own local memory (`~/.codex/memories_1.sqlite`, two-phase with a
  lease queue). It is per-user and per-machine — not shared, not reviewable, not
  available to Claude Code or pi users. That gap is why jedimem exists.
