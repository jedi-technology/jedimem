# jedimem architecture

**What it is.** Team memory for coding agents, stored as files in your repo and
shared through git. An AI monitor watches your agent sessions, extracts durable
knowledge — conventions, requirements, gotchas, workflows — and proposes it as
reviewable files. Approved memories compile into the instruction files your agents
already read, so they work in Claude Code, Codex, pi, and anything else that reads
`AGENTS.md`.

**What it is not.** A vector database. A cloud service. A replacement for
`AGENTS.md` — it is a *generator* for it.

Everything below is traceable to measured evidence in `docs/research/`. Where a
decision rests on an assumption instead, it says so.

---

## The ten decisions that define the system

| # | Decision | Because |
|---|---|---|
| 1 | Memory is **files in the repo**, one file per memory | per-file merges clean; a single appended file conflicts every time |
| 2 | **Never `merge=union`** | it silently resurrects corrected facts (04, exp 2c) |
| 3 | **Never delete a memory** — supersede in place | delete-vs-edit is a hard conflict (04, exp 3c) |
| 4 | The daemon **never runs `git add`/`commit`** | 20 concurrent commits lost 13 of 20 memories to `index.lock` (04, exp 7a) |
| 5 | Capture accumulates on a **side ref**, `refs/jedimem/log` | perfect isolation, zero loss under concurrency (04, exp 7c) |
| 6 | Sharing happens through **normal committed files** | a fresh clone does not fetch custom refs (04, exp 7d) |
| 7 | **Capture needs hooks; delivery needs a compiler** | every target agent already reads an instruction file |
| 8 | **One hook artifact** serves Claude Code and Codex | their hook schema and control protocol are identical (02) |
| 9 | Extraction is **batched, never per-turn** | headless `claude -p` costs ~27.5k tokens / $0.021 *before payload* (07) |
| 10 | Nothing shared is committed **without a human** | mem0's own audit: 10,134 entries, 97.8% junk (prior research) |

---

## 1. The two-layer git model

The central problem: an automated writer and a human share one repo. Naive
approaches lose data or corrupt the human's workflow — both measured, not
theorized.

```
   agent session
        │
        │  capture (hook, async, fail-open)
        ▼
   refs/jedimem/log ......................... LAYER 1: staging
   • lock-free plumbing commits (CAS on the ref)
   • never touches HEAD, index, or working tree
   • `git status` stays empty; the user cannot tell it is running
   • 20 concurrent writers → 0 lost
        │
        │  jedimem review   (human-initiated, batched)
        ▼
   .jedimem/memories/*.md ................... LAYER 2: shared
   • ordinary files on an ordinary branch
   • arrives with a normal clone, shows in normal diffs
   • one file per memory, ULID-named, never deleted
        │
        │  jedimem compile  (deterministic, no LLM)
        ▼
   AGENTS.md / CLAUDE.md .................... LAYER 3: delivery
   • generated, committed, CI-checked for staleness
```

Layer 1 exists because git's index is a mutex, not a queue. Layer 2 exists
because custom refs don't travel with a clone. The review gate sits exactly at
the boundary between them — not as a policy choice bolted on, but because that is
where the substrate already forces a human-initiated step.

**Why the daemon must never advance the branch**, even lock-free: it does, and it
works, and then the developer's next perfectly ordinary `git commit` commits their
stale index and **silently reverts every memory** (04, exp 7b). Anything that
makes `git status` show unexpected `D` entries has already lost the user's trust.

## 2. Capture needs hooks. Delivery needs a compiler.

The naive reading of "support three agents" is three plugins that inject memories
at runtime: triple the integration surface, triple the breakage, and a fourth
tool means a fourth plugin.

Split by direction instead:

| Direction | Mechanism | Agent-specific? |
|---|---|---|
| **Capture** (session → memory) | hooks / extension events / session tailing | yes, unavoidably |
| **Delivery** (memory → agent) | **compile into the file the agent already reads** | **no** |

Delivery needs no integration, because all three tools already read a plain
instruction file from the repo — verified: Codex documents `AGENTS.md` in its
precedence chain, and pi's `--no-context-files` flag literally reads *"Disable
AGENTS.md and CLAUDE.md discovery and loading."*

So jedimem keeps structured memory files as the source of truth and compiles them
into each tool's native format as **generated, committed artifacts** with a
do-not-edit banner, a format version, and a CI staleness check.

