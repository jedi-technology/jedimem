# pi plugin

**pi needs nothing for delivery.** It reads `AGENTS.md` and `CLAUDE.md` natively —
verified by its own flag: `--no-context-files  Disable AGENTS.md and CLAUDE.md
discovery and loading`. The compiled files just work.

The extension is the *premium* path, never load-bearing. It unlocks what hooks in
the other agents cannot do:

| Event | Capability |
|---|---|
| `pi.on("context")` | rewrite the entire message array before every LLM call |
| `session_before_compact` / `session_compact` | **re-assert memories across compaction** — impossible in Claude Code |
| `agent_settled` | natural trigger for batched extraction |

Distribution is pi's best feature and the model we push the others toward: commit
`settings.json` with a pinned package ref, and every teammate is provisioned on
next launch with no install command.

Two hard rules:

1. **Exclude `role: "custom"` messages from extraction.** That is the marker for
   extension-injected content; ingesting it creates the self-amplification loop.
2. **Never touch `before_provider_request`.** Rewriting provider requests is how a
   memory tool becomes the prime suspect for every unrelated bug.

Session files are at `~/.pi/agent/sessions/--<path>--/<timestamp>_<uuid>.jsonl`,
form a **tree via `parentId`** (branching happens in place), and are **rewritten on
load** when migrated between format versions — so a bare byte-offset checkpoint is
unsafe. Walk `parentId` from the active leaf; do not read top to bottom.
