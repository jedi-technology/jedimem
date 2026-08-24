# Making jedimem a repo people actually adopt

This is the brainstorm document: not what jedimem does, but what makes its
*repository* good enough that a team clones it, installs it, and keeps it.

The bar is set by the incumbent. Today a team that wants shared agent memory
writes an `AGENTS.md` by hand. That costs nothing, breaks nothing, and works in
every tool. jedimem is asking for something much bigger — **let an AI write files
into our repo and commit them** — so it has to clear a much higher bar than
"it works."

Everything below is in service of one sentence: *a stranger should be able to
trust this in under five minutes, and remove it in under one.*

---

## 1. The trust problem is the product problem

Read the pitch from a skeptical staff engineer's chair:

> "An AI agent will watch my sessions and commit files to my repo."

Every word of that is alarming. Surveillance, noise, merge conflicts, prompt
injection, secrets in git history, and a junior's mistake canonized as team
convention. If the repo does not visibly answer each of those *before* the
install command, the install command never runs.

So the README's job is not to explain the architecture. It is to answer, in
order:

1. What exactly gets written into my repo? (show the literal diff)
2. Who approves it? (a human, always, for anything shared)
3. What happens when it's wrong? (how do I correct or contest a memory)
4. What does it cost? (tokens/dollars per day, measured, not estimated)
5. How do I remove it completely? (one command, tested in CI)
6. What can it break? (nothing — the fail-open contract, with the test linked)

**Design rule:** every one of those six answers must be verifiable from the repo
itself, not from prose. A claim with a test next to it is worth ten paragraphs.

---

## 2. Show, don't ask: the retroactive demo

This is the single highest-leverage feature for adoption, and nobody in the
space ships it.

Every developer who would install jedimem **already has months of session
transcripts sitting on their disk** (`~/.claude/projects/`, `~/.codex/`, pi's
session store). So the first command should not be `install`. It should be:

```bash
jedimem preview
```

which reads existing local transcripts, extracts candidate memories, and prints
them — **writing nothing, installing nothing, committing nothing**.

Now the pitch is not a promise, it is a receipt: *here are 14 conventions your
team already re-explained to an agent more than once.* If the output is
compelling, the user installs. If it is junk, they walk away — and they should,
because that means it wouldn't have worked for them.

This also gives us the only honest evaluation instrument we have: run `preview`
across many real repos and count how many extracted memories a human calls
useful. That number is the product's actual quality metric, and it belongs in
the README even when it's unflattering.

---

## 3. Publish the benchmark that makes us look bad

Prior research on this project established two findings that most memory
products would bury:

- A plain **BM25** baseline beats every commercial memory product tested.
- A coding agent with `grep` over raw transcripts-as-files beats the best RAG
  pipeline by roughly **19 points**.

The temptation is to omit them. The correct move is to ship them as the
**mandatory baselines** in `evals/`, run them in CI, and print the comparison in
the README — including the columns where jedimem loses.

Two reasons this is right and not merely noble:

1. It forces the design to compete on the axes where it genuinely wins —
   latency, concurrent multi-agent writes, cross-machine and cross-teammate
   sharing, provenance, and access control — rather than on retrieval quality it
   cannot actually deliver.
2. A repo that documents its own losses is the rarest trust signal available.
   For a tool asking to commit to your repo, it may be the decisive one.

**If we cannot beat `grep` on a given task, the README should say to use `grep`.**

---

## 4. Capture needs hooks. Delivery needs a compiler.

The most important structural decision, and the one that makes cross-agent
support tractable.

The naive reading of "support Claude Code, Codex, and pi" is three plugins that
each inject memories at runtime. That is three times the integration surface,
three times the breakage, and it fails the moment a fourth tool appears — or the
moment one tool's hook API changes under us.

Split the problem instead:

| Direction | Mechanism | Agent-specific? |
|---|---|---|
| **Capture** (session → memory) | hooks / session-file tailing | yes, unavoidably |
| **Delivery** (memory → agent) | **compile to the file the agent already reads** | no |

