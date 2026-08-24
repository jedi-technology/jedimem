# The git substrate: measured behavior, and what it forces

Every claim here is **VERIFIED** by a real `git` experiment run on this machine
(git 2.x, Linux), with commands and outputs reproduced. Where I could not test
something locally it is labelled **INFERRED**.

## Executive summary — the decisions this research forces

1. **One markdown file with appended entries does not work.** Two teammates
   appending produces a merge conflict every time.
2. **`merge=union` fixes the conflict and introduces a worse bug:** it silently
   **resurrects facts you deleted or corrected**. Unusable for mutable memory.
3. **One file per memory merges cleanly** — the correct primitive.
4. **Filenames must be collision-free without coordination**, because same-name
   different-content is an `add/add` conflict.
5. **Content-addressed filenames give free cross-machine dedup**: two developers
   who independently capture the same convention produce byte-identical files
   that git merges silently.
6. **Never delete a memory file** — delete-vs-edit is a `modify/delete` conflict.
   Supersede in place instead.
7. **Scale is a non-issue.** 10,000 memory files → `git status` in 0.01 s.
8. **A daemon must never run `git add`/`git commit`.** 20 concurrent commits
   silently lost 13 of 20 memories to `index.lock`.
9. **Even a correct lock-free commit to the checked-out branch is unsafe** — the
   user's next `git commit` uses a stale index and silently reverts it.
10. **Commit to a side ref instead.** Verified: 20 concurrent writers, 0 lost,
    user's HEAD, index, and working tree completely untouched.
11. Worktrees: `--git-common-dir` is the shared identity; `info/exclude` is
    shared across all worktrees, making it the right home for per-user config.
12. **The root-commit hash is not a stable repo ID** — it changes under shallow
    clone.

---

## Experiment 1 — plain append to a shared file

```
$ # two branches each append one line to mem.md, then merge
$ git merge bob
Auto-merging mem.md
CONFLICT (content): Merge conflict in mem.md
```
```
header
<<<<<<< HEAD
- alice memory
=======
- bob memory
>>>>>>> bob
```

**VERDICT: conflicts.** Both sides appended at the same location, so git sees
overlapping edits. This is the naive design and it fails immediately — with
automated writers it would fail many times a day, and the person paying the cost
is whoever runs `git pull` next.

## Experiment 2 — `merge=union`

`.gitattributes`: `mem.md merge=union`

```
$ git merge bob     →  Auto-merging mem.md   (CLEAN)
header
- alice memory
- bob memory
```

Rebase too:
```
$ git rebase alice  →  RESULT: REBASE CLEAN
```

**VERDICT: solves the conflict.** So far this looks like the answer. It is not.

### Experiment 2c — the failure that rules it out

Alice *corrects* a wrong memory (edits the line). Bob *appends* an unrelated
memory. Both with `merge=union`:

```
$ git merge bob     →  CLEAN
- old fact CORRECTED
- old fact that is wrong        ← THE WRONG FACT IS BACK
- bob memory
```

**VERDICT: unusable.** Union merge takes the union of *lines*, so it cannot
express deletion or modification — every correction is silently undone by the
next merge from anyone who hadn't pulled it. A memory system whose corrections
spontaneously revert is worse than no memory system, because the agent now
confidently asserts a fact a human explicitly fixed.

This generalizes: **union merge is only safe for a strictly append-only log whose
entries are never edited or removed.** Memory is not that. (It *is* a reasonable
fit for an immutable audit/event log, and that is the one place jedimem may use
it.)

**INFERRED, and important:** `.gitattributes` merge drivers are applied by the
*client*. GitHub's server-side "Merge pull request" does not run them, so a
design that depends on `merge=union` also silently changes behavior between local
merges and web merges. Another reason not to build on it.

## Experiment 3 — one file per memory

```
alice: .jedimem/memories/01J000A.md
bob:   .jedimem/memories/01J000B.md
$ git merge bob  →  MERGE CLEAN
files: 01J000A.md 01J000B.md
```

**VERDICT: clean.** Distinct paths are independent to git; there is nothing to
conflict over. This is the primitive to build on.

### 3b — same filename, different content

```
$ git merge bob
CONFLICT (add/add): Merge conflict in .jedimem/memories/SAME.md
```

**VERDICT: filename collisions are conflicts.** IDs must therefore be generated
collision-free *without coordination* between machines — ULID/UUIDv7 (time +
machine entropy) or a content hash. A naive counter or a short random suffix is a
latent conflict generator.

### 3d — content-addressed filenames

Both developers independently capture the same convention; filename is
`sha256(content)[:12]`:

```
$ git merge bob  →  MERGE CLEAN (identical content dedupes)
```

