# Claude Code transcript format — reverse-engineered spec

> **Ported research.** Produced for this project's predecessor (`auto-memory`)
> and carried over unchanged: the reverse-engineering is version-specific to
> Claude Code 2.1.241 and underpins jedimem's capture design. Cited by
> [`05-distribution-security.md`](05-distribution-security.md).

> Renamed from `02-transcript-format.md` (slot 02 is Codex here).

Ground truth: `claude` **v2.1.241** (`BUILD_TIME 2026-08-22T22:46:48Z`,
`GIT_SHA c87e2742fc9ad269ec8920460d00a091b1e410f0`), native install at
`~/.local/share/claude/versions/2.1.241` — a 342 MB Bun-compiled ELF that is **not
stripped**, so JS source is recoverable from ~byte 250 MB onward. The enums and
functions below are quoted from that bundle, not guessed.

## 1. On-disk layout

```
~/.claude/
  projects/<ENCODED_CWD>/
      <sessionId>.jsonl                  # main conversation (append-MOSTLY)
      <sessionId>/
          subagents/
              agent-<agentId>.jsonl      # ONE FILE PER SUBAGENT (isSidechain:true)
              agent-<agentId>.meta.json  # join metadata (NOT jsonl)
              workflows/<runId>.json
          tool-results/                  # SPILLED large tool payloads
              toolu_<id>.txt
          workflows/<runId>.json
      memory/
  history.jsonl          # global prompt history {display,timestamp,project,sessionId}
  sessions/<pid>.json    # LIVE process registry -> daemon discovery
  session-env/<sessionId>/
~/.claude.json           # identity + per-project state (NOT under projects/)
```

### The subagent split is the most important structural fact

In v2.1.x, sidechains are **not inline** in the parent file. Measured on one live
session:

```
main = 506,341 B    subagents = 15,774,307 B    tool-results/ = 12 MB
```

**Subagent files are 96.9% of transcript bytes.** A reader that only tails
`<sessionId>.jsonl` sees ~3% of the conversation.

Subagent files stay *flat* in `subagents/` even at `spawnDepth: 3`; the tree lives
in `meta.json.parentAgentId`. The code path `agentTranscriptSubdirs` can nest them,
so **recurse** — handle both.

## 2. Project-directory encoding — exact, and LOSSY

Verbatim from the bundle:

```js
function h1r(e){ return e.replace(/[^a-zA-Z0-9]/g,"-") }   // sanitize
var Pae = 200;                                              // max length
function lX_(e){ return Math.abs(zft(e)).toString(36) }
function zft(e){ let t=0; for(let r=0;r<e.length;r++) t=(t<<5)-t+e.charCodeAt(r)|0; return t }
function qY(e){ let t=h1r(e); if(t.length<=Pae) return t; return `${t.slice(0,Pae)}-${lX_(e)}` }
```

Rules that contradict common assumptions:

- **Every** non-alphanumeric becomes `-`, including `.` `_` space `~` `/`.
  So `.config` -> `-config`, `my_app` -> `my-app`.
- **Uppercase is preserved** (`a-zA-Z0-9` is the keep-set).
- **Not reversible.** `/a/b`, `/a_b`, `/a.b` all encode to `-a-b`. Two *different*
  projects can share one `projects/` directory.
  => **Never derive `cwd` by decoding the directory name.** Read `cwd` from a line
  inside the file — the CLI itself does exactly this (`g1r(head,"cwd")`).
- Over 200 chars: truncate to 200 + `-<base36 hash of the original>`.
- A `{"type":"relocated","relocatedCwd":...}` line is appended when cwd changes
  mid-session. The CLI resolves current cwd as
  `smt(tail,"relocated","relocatedCwd") ?? g1r(head,"cwd")` — **last `relocated`
  wins, else first `cwd`**.

## 3. Line-type registry — 36 types, not 10

Extracted from the resume/rebuild dispatch table (`IwE`). One session exercises ~10;
the authoritative set is 36, grouped by resume disposition:

