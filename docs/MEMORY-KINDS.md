# Memory kinds

The taxonomy is the product. Everything downstream — where a memory is stored,
whether a human must approve it, how it reaches the agent, when it expires — is
determined by its kind. Get this wrong and you get mem0's outcome: 10,134
entries, 97.8% junk.

Two rules govern the whole table:

1. **A kind exists only if it changes behavior.** If two kinds are stored,
   delivered, expired, and approved identically, they are one kind with two names.
2. **Every kind names its failure mode.** A memory kind whose wrongness costs
   nothing is not worth capturing; a kind whose wrongness is catastrophic must not
   be auto-approved.

## Delivery tiers

| Tier | How it reaches the agent | Cost |
|---|---|---|
| `always` | compiled into `AGENTS.md` / `CLAUDE.md` | **budgeted** — competes with real context |
| `scoped` | glob-targeted section, applied when touching matching files | medium |
| `on_demand` | retrieved when asked, or grepped | ~free |
| `pre_action` | injected immediately before a matching tool call | tiny but latency-sensitive |

## The kinds

| Kind | Delivery | Where | Approval | Invalidated by | Failure mode if wrong |
|---|---|---|---|---|---|
| `convention` | `scoped` | shared | agent-proposed, human-approved | refactor, new lint rule | agent writes non-idiomatic code; cheap to fix |
| `requirement` | `always` | shared | **human required** | spec change | agent violates a real constraint; expensive |
| `style` | `scoped` | shared | auto if from a linter/formatter | tooling change | churn in diffs; cheap |
| `workflow` | `on_demand` | shared | agent-proposed | tooling change | agent runs the wrong command; medium |
| `topic` | `on_demand` | shared | agent-proposed | code moves | agent reads the wrong files; cheap |
| `gotcha` | `scoped` | shared | agent-proposed | the footgun is fixed | agent repeats a known mistake; **high value** |
| `negative` | `on_demand` | shared | **human required** | new information | team re-tries a dead end, or wrongly avoids a good path |
| `decision` (ADR) | `on_demand` | shared | **human required** | superseding decision | agent argues against a settled decision; high |
| `runbook` | `on_demand` | shared | agent-proposed | infra change | failed deploy; **high** |
| `constraint` | `always` | shared | **human required** | audit / policy change | security or compliance violation; **severe** |
| `glossary` | `on_demand` | shared | agent-proposed | domain drift | misread requirements; medium |
| `ownership` | `on_demand` | shared | auto from `CODEOWNERS`/git | team change | asks the wrong person; cheap |
| `flaky` | `pre_action` | shared | auto from CI history | test fixed | wasted debugging; medium |
| `external` | `on_demand` | shared | agent-proposed | vendor change | wrong API assumptions; medium |
| `perf` | `scoped` | shared | human required | benchmark change | reintroduced regression; high |
| `migration` | `always` (while live) | shared | **human required** | migration completes → **auto-expire** | writes code in a deprecated pattern; high |
| `preference` | `always` | **user-local** | auto | user changes mind | mild annoyance; must never be team-shared |
| `environment` | `on_demand` | **machine-local** | auto | machine change | broken local setup; cheap |

### The three kinds that carry most of the value

**`gotcha`** — the footgun that already cost someone an afternoon. *"The
integration tests need `DATABASE_URL` pointed at the docker-compose postgres, not
the local one."* Cheap to capture, high value, and almost never written down by
hand because the person who learns it is busy being annoyed.

**`negative`** — *"we tried moving auth into the gateway and reverted it; it broke
SSO refresh."* This is the highest-value and least-captured category in software.
Codebases record what was built; nothing records what was tried and abandoned, so
teams re-litigate the same dead ends yearly. It requires human approval precisely
because it is the most dangerous kind to get wrong: a wrong `negative` memory
permanently forecloses a good option, and nobody will ever question it.

**`migration`** — *"we're mid-move from `httpClient` v3 to v4; new code uses v4."*
Uniquely valuable because it is the kind a hand-written `AGENTS.md` is worst at:
it is urgent, short-lived, and someone always forgets to delete it. jedimem's
advantage is the **auto-expire**: a migration memory names its own completion
condition and retires itself.

### The kinds that must not be shared

**`preference`** — *"I like short commit messages."* Personal, and committing one
dev's preference as a team convention is a real social failure. Stays in
`.jedimem/local/`, never promoted without an explicit human act.

