# jedimem

Team memory for coding agents, stored as files in your repo.

An AI monitor watches your agent sessions, extracts the durable knowledge —
conventions, gotchas, requirements, workflows — and proposes it as reviewable
files. You approve; it commits. Approved memories compile into the instruction
files your agents already read, so they work in **Claude Code, Codex, and pi**
without per-tool magic.

Memory lives next to the code, is reviewed like code, and is shared like code.

> **Status: research and design complete. One component built.**
>
> | Piece | State |
> |---|---|
> | `docs/research/` — RE of Claude Code, Codex, pi + git experiments | **done, measured** |
> | `bin/jedimem-compile` — memories → `AGENTS.md`/`CLAUDE.md` | **working**, 7 contract tests green |
> | `plugins/` — hook + settings artifacts per agent | **written**, not yet wired by an installer |
> | capture daemon, extraction, `jedimem` CLI, install | designed, **not built** |
>
> ```bash
> ./bin/jedimem-compile --check   # CI: fails if compiled files are stale
> sh tests/test_compile.sh        # PASS: compiler contract holds
> ```
>
> Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) first.

## Why this instead of a hand-written `AGENTS.md`

Be skeptical here, because the honest answer is "partly you shouldn't."

A hand-curated `AGENTS.md` is free, breaks nothing, and works everywhere. There is
**no published evidence** that automatically captured memory beats it. On plain
recall, a coding agent with `grep` over raw transcripts beats the best RAG pipeline
by ~19 points, and a plain BM25 baseline beats every commercial memory product
tested. jedimem ships those as mandatory baselines in `evals/` and reports where it
loses.

What a hand-written file is genuinely bad at, and where jedimem earns its place:

- **Nobody writes down the gotcha** that just cost them an afternoon. They fix it
  and move on. Capture is the only way that knowledge survives.
- **Nobody deletes the stale rule.** Migration notes outlive migrations. jedimem
  memories carry expiry conditions and retire themselves.
- **Nobody records the dead end.** "We tried X and reverted it" is the highest-value,
  least-recorded knowledge in software.
- **One file doesn't scale to a monorepo.** Memories carry scope globs; the compiler
  emits per-package sections.
- **A rule with no provenance can't be argued with.** Every memory records the
  session, turn, and commit it came from.

And critically: **more memory is not better.** Unbounded growth measurably *lowers*
agent accuracy — 4 of 4 agents in a controlled study, one dropping 16.75% → 13.05%.
So the always-in-context tier is hard-budgeted and retirement is a core mechanic,
not a cleanup task.

## How it works

```
agent session
     │  capture (async hook, fail-open, 13 ms)
     ▼
refs/jedimem/log         staging: lock-free, invisible, never loses a memory
     │  jedimem review   ← a human approves, in batches
     ▼
.jedimem/memories/*.md   shared: ordinary files, ordinary diffs, ordinary clone
     │  jedimem compile  (deterministic, no LLM)
     ▼
AGENTS.md / CLAUDE.md    delivery: the files your agents already read
```

Two ideas do most of the work:

**Capture needs hooks; delivery needs a compiler.** Every target agent already
reads an instruction file from the repo, so delivery needs no integration at all —
jedimem *generates* those files. A new agent costs one compiler target, not a new
plugin. Only capture and pre-action injection need hooks.

**The daemon never touches your git.** It writes to a side ref using plumbing, so
it never takes `.git/index.lock`, never advances your branch, and never dirties
your working tree. We measured what happens otherwise: 20 concurrent porcelain
commits silently lost **13 of 20 memories**, and a lock-free commit to the checked-out
branch got **silently reverted by the developer's next `git commit`**.

## What it does to your repo

```
.jedimem/
  config.yml            committed — repo id, budgets, enabled kinds
  memories/*.md         committed — one file per memory, never deleted
  compiled/             committed, generated, CI-checked for staleness
  local/                gitignored — offsets, queues, your model choice, keys
AGENTS.md, CLAUDE.md    a generated, delimited section appended
.codex/hooks.json       capture hook (same schema as Claude Code's)
.pi/settings.json       pinned package ref, so teammates auto-provision
```

Nothing else. No writes outside those paths.

## Guarantees

- **Fail-open.** Daemon down, network down, budget spent, malformed response, git
  lock contention — all produce the same result: no injection, `exit 0`, session
  unaffected. A memory tool that occasionally injects nothing is a minor
  disappointment; one that occasionally breaks your agent is uninstalled the same
  day.
- **No secrets required.** The default extraction runtime borrows your existing
  agent login. No API key, no account, no config.
- **No telemetry.** Not "anonymized telemetry" — none.
- **No automatic PRs.** Ever.
- **No memory about a person.** The schema forbids a person as a memory's subject.
- **Nothing deleted.** Memories are superseded, never unlinked; history is kept.

## Costs, measured

- Capture hook: **13 ms** (`sh` + `curl`; Python would be 91 ms, node 124 ms).
- Extraction: batched, ~**$0.17/day** at ~120 human turns/day using the
  zero-config runtime. Per-turn extraction would be $2.52/day for the same work —
  headless `claude -p` bills ~27,500 tokens **before your payload**, which is why
  extraction is batched in a daemon rather than run from a hook.

## Documentation

| Doc | For |
|---|---|
| [`docs/TEAM-GUIDE.md`](docs/TEAM-GUIDE.md) | **start here if you're adopting it on a team** |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | the design and the ten decisions behind it |
| [`docs/MEMORY-KINDS.md`](docs/MEMORY-KINDS.md) | the taxonomy: what gets remembered, and what must not |
| [`docs/REPO-PLAN.md`](docs/REPO-PLAN.md) | how this repo earns adoption; v0.1 definition of done |
| [`SECURITY.md`](SECURITY.md) | threat model, including the risks we cannot fully mitigate |

**Research** — reverse-engineering and experiments, with evidence:

| Doc | Findings |
|---|---|
| [`02-codex.md`](docs/research/02-codex.md) | Codex's hook system is a near-clone of Claude Code's; in-repo hooks pinned by SHA-256; Codex already ships a two-phase memory pipeline |
| [`03-pi.md`](docs/research/03-pi.md) | pi has 32 extension events, full context control, and auto-provisions from a committed settings file |
| [`04-git-format.md`](docs/research/04-git-format.md) | `merge=union` silently resurrects deleted facts; `index.lock` silently loses memories; the side-ref design that fixes both |
| [`07-monitor-runtime.md`](docs/research/07-monitor-runtime.md) | headless extraction costs 27.5k tokens before payload — why batching is architectural |

## License

Not yet licensed. Design and research only.
