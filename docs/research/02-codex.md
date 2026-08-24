# Codex CLI 0.149.0, reverse-engineered

**Method:** direct inspection of the installed binary, its config, its on-disk
state, and its *locally bundled official documentation* (Codex ships its own
manual under `~/.codex/skills/.system/openai-docs/`). All claims below are
VERIFIED against this machine unless labelled otherwise.

## Executive summary — the load-bearing facts

1. **Codex has a hook system, and it is a near-clone of Claude Code's.** Same
   config shape, same event names, same JSON control protocol. One hook
   implementation can serve both tools.
2. **Hooks can live inside the repo**, at `.codex/hooks.json`, per project. This
   is exactly the distribution channel jedimem needs — no global install
   required.
3. **Codex solved repo-borne hook trust with a hash allow-list.** Each in-repo
   hook is pinned by SHA-256 in the user's global config. Changing the file
   revokes trust until the user re-approves. This is a *published, working
   answer* to the central security problem of an in-repo design, and jedimem
   should adopt the same pattern rather than invent one.
4. **Codex already ships a two-phase memory pipeline of its own**, in SQLite,
   with a lease-based job queue and watermarks — the architecture mem0 deleted.
   This is both validation of the design and a competitor to be honest about.
5. **Session history is JSONL rollouts *plus* a SQLite projection**, and Codex's
   own projection state is a **byte offset into the rollout file** — the same
   incremental-tailing checkpoint strategy we independently arrived at.
6. **Plugin manifest is `.codex-plugin/plugin.json`**, mirroring Claude Code's
   `.claude-plugin/plugin.json`, and it declares `hooks` as a path.
7. `codex exec` is a non-interactive mode usable as an extraction runtime.
8. `AGENTS.md` is the instruction surface, it is **size-capped**
   (`project_doc_max_bytes`), and nested files apply within their subtree.

## Install evidence

```
$ readlink -f $(which codex)
~/.codex/packages/standalone/releases/0.149.0-x86_64-unknown-linux-musl/bin/codex
$ file <that>
ELF 64-bit LSB pie executable, x86-64, static-pie linked, stripped
$ ls -la <that>   →  258,322,048 bytes
```

Rust, static, **stripped**. `strings` works but yields concatenated blobs with
no symbol boundaries, so string-mining gives you *vocabulary*, not structure.
The bundled docs are far better evidence and should be preferred.

## The hook system — VERIFIED

Found by reading the user's actual global config, `~/.codex/config.toml`:

```toml
[hooks.state]

[hooks.state."/home/dev/projects/acme-api/.codex/hooks.json:post_tool_use:0:0"]
trusted_hash = "sha256:91b57a3b21e8450e4af2a92639bc6a0efd74d745392041270e11b4f07de361c6"
```

Three things are proven by that one stanza:

- in-repo hooks are real, at `<repo>/.codex/hooks.json`;
- the trust key is `<file>:<event>:<group_index>:<hook_index>`;
- trust is **content-pinned by SHA-256**, stored in the *user's* config, not the
  repo's.

And the in-repo file itself (a real one on this machine):

```json
{
  "description": "Format changed source files after Codex writes them.",
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write",
        "hooks": [
          {
            "type": "command",
            "command": "bash \"$(git rev-parse --show-toplevel)/scripts/lint-changed.sh\"",
            "statusMessage": "Formatting changed files",
            "timeout": 30
          }
        ]
      }
    ]
  }
}
```

**That is the Claude Code hook schema, field for field** — `hooks.<Event>` →
array of matcher groups → `matcher` regex → inner `hooks` array of
`{type: "command", command, timeout}`. The only visible addition is
`statusMessage`.

Note also the idiom in the command itself: `$(git rev-parse --show-toplevel)`.
Codex's own examples resolve the repo root at hook-execution time rather than
baking an absolute path — which is what makes an in-repo hook work across
clones, machines, and worktrees. jedimem should use the same idiom.

### Event names and the control protocol

Event identifiers appear in two casings — CamelCase in the hooks file,
snake_case in the trust-state key (`PostToolUse` ↔ `post_tool_use`). Extracted
from the binary:

| Event (CamelCase) | Also seen as |
|---|---|
| `PreToolUse` | `pre_tool_use` |
| `PostToolUse` | `post_tool_use` |
| `SessionStart` | — |
| `SessionEnd` | — |
| `UserPromptSubmit` | — |
| `SubagentStart` | — |
| `Stop` / `Stopped` | `stop` |

Control-protocol strings present in the binary:

```
hookSpecificOutput   additionalContext   permissionDecision
allow   deny   ask   systemMessage   suppressOutput   continue
PreToolUseDecisionWire
```

This is Claude Code's hook JSON protocol verbatim. **Consequence:
`additionalContext` exists, so Codex hooks can inject text the model sees** —
Codex is not observe-only. VERIFIED by string presence; the exact injection
semantics and any size cap are UNVERIFIED (Claude Code caps injections at 10,000
chars; assume a cap exists here too until measured).

Also present: `hooks.managed_dir`, `hooks.windows_managed_dir`,
`project_root_markers`, and `HookHandlerConfig` / `MatcherGroup` type names,
confirming a structured config deserializer rather than ad-hoc parsing.

## Config surface and precedence

From the bundled manual (`codex-self-knowledge.md`), the documented precedence,
strongest first:

1. Prompt / thread context — one-off constraints
2. **Repository `AGENTS.md`** — durable team conventions; *nested files apply
   more specifically within their subtree*
3. **Project `.codex/config.toml`** — per-repo settings incl. sandbox, MCP,
   hooks, model
4. Global config — personal defaults across repositories

Relevant config keys mined from the binary:

| Key | Why it matters to jedimem |
|---|---|
| `project_doc_max_bytes` | **AGENTS.md is size-capped.** A compiled memory file that exceeds it is silently truncated. Hard budget required. |
| `project_doc_fallback_filenames` | Codex will read alternative instruction filenames — a compile target list, not a single file. |
| `model_instructions_file`, `developer_instructions` | Additional injection surfaces |
| `notify` | External notification command — an observe-only capture channel |
| `project_root_markers` | How Codex decides what "the project" is |
| `marketplaces` | Plugin distribution |
| `memory_consolidate_global` | Codex's own memory feature |

## Session persistence — VERIFIED

Two layers, both present on this machine:

**1. JSONL rollouts** (the durable log):
```
~/.codex/sessions/2026/08/23/rollout-2026-08-23T10-55-00-01a02e42-....jsonl
```
Date-sharded directories, one file per thread, UUIDv7-style ids.

**2. A SQLite projection** — `thread_history_1.sqlite` (53 MB here):

```sql
CREATE TABLE thread_items (
  thread_id TEXT, turn_id TEXT, item_id TEXT,
  rollout_ordinal INTEGER, created_at_ms INTEGER,
  item_json TEXT, item_type TEXT, updated_at_ordinal INTEGER,
  PRIMARY KEY (thread_id, turn_id, item_id));

CREATE TABLE thread_history_projection_state (
  thread_id TEXT PRIMARY KEY,
  next_rollout_byte_offset INTEGER NOT NULL,
  next_rollout_ordinal INTEGER NOT NULL);
```