Delivery needs no integration at all, because every one of these tools already
reads a plain instruction file from the repo. jedimem keeps structured memory
files as its source of truth and *compiles* them into each tool's native
format — `CLAUDE.md`, `AGENTS.md`, `.cursor/rules/*.mdc`,
`.github/copilot-instructions.md` — as **generated, committed artifacts**.

Consequences worth stating plainly:

- Delivery works in **any** tool that reads an instruction file, including tools
  that don't exist yet and tools we've never tested. A new agent costs one
  compiler target, not one plugin.
- The compiled files are generated. They get a "do not edit by hand" banner, a
  format version stamp, and a CI check that fails if they're stale — the same
  contract as any generated code.
- Only two things genuinely need a hook: **capture**, and **pre-action
  injection** (the narrow case where a rule must fire immediately before a
  specific tool call — the one delivery tier a static file cannot serve).

This is also what the compliance evidence demands. Prior research measured
retrieval-based memory at **42.5%** preference compliance versus **70.1%** for
compiled rules with a runtime verifier. Compiling into always-read instruction
files is not a convenience shortcut; it is the delivery mode the evidence
supports.

---

## 5. Provenance is the feature that survives contact with a team

A shared memory that nobody can trace is a rumor. Every committed memory must
carry, in its own file:

- the session it came from, and the human turn that triggered it
- the commit and branch the repo was at
- who the author was (which machine, which developer, which agent)
- the confidence and the approval record — agent-proposed vs human-confirmed

This makes three otherwise-impossible commands possible:

```bash
jedimem why "use httpClient, not axios"   # → the session, the turn, the commit
jedimem contest <id> "this changed in v4" # → mark disputed, stop serving it
jedimem log                               # → what changed in memory, and why
```

`jedimem why` is the answer to the staff engineer's real objection. The
complaint is never "this rule is wrong"; it is "I don't know where this rule
came from, so I can't argue with it." Provenance converts an argument about
trust into an argument about a specific claim, which is a solvable argument.

And it gives us the corrective loop: contested memories are the tuning corpus.
Prior research found mem0's own audit at **10,134 entries, 97.8% junk** — with
"no REJECT action" a named root cause. Rejection and contest have to be
first-class, typed, and logged, or the store fills with garbage and *measurably
lowers* agent accuracy.

---

## 6. The surveillance objection, answered structurally

"An AI watches my sessions and writes down what I did" is a performance-review
artifact waiting to happen, and it will be raised by the first person who
notices. Prose reassurance will not settle it. Structure will:

- **Memories are about the code, never about the person.** No memory may name a
  developer as its subject. Authorship lives in provenance metadata for
  traceability; it is never the *content*. This should be enforced by the
  extraction schema, not by a prompt.
- **Personal preferences stay local and untracked by default.** Promotion from
  personal to team-shared is an explicit human act.
- **Capture is visible and interruptible.** `jedimem status` shows what is
  queued; `jedimem pause` stops it; nothing leaves the machine without a commit
  the developer makes.
- **No telemetry.** Not "anonymized telemetry" — none. This is cheap to promise
  and worth a great deal here.

Document these in `SECURITY.md` and `docs/PRIVACY.md`, and make the default
configuration match the promise. A default that requires a config change to
become safe is not a safe default.

---

## 7. Repo layout, and why the repo eats its own food

```
jedimem/
  README.md               front door: 60-second install, the six answers
  docs/
    TEAM-GUIDE.md         "read this, install it, you're good to go"
    ARCHITECTURE.md       design + load-bearing decisions
    MEMORY-FORMAT.md      the versioned SPEC (see below)
    PRIVACY.md            what is captured, what never leaves the machine
    adr/                  our own decisions, in the format we advocate
    research/             the reverse-engineering evidence base
  SECURITY.md             threat model + disclosure policy
  plugins/
    claude-code/          hooks + plugin manifest
    codex/                config fragment + capture shim
    pi/                   config fragment + capture shim
  bin/jedimem             the CLI (one entry point, subcommands)
  compile/                memory files → CLAUDE.md / AGENTS.md / .mdc
  evals/                  BM25 + grep baselines, and our score against them
  .jedimem/               ← jedimem's OWN memories, committed
```

