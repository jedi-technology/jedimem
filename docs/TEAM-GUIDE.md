# jedimem team guide

**Who this is for:** a team adopting jedimem, and the person who has to justify it
in review.

> **What works today:** `init`, `import`, `review`, `compile`, `status`, `why`,
> `contest`, `list`, `lint`, `pause`/`resume` — with 32 tests. Memory capture from
> live agent sessions (the hooks and the extraction daemon) is designed but not
> built, so today jedimem is a *bootstrap and curation* tool: it imports what your
> repo already knows, and compiles it into the files your agents read.
> Everything about behaviour under git is measured, not assumed (`docs/research/`).

---

## 1. The thirty-second version

Your agents keep re-learning things your team already knows. jedimem captures
those things once, stores them as files in the repo, and compiles them into the
instruction files your agents already read.

- Memories are **files**, in `.jedimem/memories/`, one per memory.
- Nothing shared is committed **without a human approving it**.
- It works in **Claude Code, Codex, and pi** — the same files, no per-tool setup
  for delivery.
- It **cannot break your agent** (fail-open) and **cannot dirty your git**
  (side-ref staging).

## 2. Before you adopt: run the free evaluation

```bash
jedimem init
jedimem import        # dry run by default
```

This reads what your repo already contains — instruction files, ADRs, CODEOWNERS,
and revert commits — and prints candidate memories. It **writes nothing**, makes
**no network calls**, and uses **no model**. Safe on a private repo.

Judge the output as a team. If it surfaces things you recognize as knowledge you
have re-explained to an agent more than once, adopt it. If it looks like noise,
don't — and tell us, because that output is the honest quality metric for this
project.

Also run the baselines, because they are free and they might win:

```bash
jedimem eval --baselines    # BM25, grep-over-transcripts, your existing AGENTS.md
```

If `grep` wins on your repo, **use `grep`.** We would rather you know that from us.

## 3. Install

One command from the repo root. It detects which agents are installed and skips
the rest.

```bash
./jedimem install
```

What it writes, and nothing else:

| Path | Committed? | Purpose |
|---|---|---|
| `.jedimem/config.yml` | yes | repo id, budgets, enabled kinds |
| `.jedimem/memories/` | yes | the memories themselves |
| `.jedimem/compiled/` | yes | generated; CI checks it is not stale |
| `.jedimem/local/` | **no** | your offsets, queues, model choice, keys |
| `AGENTS.md`, `CLAUDE.md` | yes | a delimited generated section is appended |
| `.codex/hooks.json` | yes | capture hook (Codex) |
| `.claude/settings.json` hook entry | yes | capture hook (Claude Code) |
| `.pi/settings.json` | yes | pinned package ref (pi) |

Properties you can rely on:

- **Idempotent.** Running it twice equals running it once.
- **Never clobbers.** Your existing hooks are appended to, never overwritten.
  (Hook arrays concatenate across settings scopes in both Claude Code and Codex —
  verified — so appending is safe. We de-duplicate our own entry.)
- **Offline.** No network on the install path.
- **Worktree-aware.** All worktrees of a repo share memory; per-worktree state is
  kept separately.

### Teammates

Once the first person commits, a teammate does:

```bash
git pull
```

That's it for **delivery** — the compiled `AGENTS.md`/`CLAUDE.md` sections arrive
with the pull and every agent reads them immediately, with no jedimem installed at
all.

To also **capture**, they run `./jedimem install` once. For pi it is automatic: the
committed `.pi/settings.json` carries a pinned package ref, and pi installs missing
packages on startup after the project is trusted.

### Uninstall

```bash
./jedimem uninstall
```

Removes hooks and generated sections. **Leaves your memories** — they are your
team's files, not ours. `git status` is clean afterwards; this is checked in CI.

## 4. Daily use

Mostly you do nothing. Capture is automatic and invisible.

```bash
jedimem review          # approve/reject pending candidates, in a batch
jedimem status          # what's queued, what's compiled, budget used
jedimem why "<text>"    # which session, turn, and commit produced this memory
jedimem log             # what changed in memory, and why
jedimem contest <id>    # "this is wrong" — stops serving it, doesn't delete it
jedimem pause           # stop capturing (per repo, or globally)
jedimem compile         # regenerate AGENTS.md / CLAUDE.md sections
```

### The review gate

Candidates accumulate invisibly on a side git ref. They are never committed
automatically and never opened as a PR. `jedimem review` shows each with its
provenance and lets you approve, reject, or edit. Approvals land as **one commit**,
not one per memory.

**Rejections are kept.** They are the tuning corpus that stops the store filling
with junk — the failure mode that left mem0 with 10,134 entries of which 97.8% were
junk, with "no reject action" named as a root cause.

Reviewing is a batch chore, like triaging dependabot. Once a week is fine. Nothing
degrades if you ignore it; the queue just grows.

## 5. When a memory is wrong

This will happen. The workflow matters more than the accuracy.

```bash
jedimem why "use httpClient, not axios"     # where did this come from?
jedimem contest <id> "changed in v4"        # stop serving it
```