**VERDICT: this is a genuinely valuable property.** Two people whose agents
learn the same fact converge on the *same file with the same bytes*, and git
merges it to one copy with no dedup logic anywhere in jedimem. Cross-machine
deduplication falls out of the substrate for free.

The tradeoff: a content-addressed name changes when the content is edited, so an
edit becomes delete+add — which experiment 3c shows is a conflict. Resolution:
**a stable ULID identity in the filename, plus a content hash recorded inside the
file** for dedup detection. Get convergence without making edits destructive.

### 3c — delete vs edit

Alice deletes a memory; Bob edits it:
```
CONFLICT (modify/delete): .jedimem/memories/M.md deleted in HEAD
and modified in bob.
```

**VERDICT: never delete.** Retirement must be a field change inside the file
(`status: superseded`, `superseded_by: <id>`), never an unlink. This independently
re-derives the soft-invalidation rule from the prior mem0/Graphiti research —
there it was needed to avoid destroying true facts; here git enforces it anyway.

## Experiment 4 — scale

Real `git` timings against a repo of small memory files with frontmatter:

| Memories | `git status` | `git add -A` | `git checkout` |
|---|---|---|---|
| 1,000 | 0.00 s | 0.00 s | — |
| 10,000 | 0.01 s | 0.02 s | 0.19 s |
| 50,000 | 0.09 s | 2.45 s | — |

`.git` size at 50,000 memories: **167 MB loose → 11 MB after `git gc`**.