That last line is deliberate. The jedimem repo should be jedimem's own first
user, with its `.jedimem/` directory committed and visible. A visitor then sees
the real thing — actual memories, actual provenance, actual diff noise — instead
of a synthetic example. It is simultaneously documentation, a test fixture, and
proof the maintainers live with their own decisions.

---

## 8. Ship a spec, not just a tool

jedimem targets three agents today and will be asked about a fourth next month.
The way that ends well is `docs/MEMORY-FORMAT.md` as a **versioned specification**
of the on-disk memory format — every field, every enum value, the migration
rules — so that:

- a third party can write an adapter for a tool we've never seen;
- the format can evolve without silently corrupting committed files (memories
  live in git forever; a format change is a *migration*, not a refactor);
- the format can be validated in CI as a schema, so a malformed memory is a
  failing check rather than a confused agent.

`AGENTS.md` became useful because it was a convention anyone could honor, not
because any one tool was good. Same play.

Every memory file carries a `format:` version. `jedimem migrate` upgrades in
place and commits the migration as its own commit, so the diff is reviewable.

---

## 9. What "good to go" has to mean

The stated goal is that any agentic coding tool just reads the repo, installs,
and works. Concretely, that is:

```bash
git clone <repo> && cd <repo>
./jedimem install          # detects installed agents, wires hooks, idempotent
```

and the requirements that implies:

- **Detect, don't assume.** Install only for agents actually present; skip the
  rest silently and report what was skipped.
- **Never clobber.** Users have their own hooks. Append; never overwrite; and
  never double-append on re-run.
- **Idempotent.** Running install twice equals running it once. Tested.
- **Worktree-aware.** Multiple worktrees of one repo share memory but need
  separate local state.
- **Offline-clean.** No network on the install path. Update checks are cached,
  TTL'd, and a no-op when offline.
- **Uninstall is tested in CI**, and leaves the memories behind — they are the
  team's files, not ours.

The plugin lives at the repo root and ships its own scripts: an update check, a
system-prompt fragment, the compiler, and the capture shim. All of which are
executable content distributed through a git repo — which is exactly why the
threat model is the part of this project we cannot hand-wave.

---

## 10. The uncomfortable questions to answer before writing code

Kept deliberately in the repo, unanswered questions and all:

1. **Does automatic capture beat a hand-written `AGENTS.md`?** No published
   evidence says yes. `jedimem preview` across real repos is how we find out,
   and we should be willing to publish a negative result.
2. **Does more memory make agents worse?** Prior research says yes — unbounded
   growth lowered accuracy in 4 of 4 agents studied, one dropping 16.75% →
   13.05%. So a memory *budget* and active retirement are core mechanics, not
   later optimizations. The compiled always-on file must have a hard size cap.
3. **What happens on a hostile PR?** A contributor edits `.jedimem/` or the
   in-repo scripts, and a teammate's agent reads or executes it. This is the
   central security question of an in-repo design and it is not fully solvable
   by us — it needs review gates, capability limits, and honesty about residual
   risk.
4. **Who owns a memory that turns out to be wrong?** Blame in a shared memory
   store is a social problem wearing a technical costume. Provenance plus
   contest-not-delete is our answer; it may not be enough.

---

## 11. Definition of done for v0.1

Not "it works" — these:

- [ ] `jedimem preview` produces useful output on a real repo with zero install
- [ ] Compiled `AGENTS.md` / `CLAUDE.md` are byte-stable and CI-checked for staleness
- [ ] Two agents writing memories concurrently in two worktrees produce **zero**
      merge conflicts — proven by a test, not by argument
- [ ] Fail-open contract test: every failure mode ends in `exit 0`, no injection,
      session unaffected
- [ ] Install → uninstall → `git status` is clean, in CI, on Linux and macOS
- [ ] `SECURITY.md` threat model published, with residual risks named
- [ ] `evals/` runs the BM25 and grep baselines and reports our score honestly
- [ ] Measured cost per day published in the README
- [ ] jedimem's own `.jedimem/` committed and non-trivial