Consequences:
- A new agent costs **one compiler target**, not one plugin.
- Delivery works in tools we have never tested, including ones that don't exist yet.
- Only two things genuinely need a hook: **capture**, and **pre-action injection**
  (the one tier a static file can't serve).

This is also what the compliance evidence demands: prior research measured
retrieval-based memory at **42.5%** preference compliance versus **70.1%** for
compiled rules with a runtime verifier. Compiling into always-read files is not a
shortcut; it is the delivery mode that actually works.

## 3. Per-agent integration

### Claude Code + Codex: one artifact, two filenames

Codex's in-repo `.codex/hooks.json` is **field-for-field identical** to Claude
Code's hook schema — `hooks.<Event>` → matcher groups → `{type: "command",
command, timeout}` — and its binary contains the same control protocol strings
(`hookSpecificOutput`, `additionalContext`, `permissionDecision`, `allow`/`deny`/
`ask`, `systemMessage`, `suppressOutput`). Shared events include `PreToolUse`,
`PostToolUse`, `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `SubagentStart`,
`Stop`.

So jedimem ships **one hooks file**, written to two paths. Use Codex's own idiom
for portability across clones and worktrees:

```json
{ "type": "command",
  "command": "sh \"$(git rev-parse --show-toplevel)/.jedimem/bin/capture.sh\"" }
```

Claude Code specifics that matter: `async: true` gives zero-latency capture;
injections cap at **10,000 characters**; hook arrays **concatenate** across all
settings scopes (so appending is safe, duplicating is not); **exit 2 blocks** — a
memory hook must always exit 0; and **nothing can inject after compaction**.

Codex specifics: `project_doc_max_bytes` caps `AGENTS.md`, so the compiled output
needs a hard budget or it is silently truncated.

### pi: nothing required, much available

pi needs **no integration for delivery** — it reads `AGENTS.md` and `CLAUDE.md`
already. Where the extension is installed, it unlocks what hooks cannot:

- `pi.on("context")` — rewrite the entire message array before every LLM call
- `session_before_compact` / `session_compact` — **re-assert memories across a
  compaction boundary**, which is impossible in Claude Code
- `agent_settled` — a natural trigger for batched extraction

And pi's distribution model is the one to copy: a committed `.pi/settings.json`
with a pinned package ref, because *"pi installs any missing packages
automatically on startup after the project is trusted."* Clone the repo, launch
pi, done — no install command at all.

**Rule: the extension is never load-bearing.** The same memory files must produce
a good result with zero pi integration and a better one with it.

### Self-amplification guards (per agent, mandatory)

A retrieval hook injects memories into the session; the session transcript then
*contains* those memories; extracting from it re-learns what we just told it, and
confidence climbs with no new evidence. mem0 accumulated **808 entries claiming
"User prefers Vim"** this way. This is not a prompt-engineering problem — a prompt
asking the model not to re-ingest its own output is not a control. The filter is
structural:

| Agent | Marker to exclude |
|---|---|
| Claude Code | the `hook_success` attachment |
| pi | messages with `role: "custom"` (was `hookMessage` pre-v3) |
| Codex | our own injected `additionalContext` items — **needs verification** |

Each needs a test that fails if the filter is removed.

## 4. The monitor agent

A separate process, not a hook. The reason is economic and measured: headless
`claude -p` bills **~27,500 tokens and $0.021 to answer "ok"** — the whole system
prompt and tool-definition block is billed per invocation, and
`--disallowedTools` does *not* reduce it. Per-turn extraction at ~120 turns/day
would cost **$2.52/day in pure overhead** for ~$0.18 of real work; the same day
batched into 8 windowed calls costs **$0.17**.

Extraction has no latency requirement, so batching costs nothing and saves 93%.
**The economics dictate the architecture**: a hook fires per event, a daemon
chooses its own window.

Runtime, in preference order:
1. **Host-agent headless** (default) — inherits the user's existing login, so
   **no API key, no secrets, zero config**. This default is what gets jedimem
   installed.
2. **Direct API** (opt-in, one env var) — ~30x cheaper for heavy users.
3. **pi headless** where available — `--no-tools` may avoid the harness tax
   entirely (unmeasured; the highest-value open measurement we have).

Operational requirements, all learned the hard way:
- **Always redirect stdin** (`< /dev/null`) — otherwise `claude -p` blocks 3 s on
  a stdin timeout and risks hanging.