| Disposition | Types |
|---|---|
| `transcript` | `user`, `assistant`, `system`, `attachment` |
| `boundary-cleared` (dropped at a compact boundary) | `progress`, `file-history-snapshot`, `file-history-delta`, `last-prompt`, `marble-origami-{commit,snapshot,reset}` |
| `accumulate` | `content-replacement`, `fork-context-ref`, `frame-link`, `artifact-comment-monitor` |
| `last-wins` (append-only latches) | `summary`, `custom-title`, `ended-by-model`, `ai-title`, `tag`, `relocated`, `agent-name`, `agent-color`, `agent-setting`, `pr-link`, `artifact-autoreact-ledger`, `bridge-session`, `history-suppression`, `attribution-snapshot`, `mode`, `permission-mode`, `isolation-latch`, `atis-latch`, `worktree-state`, `queue-operation`, `observer-ref` |

A companion table (`luh`) marks `user/assistant/attachment/system/progress` as
`"dedup-transcript"` and everything else `"always"` — i.e. only the conversation
types are deduped on resume; **latches are re-appended unconditionally**, which is
why repeated `mode`/`atis-latch`/`ai-title` lines appear.

## 4. Envelope

`user` / `assistant` / `attachment` / `system` all carry: `type`, `uuid`,
`parentUuid`, `timestamp`, `sessionId`, `isSidechain`, `cwd`, `gitBranch`,
`version`, `userType`, `entrypoint`, `slug`, and optionally `session_id`,
`agentId`, `attributionAgent`, `effort`.

- **`sessionId` vs `session_id` are different producers.** `sessionId` (camel) is
  written by the local transcript writer on *every* line. `session_id` (snake)
  appears on a subset and leaks in from the **SDK/stream message shape** — the
  bundle emits `session_id: qt()` alongside `parent_tool_use_id`, `tool_use_meta`.
  **Always key off `sessionId`; treat `session_id` as advisory.**
- **`gitBranch` is `"HEAD"` when cwd is not a git repo** — 100% of lines on the
  probe machine. Treat `"HEAD"` as null.
- `isSidechain` is exactly a mirror of file location, not independent information.

## 5. THE critical modelling fact: one API message spans N lines

**`message.content` is ALWAYS an array of length exactly 1** (measured: `{1: 1598}`
across every assistant line on the machine). One Anthropic API message is split
across N JSONL lines, one per content block, sharing `message.id` and `requestId`,
chained parent->child:

```
uuid d17c57cd  parent 2d7ae522  ['thinking']   out_tok 578
uuid 24a4721b  parent d17c57cd  ['text']       out_tok 578
uuid e1b4e793  parent 24a4721b  ['tool_use']   out_tok 578
uuid 27e130d7  parent e1b4e793  ['tool_use']   out_tok 578
```

694 message-id groups observed, **542 multi-line**. Consequences:

1. To reconstruct an API turn, **group by `(file, message.id)`** and concatenate
   blocks in file order.
2. `usage` is repeated on every line of the group and **differs across lines in
   468/542 groups** (updated progressively during streaming).
   **Do not sum. Take the last line's usage per `message.id`.**

Block shapes: `text{type,text}`, `thinking{type,thinking,signature}`,
`tool_use{type,id,name,input,caller}`, `tool_result{tool_use_id,type,content,is_error?}`.
Also defined: `image`, `document`, `redacted_thinking`, `server_tool_use`,
`mcp_tool_use`, `grouped_tool_use`.

`usage.input_tokens` is typically **2** because everything else is cache hits — real
cost sits in `cache_creation_input_tokens`.

## 6. `toolUseResult` — the payoff finding

`toolUseResult` is a sibling of `message` on `user` lines, and it carries the
**full, untruncated** tool output *even when the model-visible `tool_result.content`
was truncated*. Proof from disk:

```
tool_result.content -> "<persisted-output>\nOutput too large (69.5KB). Full output
                        saved to: .../tool-results/toolu_013K….txt
                        Preview (first 2KB):…"
toolUseResult       -> {"bytes":71146,"code":200,"result":"<the ENTIRE 71 KB>",...}
```

**The daemon therefore has strictly more information than the model did.** Preview
cap is `sKr = 2000` chars.

Join to tool name via `tool_result.tool_use_id` -> `tool_use.id` -> `tool_use.name`.

| tool | max B | keys |
|---|---|---|
| Bash | 34,943 | `stdout, stderr, interrupted, isImage, noOutputExpected` (+`backgroundTaskId`, `persistedOutputPath`, `returnCodeInterpretation`) |
| WebFetch | **99,235** | `bytes, code, codeText, result, durationMs, url` |
| Read | 73,203 | `type`, `file{filePath, content, numLines, totalLines, truncatedByTokenCap?}` |
| Agent | 6,982 | `isAsync, status, agentId, description, resolvedModel, prompt, outputFile` |
| WebSearch | 3,929 | `query, results, durationSeconds, searchCount` |
| AskUserQuestion | 1,836 | `questions, answers, annotations` |