Contesting is not deleting. A contested memory stops being delivered, stays in the
repo, and shows in review with the objection attached. Deleting is not offered at
all — delete-vs-edit is a hard git conflict, and destroying a true fact is worse
than carrying a stale one.

To replace a memory, write the correction; the new memory supersedes the old one by
subject match. **Supersession is refused when the newer claim has weaker provenance
than the older one** — an agent's guess does not overrule a human's confirmed rule.

## 6. Conventions for a team

**Keep the always-on tier small.** It is hard-budgeted for a measured reason: more
memory makes agents *worse*. If the budget is full, something must be demoted
before something is added. Treat a repo-wide rule as expensive — most rules that
look global are really about one package.

**Scope everything you can.** Every memory takes a glob:

```yaml
scope: "packages/api/**"
```

An unscoped memory consumes every teammate's context budget on every task forever.

**Requirements and constraints need a human.** The taxonomy
([`MEMORY-KINDS.md`](MEMORY-KINDS.md)) marks `requirement`, `constraint`,
`decision`, `negative`, and `migration` as human-approval-only, because they encode
*intent and authority* — which a transcript does not contain. An agent can observe
that a command failed; it cannot know whether "don't call that API directly" is a
team rule, one reviewer's opinion, or a temporary workaround.

**Personal preferences stay personal.** They live in `.jedimem/local/` and are
never committed. Promoting one to a team convention is an explicit human act, and
should be a conversation.

**Add `CODEOWNERS` for `.jedimem/**`.** Memories are read into your agents' context,
which makes them instructions with persistence. Treat a memory diff like a code
diff.

## 7. Privacy, and the surveillance question

Someone on your team will ask whether this is monitoring them. It is a fair
question and the answer is structural, not a promise:

- **No memory may name a developer as its subject.** Enforced by the extraction
  schema, not by a prompt. Authorship exists in provenance metadata for
  traceability — so you can ask *"where did this rule come from"* — and never as
  content.
- **Nothing leaves your machine** except in a commit that a human makes.
- **No telemetry.** None.
- **Personal preferences are local and untracked** by default.
- **`jedimem pause` works**, immediately, per repo or globally.
- **`jedimem status`** shows exactly what is queued, so capture is inspectable
  rather than ambient.

jedimem stores facts about **code**, not about people. If you find a memory that
describes a person, that is a bug — please report it.

## 8. Monorepos

Memories carry scope globs and the compiler emits nested instruction files, so a
memory scoped to `packages/api/**` never reaches an agent working in
`packages/web/`. This follows the precedent both Codex (nested `AGENTS.md` applying
within its subtree) and Cursor-style rule targeting already set.

## 9. CI

Add two checks:

```bash
jedimem compile --check     # fails if compiled files are stale
jedimem lint                # fails on schema-invalid or unscoped-and-oversized memories
```

The first matters because compiled files are committed generated artifacts — the
same contract as any generated code. Without the check they drift, and a drifted
instruction file is worse than none, because it is confidently wrong.

## 10. Security posture

Read [`SECURITY.md`](../SECURITY.md) before adopting. The short version of what you
are accepting:

- jedimem ships **executable content in your repo** (a capture hook). A hostile PR
  that edits it runs on the next teammate's session. Mitigations: content-hash
  pinned hook trust (the model Codex already ships), `CODEOWNERS`, review. **Residual
  risk is real** — a teammate who approves the PR defeats all of it.
- **Memory files are read into agent context, so a malicious memory is an injected
  instruction with persistence.** This is unsolved industry-wide. Our mitigations
  are the human review gate and a schema in which memories are data that can never
  grant capabilities.
- **A secret captured into a committed memory is in git history forever.** There is
  redaction before write, but the only true remedy is history rewrite. This is why
  redaction runs before *anything* is written, including to the staging ref.

We would rather you decline on an informed basis than adopt on an uninformed one.

## 11. FAQ

**Do I need an API key?** No. The default extraction runtime borrows the login your
coding agent already has. A key is opt-in, and only for people who want the ~30x
cheaper direct-API path.

**What does it cost?** ~$0.17/day at ~120 human turns/day, batched. Per-turn
extraction would be $2.52/day for the same work, which is why it is batched.

**Will it slow my agent down?** Capture is 13 ms and asynchronous. Delivery is a
static file, so it costs nothing at runtime — only context budget.

**What if the daemon crashes?** Nothing happens. Every failure path ends in no
injection and `exit 0`. Sessions are unaffected; capture resumes from a durable
checkpoint.

**Does it work with worktrees?** Yes. Worktrees of a repo share memory; per-worktree
state is separate.

**What if two of us capture the same fact?** Memory files are content-addressed for
dedup detection, so identical facts converge instead of duplicating.

**What if two of us capture memories at the same time?** Nothing conflicts.
One-file-per-memory merges cleanly, and the staging layer is lock-free — verified
with 20 concurrent writers and zero loss.

**Can I just write memories by hand?** Yes, and you should. They are plain markdown
files with frontmatter. jedimem is a capture and compile pipeline, not a gatekeeper.

**Does this replace `AGENTS.md`?** No — it generates a section of it. Your
hand-written content stays, outside the delimiters, and always wins on conflict.