- **Force a tool call with a JSON Schema.** Asked for JSON in prose, the model
  returns a fenced block inside a string and free-styles field types (`confidence:
  "high"` instead of a number). Validate at the tool-call layer so a mismatch
  triggers a model retry, not a dropped memory.
- **Split extraction from consolidation.** Extraction is cheap classification
  (Haiku); consolidation makes supersession decisions and deserves a better model.

## 5. Delivery tiers

Not all memory should reach the agent the same way. Uniform delivery is what makes
instruction files bloat and compliance fall.

| Tier | Mechanism | Use for | Budget |
|---|---|---|---|
| `always` | compiled into `AGENTS.md`/`CLAUDE.md` | non-negotiable rules | **hard cap** |
| `scoped` | glob-targeted section / `.mdc`-style rule | per-package or per-language conventions | medium |
| `on_demand` | retrieved by grep/index when asked | runbooks, ADR rationale, glossary | unbounded |
| `pre_action` | `PreToolUse` hook + `additionalContext` | "before you run migrations, …" | tiny |

The `always` cap is not tidiness. Prior research found unbounded memory growth
**measurably lowers** agent accuracy — 4 of 4 agents in a controlled study, one
dropping 16.75% → 13.05%. So a memory budget and active retirement are core
mechanics, not later optimizations. When the cap is hit, jedimem must demote,
never truncate silently — and Codex's `project_doc_max_bytes` will truncate
silently if we don't.

## 6. Review gate

Automatic capture, human promotion. Concretely:

- The daemon appends candidates to `refs/jedimem/log` continuously — invisible,
  lossless, costless to ignore.
- `jedimem review` shows pending candidates with provenance and a diff.
- Approved ones become files in `.jedimem/memories/` in **one commit**, not one
  commit each.
- Rejections are **kept**, typed, as a tuning corpus. mem0's junk audit named "no
  REJECT action" as a root cause; rejection must be a first-class verdict.

**No automatic PRs.** A team will not tolerate a bot opening noisy PRs, and that
single behavior would get jedimem uninstalled faster than any bug.

Provenance makes review possible. Every memory records the session, turn, commit,
branch, agent, machine, and whether a human confirmed it — enabling:

```
jedimem why "use httpClient, not axios"    # → session, turn, commit
jedimem contest <id> "changed in v4"       # → stop serving it, don't delete
jedimem log                                # → what changed, and why
```

The staff engineer's real objection is never "this rule is wrong", it is "I can't
tell where this rule came from, so I can't argue with it." Provenance converts an
argument about trust into an argument about a claim.

## 7. Config layering

| Layer | Location | Committed? | Holds |
|---|---|---|---|
| Repo | `.jedimem/config.yml` | **yes** | `repo_id`, enabled kinds, budgets, scopes |
| Repo-generated | `.jedimem/compiled/`, `AGENTS.md` sections | **yes** | derived, CI-checked |
| Per-user, per-repo | `.jedimem/local/` | **no** (`.gitignore`) | offsets, queues, model choice, keys |
| Per-user patterns | `$(git rev-parse --git-common-dir)/info/exclude` | never | personal ignores, not imposed on teammates |
| Per-worktree state | `$(git rev-parse --git-dir)/jedimem/` | never | per-checkout offsets |

`info/exclude` resolves into the **common** dir, so it is shared across all
worktrees of a repo — write it once (04, exp 6).

**Secrets:** the default requires none. If a user opts into direct-API mode, the
key is read from the environment and never written to a file jedimem controls.
Nothing under `.jedimem/local/` is ever committed, and a pre-write redaction pass
runs before any memory reaches even the staging ref — because git history keeps a
committed secret forever, and removing it requires rewriting history for the whole
team.

## 8. Identity

Repo identity, in precedence order (04, exp 5): a committed **`repo_id`** ULID;
else a normalized remote URL; else the root-commit hash; never a filesystem path.
The root commit alone is not stable — a `--depth 1` clone produces a *different*
root commit, and a fork produces the *same* one.

On one machine, worktrees of a repo share `--git-common-dir` (the local scope key)
and differ by `--git-dir` (per-worktree state). `git worktree list --porcelain`
enumerates them from any one.

Machine identity is a locally generated ID in `.jedimem/local/`, used for
provenance and ULID entropy. **No memory may name a developer as its subject** —
authorship lives in provenance for traceability, never in content. Enforced by the
extraction schema, not by a prompt, because "memory as performance-review
artifact" is a real adoption blocker and prose reassurance does not settle it.

## 9. Failure contract