From the bundle's zod schemas (not exercised on the probe machine):

- **Edit** -> `{filePath, oldString, newString, originalFile (nullable, FULL pre-edit
  content), structuredPatch:[{oldStart,oldLines,newStart,newLines,lines[]}],
  userModified, replaceAll, gitDiff?{status,additions,deletions,patch,repository}}`.
  **`originalFile` can embed an entire file — a size bomb. Strip it, keep
  `structuredPatch`.**
- **Write** -> `{type:"create"|"update", filePath, content, structuredPatch}`
- **Grep** -> `{mode, numFiles, filenames[], content, numLines, totalLines}`
- **TodoWrite** -> `{oldTodos, newTodos}` — the model-visible text is a fixed ack
  string, so **the real todo state exists only in `toolUseResult`**.

`toolUseResult` is sometimes a **bare string** — always type-check before `.get()`.
Largest observed single line: **151,877 bytes**.

## 7. Attachments

Observed types by volume: `total_tokens_reminder`, `skill_listing` (avg 17,784 B —
**the single biggest noise source, 3.3% of all bytes**), `deferred_tools_delta`,
`hook_success` (`{hookName, toolUseID, hookEvent, content, stdout, stderr,
exitCode, command, durationMs}`), `agent_listing_delta`, `auto_mode`,
`hook_non_blocking_error`, `mcp_instructions_delta`, `plan_mode_exit`, `goal_status`.

Full enum from the bundle also includes: `file`, `new_file`, `edited_text_file`,
`edited_image_file`, `selected_lines_in_ide`, `opened_file_in_ide`, `todo`,
**`nested_memory`** (`{path, displayPath, content:{content}}`),
**`relevant_memories`** (`{memories:[{path,…}]}`), **`ultramemory`**,
`dynamic_skill`, `diagnostics`, `mcp_resource`, `plan_mode`, `command_permissions`,
`queued_command`, `output_style`, `compact_summary`, `read_truncation_notice`,
`background_task_status`, `hook_system_message`, `hook_blocking_error`,
`ide_selection`, `ide_opened_file`.

`nested_memory` / `relevant_memories` / `ultramemory` are the **CLAUDE.md-injection
attachments** — they tell us what guidance was already in context, which is directly
useful for deduping against our own store. The bundle's observer tap treats them as
first-class: `case"nested_memory": return [{type:"guidance_loaded", path:…}]`.

## 8. Compaction

Recorded as a `system` line. Verbatim constructor:

```js
function lOi(e){ return {
  type: "system", subtype: "compact_boundary",
  content: "Conversation compacted", level: "info",
  compactMetadata: aOi(e.compact_metadata),
  ...(e.logical_parent_uuid !== undefined && { logicalParentUuid: e.logical_parent_uuid }),
  uuid: e.uuid, timestamp: new Date().toISOString() } }
```

`compactMetadata`: `{trigger:"auto"|"manual", preTokens, postTokens,
cumulativeDroppedTokens, durationMs, userContext, messagesSummarized,
preservedSegment{headUuid,anchorUuid,tailUuid}, preservedMessages{anchorUuid,uuids[],allUuids[]}}`.

The summary itself is a `user` line flagged **`isCompactSummary: true`**. The CLI's
own fast-path scanner string-matches `'"isCompactSummary":true'` **and**
`'"isCompactSummary": true'` (both spacings) before parsing — mirror that tolerance.

**A post-compact session continues in the SAME FILE with the SAME `sessionId`.**
The boundary line is appended in place.

> For a memory daemon, **read linearly straight through the boundary** rather than
> applying `preservedSegment`. Compaction destroys exactly the context we are trying
> to capture, and we are not bound by the model's context window.

Other `system` subtypes: `turn_duration` (with `durationMs` + `messageCount` — a
free per-turn wall-clock and a cheap progress cursor), `local_command`,
`informational`, `model_refusal_fallback`, `error`, `warning`, `permission_denied`,
`tool_error`, `session_start`.

## 9. Linearizing the DAG

Measured across all 26 files: **0 duplicate uuids within a file, exactly 1 root per
file, 0 dangling parents, 0 parent-before-child violations.**