**Codex tails its own rollout JSONL by byte offset and projects into SQLite.**
That is precisely the incremental-reader design in `auto-memory/automem/reader.py`
— independently arrived at, now confirmed as the approach the vendor itself uses.
The `migrate-rollouts` subcommand ("migrate legacy local sessions to paginated
thread history") confirms this projection layer is newer than the JSONL.

`state_5.sqlite` additionally holds `threads` (with `cwd`, `rollout_path`,
`tokens_used`), `thread_spawn_edges` (parent→child, i.e. **subagent threads are
tracked explicitly** — no filename sniffing needed), and
`external_agent_config_imports` with a `provider_id` column, implying Codex
imports configuration from *other* agents.

### Which to read

Prefer the **rollout JSONL** as the capture source: it is the durable, ordered,
append-oriented log, and reading a live SQLite WAL that another process owns is
an unnecessary hazard. Use `state_5.sqlite` read-only for discovery (thread →
`cwd`, `rollout_path`, spawn edges), which saves us from re-implementing
path-encoding guesswork. UNVERIFIED: whether rollouts are ever rewritten in
place (Claude Code's are, via `performRemoveByUuid`); until proven otherwise,
checkpoint with a head fingerprint plus size, not a bare offset.

## Codex's own memory feature — the honest competitive note

`memories_1.sqlite`:

```sql
CREATE TABLE stage1_outputs (
  thread_id TEXT PRIMARY KEY, source_updated_at INTEGER,
  raw_memory TEXT NOT NULL, rollout_summary TEXT NOT NULL, rollout_slug TEXT,
  generated_at INTEGER, usage_count INTEGER, last_usage INTEGER,
  selected_for_phase2 INTEGER NOT NULL DEFAULT 0,
  selected_for_phase2_source_updated_at INTEGER);

CREATE TABLE jobs (
  kind TEXT, job_key TEXT, status TEXT, worker_id TEXT, ownership_token TEXT,
  started_at INTEGER, finished_at INTEGER, lease_until INTEGER,
  retry_at INTEGER, retry_remaining INTEGER NOT NULL, last_error TEXT,
  input_watermark INTEGER, last_success_watermark INTEGER,
  PRIMARY KEY (kind, job_key));
```

Read that carefully, because it is a design review of our own plan by a vendor:

- **Two phases** (`stage1_outputs` → `selected_for_phase2`) — cheap per-thread
  extraction, then a selective, presumably more expensive consolidation pass.
- **A lease queue** with `ownership_token`, `lease_until`, `retry_remaining`,
  `last_error` — the same job pattern as `auto-memory/automem/store.py`.
- **Watermarks** (`input_watermark`, `last_success_watermark`) for resumable
  incremental work.
- **Usage feedback** (`usage_count`, `last_usage`) — they track whether a memory
  is ever actually used, which is the retirement signal our design needs and
  mem0 lacks.

What Codex's memory is *not*: it is `~/.codex/`-local, per-user, per-machine, and
invisible to the team. Nothing is committed, nothing is shared, nothing is
reviewable, and it does not exist for Claude Code or pi users.

**That gap is jedimem's entire reason to exist**, and it is a much more defensible
position than "we extract memories better." Copy their pipeline shape; compete on
the repo being the substrate.

## Plugins

`codex plugin {add,list,marketplace,remove}` — a marketplace-based installer.
Manifest is `.codex-plugin/plugin.json` (VERIFIED from the bundled
`plugin-json-spec.md`), with `name`, `version`, `skills`, **`hooks`** (path to a
hooks file), `mcpServers`, `apps`, `interface`. Also `~/.agents/plugins/marketplace.json`
as a local marketplace root — note the **vendor-neutral `~/.agents/` path**,
which suggests a shared convention across tools.

One caution from the plugin-creator skill: *"Omit unsupported plugin manifest
fields that validation rejects, including `hooks`."* So the spec documents
`hooks` while the current validator may reject it. **Do not depend on plugin-declared
hooks; ship `.codex/hooks.json` in the repo instead** — which is what we want
anyway.

## What this means for jedimem

| Need | Mechanism | Confidence |
|---|---|---|
| **Capture** | `.codex/hooks.json` in-repo, `PostToolUse`/`Stop`/`SessionEnd` | high |
| **Capture (fallback)** | tail `~/.codex/sessions/**/rollout-*.jsonl` by offset | high |
| **Delivery (bulk)** | compile to `AGENTS.md`, respecting `project_doc_max_bytes` | high |
| **Delivery (pre-action)** | `PreToolUse` hook + `additionalContext` | medium |
| **Discovery** | `state_5.sqlite` read-only: thread → cwd, rollout_path, spawn edges | medium |
| **Extraction runtime** | `codex exec` | medium |
| **Trust model to copy** | SHA-256-pinned in-repo hooks | high |

The single biggest architectural consequence: **write the hooks file once.**
Claude Code and Codex accept the same schema and the same control protocol, so
jedimem's "two plugins" are really one artifact with two filenames.

## Open questions

- Exact `additionalContext` size cap for Codex (Claude Code's is 10,000 chars).
- Whether Codex has an `async: true` equivalent for zero-latency capture.
- Whether rollout JSONL is ever rewritten in place.
- Full documented hook event list — the anchor `config-advanced#hooks` exists in
  the official docs; fetch it to close this.
- Fixed token overhead of `codex exec` (the analogous `claude -p` figure is
  ~27.5k tokens / $0.021 per invocation — see `07-monitor-runtime.md`).