**`environment`** — machine paths, local ports, container quirks. Useless or
harmful on someone else's machine.

The dividing line: **is this about the code, or about the person or the box?** Only
the first is shareable. And no memory of any kind may name a developer as its
*subject* — authorship belongs in provenance metadata, never content.

## Auto-capture safety

Which kinds can a monitor agent reliably infer from a transcript?

| Reliability | Kinds | Why |
|---|---|---|
| **High — auto-propose** | `gotcha`, `workflow`, `convention`, `topic`, `style`, `flaky`, `ownership`, `environment` | grounded in an observable event: a command failed, a human corrected the agent, a linter fired, CI flaked |
| **Medium — propose, flag for review** | `runbook`, `glossary`, `external`, `perf` | inferable but easy to over-generalize from one instance |
| **Low — human required** | `requirement`, `constraint`, `decision`, `negative`, `migration` | these encode *intent and authority*, which a transcript does not contain |

The line is whether the memory is a **description** or a **commitment**. A model
can reliably observe that a command failed. It cannot know whether "don't call
that API directly" is a team rule, one reviewer's opinion, or a temporary
workaround — and guessing wrong writes a fake law into the repo.

**The strongest capture signal is a human correcting the agent.** "No, don't use
axios here" is a labelled training example: a human, unprompted, asserting a rule
at the moment it was violated. Prior research measured that assistant reasoning is
unavailable for mining (every `thinking` block empty) while human corrections and
failed commands are the real signal. Weight extraction accordingly, and treat a
correction that follows a *reverted* action as stronger still.

## Anti-kinds — never capture these

- **Anything about a person.** Not their habits, hours, error rate, or skill.
- **Task state.** "We're currently fixing the login bug" is not memory; it is a
  ticket, and it will be wrong tomorrow.
- **Restated code.** "The `User` model has an `email` field" — the agent can read
  the file, and this rots the instant the file changes.
- **Anything already enforced by tooling.** If the linter rejects it, the linter
  is the memory. Duplicating a lint rule as prose costs context budget and adds a
  second source of truth that will disagree.
- **Secrets, credentials, tokens, customer data, PII.** Redacted before write, and
  never captured verbatim from tool payloads.
- **The agent's own injected memories.** Structurally filtered — see the
  self-amplification guards in `ARCHITECTURE.md`. mem0 reached **808 duplicate
  entries** for one hallucinated preference through exactly this loop.
- **Anything the agent merely inferred and no human confirmed**, for the five
  low-reliability kinds above.

## Lifecycle

```
proposed ──approved──> active ──contested──> contested
   │                     │                      │
   └──rejected──> rejected (kept as tuning corpus)
                         │
                    superseded ──> superseded (kept, never deleted)
                         │
                     expired  (auto: migration complete, TTL, unused)
```

Never `deleted`: delete-vs-edit is a hard git conflict, and destroying a true fact
is worse than carrying a stale one. Graphiti's unscoped semantic judge invalidated
**41% of a production graph, ~3 of 4 audited invalidations being collateral** —
which is why supersession here is decided by code on a subject-key match, gated by
a structural check, and **forbidden when the newer claim has weaker provenance
than the older one**. A newer statement is not automatically a truer one.

`expired` deserves emphasis because it is the mechanism nobody ships. Unbounded
growth **measurably lowers** agent accuracy (4 of 4 agents in a controlled study;
one dropped 16.75% → 13.05%), so retirement is a correctness feature, not
housekeeping. Three triggers:

1. **Self-naming** — `migration` memories declare their completion condition.
2. **Disuse** — never retrieved, never matched, in N days. Codex tracks
   `usage_count`/`last_usage` for exactly this; we should too.
3. **Budget pressure** — when the `always` tier hits its cap, **demote** to
   `scoped`; never silently truncate. Codex's `project_doc_max_bytes` will
   truncate `AGENTS.md` without telling anyone, so the budget must be ours to
   enforce.

## Scoping in a monorepo

Every memory carries a `scope` glob. A memory whose scope is `packages/api/**`
must never be compiled into an instruction file consumed while working in
`packages/web/`. Both Codex (nested `AGENTS.md` applying within its subtree) and
Cursor-style rule targeting establish the precedent; the compiler emits per-scope
sections and nested files rather than one global blob.

Unscoped memories default to repo-wide, which should feel expensive — a rule that
applies everywhere consumes everyone's context budget forever, and most rules that
look global are really about one package.