> **File order is already a valid topological order. You never need to sort.**

Forks *within* a file are real (10 in a 243-line session) but benign — every one has
shape `parent -> [assistant, user]`, which is the **parallel-tool-call pattern**: the
N block-lines of one message chain A->B->C, and each `tool_result` attaches to the
block-line holding its own `tool_use`. The "tree" is an artifact of block-splitting.

**True forks (edit / rewind / branch) create a NEW FILE, not a branch.** From the
branch writer:

```js
let P = {...I, cwd, userType, entrypoint, version, gitBranch,
         sessionId: o /* NEW sessionId */, timestamp: new Date().toISOString()};
let M = {...P, parentUuid: k, isSidechain: false};
```

**`uuid` is PRESERVED from the source messages; `sessionId` and `timestamp` are
rewritten.** Therefore:

> **`uuid` alone is NOT a global primary key. Use `(session_file_id, uuid)`.**
> A rewind duplicates content under a new `sessionId` with identical `uuid`s. Key on
> `uuid` alone and you silently drop the forked branch.

Detect a fork-derived file by the presence of `content-replacement` /
`fork-context-ref` lines, or by a `uuid` already known under a different `sessionId`.

## 10. Sidechain join chain (verified end-to-end)

```
parent assistant line
  uuid=85309454, content[0] = {type:"tool_use", id:"toolu_01LjQ…", name:"Agent",
                               input:{subagent_type, description}}
    |
    +- parent user line
    |    sourceToolAssistantUUID = 85309454        <- back-ref to the assistant LINE
    |    toolUseResult = {agentId:"a325a371330434033", status:"async_launched",
    |                     resolvedModel:"claude-opus-5[1m]", outputFile, prompt}
    |
    +- subagents/agent-a325a371330434033.meta.json
         {agentType, description, toolUseId:"toolu_01LjQ…",  <- matches tool_use.id
          spawnDepth:1, model:"opus", parentAgentId?}
       + subagents/agent-a325a371330434033.jsonl
```

Two independent join keys: `meta.json.toolUseId` <-> `tool_use.id` (primary, always
present) and `toolUseResult.agentId` <-> filename (secondary).
`sourceToolAssistantUUID` gives the exact parent *line*.

`agentId` = `a` + `randomBytes(8).hex` (17 chars), or `a<sanitized-name>-<8 hex>`.
Note `resolvedModel:"claude-opus-5[1m]"` — the model id **includes the
context-window variant tag**.

## 11. Append-only? Mostly — two real exceptions

**Evidence for append-only** (60 samples over 5 min on a live session): inode
constant, size strictly monotonic 79,163 -> 216,174, **head-64KB md5 identical in all
60 samples**. Writers use `O_APPEND` / `appendFileSync` with mode `0o600`.

**EXCEPTION 1 — tombstone removal rewrites in place.** `performRemoveByUuid`:

```js
let i = await lp.open(e,"r+");
let u = Math.min(c, Wx /*65536*/), d = c-u;      // read last 64 KB
let g = m.lastIndexOf(`"uuid":"${t}"`);
if (g >= 0) { … await i.truncate(w);              // <- TRUNCATE
              if (H>0) await i.write(m, S, H, w); return }
// fallback, only if size <= Lch (50 MB):
let l = a.filter(c => er(c).uuid !== t);
await lp.writeFile(e, l.join("\n"));              // <- FULL FILE REWRITE
```

A line can be deleted from the middle, shrinking the file and shifting every
subsequent byte. Triggered by message retraction (dequeued prompts, model-fallback
`retractedMessageUuids`).

> **A bare byte offset is NOT a safe resume checkpoint** — only safe when validated
> against a size + head-hash fingerprint that catches a shrink or rewrite.

**EXCEPTION 2 — orphan rotation renames the file.**
`<sessionId>.jsonl` -> `<sessionId>.orphaned-<ms>-<8hex>.jsonl`. The watcher must not
lose the tail, and must not re-ingest it as a new session (same `sessionId` inside).

Also: `wst()`/`Est()` call `lp.utimes(t, now, now)` on resume — **mtime is bumped
with no content change. Never use mtime alone as a change signal.**

## 12. Torn tails are a first-class state

The writer has explicit repair machinery:

```js
function PEE(e,t){ let r=Buffer.alloc(1);
                   return readSync(e,r,0,1,t-1)===1 && r[0]===10 }   // ends in \n?
function bjl(e,t,r){ let s = !PEE(o,i);                              // torn?
                     let a = Buffer.from(s ? "\n"+Re(t)+"\n" : n,"utf8");  // PREPEND \n
                     while(c<a.length){ let u=writeSync(o,a,c,a.length-c,l);
                                        if(u<=0) throw Error("short write"); c+=u } }
```

After a crash mid-write the file ends in a partial JSON line with no newline. On the
next append the writer **prepends `\n`**, sealing the fragment as its own line —
which stays in the file forever as an **unparseable line in the middle**.

> A tailer must (1) consume only up to the last `\n`, buffering trailing partial
> bytes, and (2) skip unparseable lines **anywhere**, not just at EOF.

`createWriteStream` has a 64 KB `highWaterMark` and observed lines reach 151,877 B,
so a >64 KB line can split across syscalls. CLI's own guards: `nX_=4194304`,
`tEE=16777216`.

Encoding: UTF-8, `\n` only, mode `0o600`. Over 4,147 lines: 0 non-UTF-8, 0 BOMs,
0 CRLF, 0 lone surrogates. Read as **bytes**, split on `b'\n'`, decode per line with
`errors='replace'` so one bad line cannot kill a batch.

## 13. Lifecycle event effects

| Event | Effect on disk |
|---|---|
| normal turn | append to `<sessionId>.jsonl` |
| `--resume` / `--continue` | **same file, same sessionId**; `utimes(now)` then re-appends latch lines (expect duplicate latches) |
| `/clear` | current file finalized; **new sessionId -> new file** |
| compaction | **same file, same sessionId**; appends `compact_boundary` + `isCompactSummary` |
| rewind / edit / branch | **NEW file, NEW sessionId, `uuid`s copied** + `content-replacement` |
| `cd` mid-session | appends `relocated` |
| crash | torn tail, sealed with a leading `\n` on next append |
| abandoned | renamed `<sessionId>.orphaned-<ms>-<hex>.jsonl` |

## 14. Volume and signal (measured: 13,088,981 B / 3,158 lines over 26 files)

By line type: `user` 68.48% (avg 8,075 B), `assistant` 25.65%, `attachment` 4.87%,
`queue-operation` 0.86%, `system` 0.05%, all latches + snapshots 0.09%.

By signal bucket:

| bucket | bytes | % |
|---|---|---|
| `LO:tool_result/Bash` | 6,026,513 | **46.04** |
| `MED:tool_use_input` | 1,892,071 | 14.46 |
| `LO:tool_result/WebFetch` | 1,627,473 | 12.43 |
| **`HI:assistant_thinking`** | 1,229,919 | **9.40** |
| `LO:tool_result/WebSearch` | 703,902 | 5.38 |
| `LO:attachment/skill_listing` | 426,829 | 3.26 |
| **`HI:assistant_text`** | 235,155 | **1.80** |
| **`HI:user_prompt`** (29 lines!) | 94,292 | **0.72** |

```
HIGH (user prompts + assistant text + thinking)  = 11.91%  -> 88.09% reduction
HIGH + MED (+ tool_use inputs, system)           = 26.42%  -> 73.58% reduction
LOW  (tool results, attachments, latches)        = 73.58%
```

Tokens (deduped by `message.id`, 802 API messages): `input=2,983`,
`cache_creation=2,185,826`, `cache_read=59,512,329`, `output=409,706`
(`thinking=117,198`). **`cache_read` dwarfs everything — the transcript is a replay
log of a cached context, so byte volume massively overstates unique information.**

### Filtering recommendation

The headline number: **29 user prompts produced 16 MB of transcript.**

1. **Drop entirely** (zero durable-information loss): all latch types,
   `file-history-{snapshot,delta}`, `progress`, and attachments in
   {`skill_listing`, `deferred_tools_delta`, `agent_listing_delta`,
   `total_tokens_reminder`, `mcp_instructions_delta`, `auto_mode`,
   `plan_mode_exit`, `goal_status`}. -> **-9.0%**
2. **Digest `tool_result` bodies** to
   `{tool_name, exit_ok, byte_len, head_512, tail_512, file_paths_touched}`.
   -> **-66%**, losing almost nothing durable: Bash stdout and WebFetch bodies are
   transient facts, not memories.