**VERDICT: scale is not a design constraint.** A realistic repo holds hundreds to
low thousands of memories; we have two orders of magnitude of headroom. The one
real note is that many small loose objects inflate `.git` ~15x until packed, so a
periodic `git gc` (or letting git's automatic gc do its job) matters more than
file count does.

This kills the case for a committed index/manifest file *for performance
reasons*. Which is fortunate, because a committed index would conflict on every
concurrent write — the correct answer is to not commit derived indexes at all and
rebuild them locally.

## Experiment 5 — repo identity across machines

```
$ git rev-list --max-parents=0 HEAD          # root commit
2d28d572ddb863e0601189d783b95ad79ad25f5a

$ git clone --depth 1 file:///tmp/jmg/e5 e5shallow
$ git rev-list --max-parents=0 HEAD
36d4ab03abf2f487911ed397205860bd75fb5ea5    ← DIFFERENT
```

**VERDICT: the root-commit hash is not a reliable repo ID.** Under a shallow
clone the graft point becomes the root, so the "stable" identity changes. It also
breaks on repos with multiple root commits (merged histories) and tells you
nothing about forks — a fork has the *same* root commit and is emphatically not
the same memory scope.

Remote URLs are no better on their own:
```
ssh form:   git@github.com:jedi-technology/jedimem.git
https form: https://github.com/jedi-technology/jedimem
no remote:  error: No such remote 'origin'
```
Same repo, three different strings, one of which does not exist.

**Recommended identity function**, in precedence order:

1. An explicit `repo_id` (a generated ULID) **committed in the repo's own
   jedimem config**. Authoritative, survives shallow clones, renames, host
   migrations, and mirrors — and correctly makes a fork a *distinct* scope only
   when its owner regenerates the ID.
2. Fall back to a **normalized remote URL** (strip scheme, user, trailing `.git`,
   lowercase host) when no `repo_id` exists yet.
3. Fall back to the root-commit hash only for a repo with no remote.
4. Never fall back to the filesystem path — worktrees and clones break it.

The lesson is the same one the transcript-directory encoding taught: **do not
derive identity from something lossy when you can just write it down.**

## Experiment 6 — worktrees

Three worktrees of one repo:

| Location | `--git-dir` | `--git-common-dir` | root commit |
|---|---|---|---|
| `/tmp/jmg/f6` | `.git` | `.git` | `16fb2827…` |
| `/tmp/jmg/wt2` | `…/f6/.git/worktrees/wt2` | `…/f6/.git` | `16fb2827…` |
| `/tmp/jmg/wt3` | `…/f6/.git/worktrees/wt3` | `…/f6/.git` | `16fb2827…` |

```
$ git worktree list --porcelain
worktree /tmp/jmg/f6     HEAD 16fb2827…  branch refs/heads/main
worktree /tmp/jmg/wt2    HEAD 16fb2827…  branch refs/heads/feat2
worktree /tmp/jmg/wt3    HEAD 16fb2827…  branch refs/heads/feat3
```

**VERDICTS:**
- `--git-common-dir` is identical across worktrees → **the natural key for
  "same repo, same memory scope"** on one machine.
- `--git-dir` is unique per worktree → **the natural home for per-worktree state**
  (checkpoints, offsets, queues).
- `git worktree list --porcelain` enumerates them all → the daemon can discover
  every worktree from any one of them.
- `info/exclude` resolves into the **common** dir, i.e. it is **shared by all
  worktrees**. So per-user untracked patterns need to be written once, not once
  per worktree.

## Experiment 7 — concurrent writes (the important one)

### 7a — the naive daemon: `git add` + `git commit`

20 concurrent writers, each adding one memory file:

```
commits landed: 2
memory files on disk: 20
files actually COMMITTED: 7
   17 x fatal: Unable to create '.git/index.lock': File exists.
```

**VERDICT: 13 of 20 memories silently lost.** `.git/index.lock` is not a queue —
it fails immediately. The files remained on disk as untracked, so nothing looked
broken; the memories simply never reached git. In real use the competing process
is the developer's own `git commit`, their IDE's `git status`, or a pre-commit
hook, so this is the *normal* case, not a stress test.

Worse, the error text git prints is:
> Another git process seems to be running… remove the file manually to continue.

A background daemon that provokes that message has made the user's git look
broken. **This alone disqualifies porcelain commits from a daemon.**

### 7b — lock-free commit to the checked-out branch

Build the tree with plumbing against a private index (`GIT_INDEX_FILE`), then
compare-and-swap the branch ref:

```sh
parent=$(git rev-parse HEAD)
idx="$(mktemp -u …)"                      # NAME ONLY — git rejects an empty index file
GIT_INDEX_FILE="$idx" git read-tree "$parent"
GIT_INDEX_FILE="$idx" git update-index --add --cacheinfo 100644,"$blob","$path"
tree=$(GIT_INDEX_FILE="$idx" git write-tree)
commit=$(git commit-tree "$tree" -p "$parent" -m "…")
git update-ref "$branch" "$commit" "$parent"   # CAS; retry on failure
```

```
commits landed:   21
memories in HEAD: 20      ← zero lost
```

**VERDICT: correct, and still unsafe.** Two traps found the hard way:

1. **Read the parent *before* building the tree.** Building the tree first and
   reading HEAD second passes the CAS while committing a stale tree — a lost
   update that silently drops other writers' memories. My first implementation
   had this bug and dropped 11 of 20 files while reporting success.
2. **`mktemp` is wrong for an index path.** It creates a zero-byte file and git
   fails with `index file smaller than expected`. Use `mktemp -u`.

But the disqualifying result came from running it alongside a human:

```
=== 20 side commits, then the human runs `git commit` ===
memories in HEAD after human's commit: 10   ← the other 20 were reverted
```

The user's `git commit` commits *their index*, which still reflects the older
HEAD. Their perfectly ordinary commit **silently reverted every memory the daemon
had committed**, and left their working tree showing spurious `D` (deleted)
entries.

**Conclusion: never advance the checked-out branch behind a user's back.** It
desynchronizes their index, and their next commit quietly undoes your work.

### 7c — the side ref (recommended)

Same plumbing, but CAS a ref *outside* `refs/heads/`, e.g. `refs/jedimem/log`,
and never write to the working tree:

```
=== 20 concurrent side-ref commits WHILE a human commits ===
--- USER'S VIEW ---
  HEAD commits:         2        [expect 2]        ✓
  worktree status:      ''       [expect empty]    ✓
  human.txt in HEAD:    1                          ✓
  .jedimem in worktree: none                       ✓
--- JEDIMEM'S VIEW ---
  side-ref commits:     20
  memories stored:      20       [expect 20]       ✓
  read one back:        memory 7                   ✓
  survives gc:          20                         ✓
```

**VERDICT: this is the design.** Properties, all verified:

- **Perfect isolation.** No index, no HEAD, no working-tree file. `git status` is
  empty; the user cannot tell it is happening.
- **No loss under concurrency.** 20 simultaneous writers, 20 memories stored.
- **Survives `git gc --prune=now`** — a ref is a real GC root, unlike a dangling
  object.
- **Pushable and fetchable** as an ordinary ref
  (`git push origin refs/jedimem/log`), so cross-machine sync needs no server.
- **Readable without checkout**: `git show refs/jedimem/log:memories/m7.md`.

This gives capture and review a clean separation: the daemon appends to the side
ref continuously and invisibly; a human-initiated `jedimem sync` materializes
approved memories into the working tree as one normal, reviewable commit. The
agent never surprises anyone, and nothing is lost while waiting for review.

### 7d — can the side ref carry memories between teammates? No.

```
$ git push origin 'refs/jedimem/log:refs/jedimem/log'
 * [new reference]   refs/jedimem/log -> refs/jedimem/log

$ git ls-remote origin
4df1ec25…  refs/heads/main
11a200b7…  refs/jedimem/log          ← accepted by the remote

$ git clone <remote> fresh && cd fresh
$ git for-each-ref refs/jedimem
                                    ← EMPTY. Nothing fetched.
$ git fetch origin 'refs/jedimem/*:refs/jedimem/*'
  3 memories                        ← only with an explicit refspec
```

**VERDICT: the side ref is push/fetchable but invisible by default.** A teammate
who clones the repo normally gets **zero memories**. `git clone` and `git fetch`
only follow the default refspec (`refs/heads/*`), so anything under
`refs/jedimem/*` requires every teammate to configure an extra refspec — which
is precisely the kind of setup step that makes a tool fail to spread.

**This settles the architecture into two layers, each doing what it is actually
good at:**

| Layer | Ref / path | Role | Visible to teammates |
|---|---|---|---|
| **Capture staging** | `refs/jedimem/log` | lock-free, lossless, invisible accumulation | no |
| **Shared memory** | `.jedimem/memories/*.md` on the branch | reviewed, committed, diffable | **yes, automatically** |

The daemon writes only to the staging layer, where it can never conflict with a
human or lose a memory. Promotion into the shared layer is a normal file commit —
which is also exactly where a human review gate belongs. The thing that makes
sharing work is the thing that makes review necessary, and vice versa.

(Verified against a local bare remote. Behavior on GitHub specifically is
INFERRED from the protocol semantics; GitHub is known to accept custom refs such
as `refs/notes/*`, and the default-refspec behavior is client-side so it holds
regardless. Worth one confirmation against the real remote before release.)

## Recommended layout

```
<repo>/
  .jedimem/
    config.yml              committed: repo_id, budgets, enabled kinds
    memories/
      01J8X....md           one file per memory, ULID name, never deleted
    compiled/               GENERATED, committed, CI-checked for staleness
      AGENTS.md.part
      CLAUDE.md.part
    local/                  NEVER committed (see below)
      state.db              offsets, queues, per-machine identity
      config.yml            per-user overrides, model choice, keys
  AGENTS.md                 generated section, delimited by markers
  CLAUDE.md                 generated section, delimited by markers
```

Refs:
```
refs/jedimem/log            append-only capture log (daemon writes here)
refs/jedimem/pending/<id>   proposals awaiting human review
```

Per-worktree state lives under `$(git rev-parse --git-dir)/jedimem/`; shared
per-repo state under `$(git rev-parse --git-common-dir)/jedimem/`.

### A memory file

```markdown
---
id: 01J8XQ2K7ZVN4P3M9WQZ8YT6RA
format: 1
kind: convention
scope: "packages/api/**"
status: active            # active | superseded | contested
content_hash: 3f9a1c8e2b04
provenance:
  source: session
  agent: claude-code
  session: 3803e5d3-7194-4f6b-bde4-93df38d378ef
  turn: 41
  commit: 9a1f2c3
  captured_at: 2026-08-24T06:31:00Z
  confirmed_by: human      # human | agent
supersedes: []
---
Use the internal `httpClient` wrapper for outbound HTTP, not `axios`.

**Why:** axios bypasses the retry and tracing middleware.
```

`.gitignore` (committed) covers `.jedimem/local/`. Per-*user* patterns that
should not be imposed on teammates go in
`$(git rev-parse --git-common-dir)/info/exclude` instead — verified above to be
shared across all worktrees and, by design, never committed.

## Failure modes to design against

| Failure | Cause | Mitigation |
|---|---|---|
| Corrections silently revert | `merge=union` | never use it for memories |
| Memories silently not committed | `index.lock` | side ref + plumbing, never porcelain |
| Daemon commits reverted | user's stale index | never advance `refs/heads/*` |
| add/add conflict | colliding filenames | ULID/UUIDv7 or content hash |
| modify/delete conflict | deleting a memory | supersede, never unlink |
| Repo identity drift | shallow clone / URL forms | committed `repo_id` |
| Secrets immortal in history | any committed secret | pre-write redaction; deletion needs history rewrite |
| CRLF churn | `core.autocrlf` | `.gitattributes`: `*.md text eol=lf` for `.jedimem/**` |
| `.git` bloat | many loose objects | 15x until `git gc`; let auto-gc run |

## Unverified / open

- Whether GitHub's server-side merge honors `.gitattributes` merge drivers
  (INFERRED: it does not). Moot given we reject union merge.
- Whether GitHub specifically accepts `refs/jedimem/*` pushes. Verified against a
  local bare remote (accepted); GitHub accepts `refs/notes/*`, so this is likely,
  but confirm once against the real remote. Not load-bearing any more: experiment
  7d showed the side ref cannot be the sharing mechanism regardless, so a refusal
  would only make it local-only — which is all we now rely on it for.
- `core.autocrlf` behavior on Windows was not tested (no Windows host here).
- Submodules and bare repos were not tested.