> A memory system that occasionally injects nothing is a minor disappointment.
> One that occasionally breaks the agent is uninstalled the same day.

Daemon down, network down, budget exhausted, malformed response, slow response,
stale socket, git lock contention — all degrade to the same observable behavior:
**no injection, exit 0, no error shown, session proceeds.** This must be a test
suite, not an intention. (The predecessor project has one; port it.)

Corollaries: exit 2 blocks in both Claude Code and Codex, so a capture hook must
*always* exit 0 — even when it fails. And the capture hook must not be the thing
that spawns an interpreter: measured spawn costs are `sh`+`curl` **13 ms**, Python
**91 ms**, node **124 ms**.

## 10. Threat model

An in-repo design that ships executable content and reads repo files into an
agent's context has a genuinely hostile surface. The honest version:

| Threat | Vector | Mitigation | Residual |
|---|---|---|---|
| Supply chain | malicious commit to jedimem runs on every teammate's session start | pin versions, never track a floating branch; adopt Codex's **SHA-256 content-pinned hook trust** | **real** — pinning delays, doesn't prevent |
| Repo-borne code | hostile PR edits `.jedimem/bin/*` or the hooks file | hook trust hashes; `CODEOWNERS` on `.jedimem/**`; require review | **real** — a teammate who approves the PR defeats it |
| Prompt injection | a memory file *is* an instruction, with persistence | memories are data, not instructions; human review gate; no memory may grant capabilities | **real and unsolved industry-wide** |
| Secret capture | a key in a transcript becomes a committed memory, forever | redaction before write; never capture env/tool payloads verbatim | history rewrite is the only true remedy |
| Surveillance | memory becomes a performance-review artifact | schema forbids person-as-subject; personal stays local; no telemetry | social, not technical |

Codex has already shipped the best available answer to the second row — in-repo
hooks pinned by content hash in the *user's* global config, so changing the file
revokes trust until re-approved. jedimem should adopt that pattern rather than
invent one, and say plainly in `SECURITY.md` which rows remain unmitigated.

## 11. Milestones

**M0 — prove it's worth building.** `jedimem preview`: read existing local
transcripts, extract candidates, print them, write nothing. Every prospective
user already has months of sessions on disk; this turns the pitch into a receipt
and is the only honest quality metric we have. Also run the mandatory baselines
(BM25, `grep` over transcripts, hand-written `AGENTS.md`) — prior research found
**BM25 beats every commercial memory product** and a `grep`-wielding agent beats
the best RAG pipeline by ~19 points. If we can't beat those, say so.

**M1 — capture, local only.** Side-ref staging, batched extraction, redaction,
`jedimem review`, `jedimem why`. Useful with no server and no team. Dogfood two
weeks before writing any sharing code.

**M2 — compile and deliver.** `AGENTS.md`/`CLAUDE.md` generation with budgets and
tiers, CI staleness check, install/uninstall for all three agents.

**M3 — team.** Promotion into committed files, contest/supersede, `CODEOWNERS`,
monorepo scoping.

**M4 — the premium paths.** pi extension (compaction survival, precise context),
`pre_action` injection, usage-feedback-driven retirement (Codex tracks
`usage_count`/`last_usage`; we should too — it is the retirement signal mem0
lacks).

## 12. What we deliberately do not build

- No vector database, no embeddings, until BM25 + `grep` are measurably beaten.
- No cloud service, no account, no telemetry.
- No automatic PRs.
- No writing outside `.jedimem/`, the two compiled instruction files, and the
  side ref.
- No touching `pi.on("before_provider_request")` — rewriting provider requests is
  how a memory tool becomes the prime suspect for every unrelated bug.
- No memory whose subject is a person.
- No deletion. Supersede, and keep the history.

## 13. The honest competitive position

**Codex already ships a memory feature** — `memories_1.sqlite`, with a two-phase
pipeline (`stage1_outputs` → `selected_for_phase2`), a lease-based job queue, and
usage counters. It is a well-built version of the architecture mem0 deleted, and
it validates this design's shape.

It is also `~/.codex/`-local: per-user, per-machine, invisible to the team, not
reviewable, not shared, and nonexistent for Claude Code and pi users.

**That gap is the entire reason jedimem exists**, and it is far more defensible
than "we extract memories better." The claim to make is not better retrieval — the
literature says we would lose that fight. It is that memory belongs in the repo,
next to the code, reviewed like code, shared like code, and readable by every tool
the team uses.