3. **Keep verbatim**: `origin.kind=="human"` user prompts, assistant `text`,
   assistant `thinking`, `tool_use.input` for mutating tools (Edit/Write/Bash),
   `AskUserQuestion.answers` (**explicit stated preferences — gold**),
   `TodoWrite.newTodos`, `compact_boundary`, `ai-title`, `summary`.
4. **Strip `Edit.originalFile`**, keep `structuredPatch`.
5. **Subagent policy**: subagents are 96.9% of bytes, but each one's *final report*
   is already in the parent's `Agent` `toolUseResult`. Default to ingesting the
   parent session + each subagent's final assistant text, skipping subagent tool
   churn.

Net: HIGH+digest is **~15-20% of raw bytes (~85% reduction)**; with the subagent
policy, a fan-out-heavy session goes **16.3 MB -> ~400 KB (>97%)**.

**Do not re-read `tool-results/*.txt` sidecars** — identical content is already
inline in `toolUseResult`. That was 12 MB of pure duplication on the probe machine.

## 15. Identity

| Identity | Source | Stability |
|---|---|---|
| **User (canonical, cross-machine)** | `~/.claude.json` -> `oauthAccount.accountUuid` | **the correct global user key** |
| email / name / org | `oauthAccount.{emailAddress,fullName,organizationUuid,seatTier}` | stable |
| **Machine** | `~/.claude.json` -> `machineID`; also `/etc/machine-id` | per-install |
| **Project** | `cwd` from transcript lines (**not** the decoded dir name) + `relocated` | |
| repo | `git remote get-url origin`; **also `~/.claude.json -> githubRepoPaths`**, e.g. `{"org/repo": ["/home/u/projects/repo"]}` | **best cross-machine project key** |
| branch | `gitBranch` — `"HEAD"` when not a repo | unreliable |
| **Agent** | `version`, `entrypoint`, `message.model`, `toolUseResult.resolvedModel`, `effort`, `attributionAgent` | |
| **Session** | `sessionId` + `slug` + `ai-title` | survives resume & compaction; **changes on `/clear` and rewind** |
| live? | `~/.claude/sessions/<pid>.json` -> `{pid, sessionId, cwd, status, name, messagingSocketPath}` | **use for liveness** |

```
session_key = sha256(f"{account_uuid}|{machine_id}|{project_key}|{session_id}")
project_key = git_remote_normalized  or  f"local:{machine_id}:{cwd}"
```

Collision risks:

1. **Two machines, same cwd path** (e.g. 50 EC2 boxes at
   `/home/dev/projects/x`) -> identical `projects/` dir name. `sessionId` is
   uuid4 so filenames won't collide, but `project_key` will. Always include
   `machine_id` unless a git remote is available; prefer the git remote — that is
   exactly what `githubRepoPaths` is for.
2. **Encoding collisions** (§2) — never use the dir name as project identity.
3. **`uuid` copied across fork files** (§9) — PK on `(session_id, uuid)`.
4. `~/.claude/sessions/` is keyed by **pid**, which the OS reuses — pair with
   `procStart` before trusting it.

**Privacy:** honour `history-suppression` (`{sessionId, cause,
vetoedAgainstAccountUuid}`) and skip those sessions. Never read
`~/.claude/.credentials.json` or `sessions/*.key`. Transcripts routinely contain
secrets in Bash stdout — redaction before shipping is mandatory, not optional.

## 16. Things that will bite you, ranked

1. **Subagents are 96.9% of the data and live in separate files.** Tailing only
   `<sessionId>.jsonl` gets you 3%.
2. **`content` is one block per line** — a naive reader treats each assistant line
   as a whole turn and double-counts `usage` in 542 of 694 groups.
3. **Byte offsets can be invalidated** by `performRemoveByUuid` (truncate-tail, or
   full rewrite under 50 MB). Fingerprint the head *and* track size.
4. **`uuid` is copied across fork files.** PK must include the session.
5. **Torn tails are first-class** — unparseable lines persist mid-file forever.
6. **`gitBranch == "HEAD"`** whenever cwd isn't a repo.
7. **The project dir name is lossy and irreversible.** Read `cwd` from inside.
8. **`toolUseResult` is untruncated** even when the model saw a 2 KB preview —
   great for signal, lethal for volume. Digest it.
9. **mtime is bumped by `utimes()` on resume** with zero content change.
