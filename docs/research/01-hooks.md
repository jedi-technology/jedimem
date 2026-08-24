# Claude Code hook system — reverse-engineered ground truth

> **Ported research.** Produced for this project's predecessor (`auto-memory`)
> and carried over unchanged: the reverse-engineering is version-specific to
> Claude Code 2.1.241 and underpins jedimem's capture design. Cited by
> [`05-distribution-security.md`](05-distribution-security.md).

Source: `~/.local/share/claude/versions/2.1.241` (342 MB Bun single-file
ELF, **not stripped**; readable JS from ~byte 293.6M–342M, greppable with `grep -a`).
Cross-checked against `docs.claude.com/en/docs/claude-code/{hooks,hooks-guide,settings}`.
**Where the docs and the bundle disagree, the bundle wins** — §7 lists 19 such
disagreements. No hooks were executed and nothing was modified; this is read from code.

## 1. There are 31 events, not the documented 28

Verified: two identical literal arrays (`Lq`, `A3b`) feed the zod enum `Dr(A3b)` used
for settings validation. Anything else is rejected at load with
`unknown hook event. Valid events: <list>`.

```js
["PreToolUse","PostToolUse","PostToolUseFailure","PostToolBatch","Notification",
 "UserPromptSubmit","UserPromptExpansion","SessionStart","SessionEnd","Stop",
 "StopFailure","SubagentStart","SubagentStop","PreCompact","PostCompact",
 "PermissionRequest","PermissionDenied","Setup","TeammateIdle","TaskCreated",
 "TaskCompleted","Elicitation","ElicitationResult","ConfigChange","WorktreeCreate",
 "WorktreeRemove","InstructionsLoaded","CwdChanged","FileChanged","DirectoryAdded",
 "MessageDisplay"]
```

Undocumented or little-known: `PostToolUseFailure`, `PostToolBatch`,
`UserPromptExpansion`, `StopFailure`, `SubagentStart`, `PostCompact`,
`PermissionRequest`, `PermissionDenied`, `Setup`, `TeammateIdle`, `TaskCreated`,
`TaskCompleted`, `Elicitation`, `ElicitationResult`, `ConfigChange`,
`WorktreeCreate`, `WorktreeRemove`, `InstructionsLoaded`, `CwdChanged`,
`FileChanged`, `DirectoryAdded`, `MessageDisplay`.

## 2. stdin: every event gets `transcript_path`

Built by `S_()`, spread into **every** event's payload:

```js
function S_(e,t,r,n){ return {
  session_id:      e.id,
  transcript_path: j$(e.id),          // <-- ALWAYS PRESENT
  cwd:             t,
  prompt_id:       kft() ?? undefined,
  permission_mode: r,                 // default|plan|acceptEdits|auto|dontAsk|bypassPermissions
  agent_id:        n?.agentId,        // ONLY inside a subagent
  agent_type:      o,
  effort:          a                  // {level: string}
}}
```

`j$(id)` = `~/.claude/projects/<cwd-slug>/<session_id>.jsonl`. `hook_event_name` is
added per event via `.and({hook_event_name: literal(...)})`.

**stdin is snake_case; stdout is camelCase.** The payload is one JSON line + `\n`,
then stdin closes. EPIPE (hook exits without reading stdin) is a non-blocking error.

Notable per-event extras:

| Event | Extra stdin fields |
|---|---|
| `Stop` | `stop_hook_active`, **`last_assistant_message?`**, `background_tasks?`, `session_crons?` |
| `SubagentStop` | `stop_hook_active`, `agent_id`, **`agent_transcript_path`**, `agent_type`, `last_assistant_message?` |
| `PostCompact` | `trigger`, **`compact_summary`** (the summary compaction produced) |
| `PreCompact` | `trigger` ∈ `manual\|auto`, `custom_instructions` (nullable) |
| `UserPromptSubmit` | `prompt`, `source?` ∈ `user\|sdk\|system\|loop_wakeup\|schedule_wakeup\|poll_event`, `session_title?` |
| `SessionStart` | `source` ∈ `startup\|resume\|clear\|compact\|**fork**`, `agent_type?`, `model?` |
| `SessionEnd` | `reason` ∈ `clear\|resume\|logout\|prompt_input_exit\|other` |
| `PostToolUse` | `tool_name`, `tool_input`, `tool_response`, `tool_use_id`, `duration_ms?` |
| `PostToolBatch` | `tool_calls: [{tool_name, tool_input, tool_use_id, tool_response?}]` |
| `InstructionsLoaded` | `file_path`, `memory_type` ∈ `User\|Project\|Local\|Managed`, `load_reason` ∈ `session_start\|nested_traversal\|path_glob_match\|include\|**compact**` |

`Stop.last_assistant_message` exists explicitly to "avoid the need to read and parse
the transcript file" (verbatim from the schema).

## 3. Which events can inject into the model, and how

"Inject" verified by tracing the attachment renderers, not just the schemas:

```js
LS(e) => `<system-reminder>\n${e}\n</system-reminder>`
hook_additional_context: (e) => [Cn({content: LS(`${e.hookName} hook additional context: ${e.content.join("\n")}`), isMeta:true})]
hook_success:            (e) => e.hookEvent!=="SessionStart" && !=="UserPromptSubmit" && !=="UserPromptExpansion" ? []
                                : [Cn({content: LS(`${e.hookName} hook success: ${e.content}`), isMeta:true})]
hook_blocking_error:     (e) => [Cn({content: LS(`${e.hookName} hook blocking error ...`), isMeta:true})]
hook_system_message:         () => []      // NOTHING
hook_non_blocking_error:     () => []      // NOTHING
hook_error_during_execution: () => []      // NOTHING
```

Consequences that shape the design:

- Injected content arrives as a **user-role message with `isMeta:true`**, wrapped in
  `<system-reminder>` and **prefixed** `"<hookName> hook additional context: "`,
  where `hookName` is `<Event>` or `<Event>:<matchQuery>`. **The prefix cannot be
  suppressed.**
- **Plain stdout injects only for `SessionStart`, `UserPromptSubmit`,
  `UserPromptExpansion`.** Everywhere else, exit-0 stdout is recorded and discarded.
- **`systemMessage` never reaches the model** (`hook_system_message: () => []`); the
  TUI renders `"<hookName> says: <content>"`. The docs say otherwise.
- **Hard cap 10,000 characters** (`vHt(text, id, kind, n = kYp)`, `kYp = 10000`) on
  `additionalContext`, `systemMessage`, `initialUserMessage`, and plain stdout.
  Beyond it the model sees `Output too large (...). Full output saved to: <path>` plus
  a preview. **Budget retrieval to ~8 KB.**

Only 12 events declare `additionalContext`; 11 actually deliver it
(`Notification`'s is schema-valid but discarded by its caller).

The 20 `hookSpecificOutput` variants that exist:

```jsonc
PreToolUse:          { hookEventName, permissionDecision?: "allow"|"deny"|"ask"|"defer",
                       permissionDecisionReason?, updatedInput?, additionalContext? }
UserPromptSubmit:    { hookEventName, additionalContext?, sessionTitle?, suppressOriginalPrompt? }
UserPromptExpansion: { hookEventName, additionalContext?, suppressOriginalPrompt? }
SessionStart:        { hookEventName, additionalContext?, initialUserMessage?, sessionTitle?,
                       watchPaths?, reloadSkills? }
Setup:               { hookEventName, additionalContext? }
SubagentStart:       { hookEventName, additionalContext? }
PostToolUse:         { hookEventName, additionalContext?, classifierContext?,
                       updatedToolOutput?, updatedMCPToolOutput? }
PostToolUseFailure:  { hookEventName, additionalContext? }
PostToolBatch:       { hookEventName, additionalContext? }
Stop:                { hookEventName, additionalContext? }
SubagentStop:        { hookEventName, additionalContext? }
Notification:        { hookEventName, additionalContext? }   // DISCARDED by caller
PermissionRequest:   { hookEventName, decision: {behavior:"allow", updatedInput?, updatedPermissions?}
                                             | {behavior:"deny", message?, interrupt?} }
PermissionDenied:    { hookEventName, retry? }
Elicitation:         { hookEventName, action?: "accept"|"decline"|"cancel", content? }
ElicitationResult:   { hookEventName, action?, content? }
CwdChanged:          { hookEventName, watchPaths? }
FileChanged:         { hookEventName, watchPaths? }
WorktreeCreate:      { hookEventName, worktreePath }
MessageDisplay:      { hookEventName, displayContent? }
```

**No variant exists for** `PreCompact`, `PostCompact`, `SessionEnd`, `StopFailure`,
`ConfigChange`, `InstructionsLoaded`, `TeammateIdle`, `TaskCreated`, `TaskCompleted`,
`WorktreeRemove`, `DirectoryAdded` — emitting one is a validation error. A
`hookEventName` mismatch throws
`Hook returned incorrect event name: expected 'X' but got 'Y'`.

> **Design consequence: there is no hook that can re-inject memory after
> compaction.** `PostCompact` has no injection channel. The only thing automatically
> re-read at compaction time is CLAUDE.md (`load_reason: "compact"`). So durable
> guidance that must survive compaction has to live in a file, not in a hook.

## 4. `async: true` — the key to zero-cost capture

Two routes, both verified:

1. **Config:** `{"type":"command","command":"…","async":true}`. The payload is
   written to stdin, stdin closed, the process detached and backgrounded
   immediately; `Ies` returns `{status:0, backgrounded:true}`.
   **Zero blocking cost to the agent loop.**
2. **Runtime:** print `{"async":true,"asyncTimeout":<ms>}` as the first line
   containing `}` and keep running. The eventual JSON stdout is collected and
   delivered on a later turn as an `async_hook_response` attachment — whose renderer
   injects both `systemMessage` and `hookSpecificOutput.additionalContext`, **for any
   originating event**. This is a back door to late injection from events that
   normally cannot inject.

`asyncRewake: true` additionally: a backgrounded hook exiting **2** pushes a
system-reminder with `priority:"next"` and `stopHookActive:true`, waking the model.

`forceSyncExecution` disables async — hard-coded for `MessageDisplay`.

## 5. Exit codes, in evaluation order

`kes(stdout)`: if `stdout.trim()` doesn't start with `{`, it's plain text; otherwise
it must parse **and** validate, else `validationError`.

1. **aborted / timed out** -> `hook_cancelled`. **Not blocking, nothing to the
   model.** A timeout is silent to the model — but the agent still waited.
2. **validationError && status !== 2** -> `non_blocking_error`; JSON discarded.
3. **valid JSON** -> mapped by `wfo()`. If `status === 2` and the JSON produced no
   blockingError, one is synthesized from stderr — so JSON + exit 2 both apply.
4. **status 0 + plain stdout** -> `hook_success` (injected only for the 3 events).
5. **status 2, empty stdout, stderr matching `/no such file|can't open/i`, on
   `Stop`/`SubagentStop`/`TaskCompleted`/`TeammateIdle`/plugin-`UserPromptSubmit`**
   -> downgraded to non-blocking "Hook script appears to be missing". An
   anti-footgun for uninstalled plugin hooks.
6. **status 2 otherwise** -> **blocking**:
   `blockingError = "[<command>]: <stderr || 'No stderr output'>"`, injected to the
   model. On `PreToolUse` the tool is denied; on `UserPromptSubmit` the prompt is
   blocked; on `Stop` the conversation is forced to continue.
7. **any other non-zero** -> `non_blocking_error`; user sees
   `Failed with non-blocking status code: …`; **model sees nothing**. Repeats for the
   same `event:command` are surfaced once (`Odh`).

> **Never exit 2 from a memory hook.** Always `exit 0`.

## 6. Configuration

### Hook types (zod, discriminated on `type`)

```jsonc
{ "type":"command", "command":"…",
  "args": ["…"],            // present => EXEC form, no shell
  "if": "Bash(git *)",      // permission-rule prefilter; tool events only
  "shell": "bash"|"powershell",
  "timeout": <seconds>,
  "statusMessage": "…",
  "once": bool,
  "async": bool, "asyncRewake": bool, "rewakeMessage": "…", "rewakeSummary": "…" }
```

Also `type: "prompt"` (single model call), `"agent"` (multi-turn subagent verifier),
`"http"` (POSTs the payload; **silently skipped for `SessionStart` and `Setup`**), and
`"mcp_tool"` (`{server, tool, input}` with `${…}` interpolation from the payload).
Internal-only: `callback`, `function`. **There is no `env` field.**

### Timeouts (verified constants)

| Scope | Default |
|---|---|
| Global (`eb`) | **600000 ms (10 min)** |
| `UserPromptSubmit` (`Eal`) | **30000 ms** |
| `MessageDisplay` (`VFy`) | 10000 ms (+ forceSync) |
| `SessionEnd` (`Ses`) | **1500 ms** |
| `type:"prompt"` (`fdh`) | 30000 ms |
| `type:"agent"` (`gdh`) | 60000 ms |

Per-hook `timeout` overrides these. **No maximum found.** A `SessionStart` hook with
no explicit timeout can stall the CLI for **ten minutes** — always set one.

### Concurrency

`rOi(I)` with `t = 1/0` — **unbounded parallel** merge. All matched hooks for an event
start simultaneously; event latency is `max`, not `sum`. No serialization, no cap.

### Precedence and merge — arrays CONCATENATE

```js
OP = ["userSettings","projectSettings","localSettings","flagSettings","policySettings"]
function RMe(e,t,r){
  if (Array.isArray(e) && Array.isArray(t)) {
    if (r === "fallbackModel") return t;     // replace
    return qn([...e, ...t]);                 // CONCATENATE + dedupe
  }
  ...
}
```

So `~/.claude/settings.json` -> `.claude/settings.json` -> `.claude/settings.local.json`
-> `--settings` -> managed, and **`hooks.<Event>` arrays concatenate across all five
scopes — nothing overrides.** Both a user-level and a project-level matcher run.

Command hooks are then deduped in `_zl` on
`(pluginRoot||skillRoot) \0 shell \0 command \0 JSON(args) \0 if`, so the same command
from two scopes runs once.

Config is **snapshot-based** (`initialHooksConfig` via `captureHooksConfigSnapshot()`,
refreshed by `vgt` on settings-reload/cwd-change/worktree paths) — mid-session edits
take effect via a reload event, not on every dispatch.

### Kill switches

- `policySettings.disableAllHooks: true` -> no hooks at all, including managed.
- `policySettings.allowManagedHooksOnly: true` -> only `policySettings.hooks`.
- `disableAllHooks: true` in a **non-policy** scope -> only *managed* hooks run
  (not "none" — a surprising asymmetry).
- `policySettings.strictPluginOnlyCustomization` -> non-plugin hooks dropped.
- Safe mode -> plugin hooks skipped.
- Workspace trust not accepted -> all hooks skipped.

> Any of these silently disables our hooks and **nothing tells the model**. The
> daemon must surface "hooks not firing" itself.

### Matcher semantics (`vAE`)

```js
function vAE(query, matcher, extended, aliases){
  if (!matcher || matcher === "*") return true;
  const literal = extended ? /^[a-zA-Z0-9_|, -]+$/ : /^[a-zA-Z0-9_|]+$/;
  if (literal.test(matcher))
    return matcher.split(extended ? /[|,]/ : /\|/).map(trim).filter(Boolean)
                  .flatMap(s => expandAliases(canonical(s), aliases)).includes(query);
  try { return new RegExp(matcher).test(query) || ... }   // UNANCHORED
  catch { log("Invalid regex pattern in hook matcher"); return false; }
}
```

Plain names are an exact pipe/comma list with tool-alias expansion; anything with
another character compiles as an **unanchored regex**.

`matchQuery` per event: tool events -> `tool_name`; `SessionStart` -> `source`;
`Setup`/`PreCompact`/`PostCompact` -> `trigger`; `SessionEnd` -> `reason`;
`SubagentStart`/`SubagentStop` -> `agent_type`; `InstructionsLoaded` -> `load_reason`;
`FileChanged` -> **`basename(file_path)`**.
`UserPromptSubmit`, `PostToolBatch`, `Stop`, `TeammateIdle`, `TaskCreated`,
`TaskCompleted`, `WorktreeCreate/Remove`, `CwdChanged`, `MessageDisplay` have **no**
matchQuery — the matcher is ignored and the hook always runs.

### Environment variables (from `Ies()`)

Inherited sanitized parent env, plus: `CLAUDECODE=1`,
**`CLAUDE_CODE_SESSION_ID`** (note: **`CLAUDE_SESSION_ID` is NOT set** — that spelling
is only a skill/command template token), `CLAUDE_CODE_CHILD_SESSION=1`, `CLAUDE_PID`,
`CLAUDE_EFFORT`, `TRACEPARENT`, `CLAUDE_PROJECT_DIR`, `COLUMNS`/`LINES` when a TTY,
and for plugins `CLAUDE_PLUGIN_ROOT`, `CLAUDE_PLUGIN_DATA`,
`CLAUDE_PLUGIN_OPTION_<KEY>`.

`CLAUDE_ENV_FILE` is set **only** for `SessionStart`, `Setup`, `CwdChanged`,
`FileChanged` (and only when `shell !== "powershell"`): whatever the hook writes there
is sourced into subsequent Bash tool commands.

Command-string substitution before spawn: `${CLAUDE_PROJECT_DIR}`,
`${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}`, `${user_config.KEY}`. POSIX children
are spawned `detached: true`.

## 7. Docs vs bundle — 19 disagreements (bundle is truth)

1. "All JSON field names are camelCase in hook input" — **false**. stdin is
   snake_case; stdout is camelCase.
2. "`systemMessage` = message for Claude to read" — **false**, it produces zero
   model-visible content.
3. "Events supporting `additionalContext`: `UserPromptSubmit`, `SessionStart`" —
   understated; 12 declare it, 11 deliver it.
4. `CwdChanged`: docs `previous_cwd`; bundle **`old_cwd`**.
5. `DirectoryAdded`: docs `directory_path`/`how`; bundle **`directory`**/**`source`**.
6. `FileChanged`: docs `watched_paths`; bundle **`event`** ∈ `change|add|unlink`.
7. `WorktreeCreate`: docs `worktree_path`; bundle **`name`**.
8. `Stop`: docs claim `turn_count`; **not present**.
9. `Notification`: docs omit required **`message`**.
10. `Elicitation`: docs `server_name`/`request`; bundle **`mcp_server_name`**/`message`.
11. `InstructionsLoaded`: docs omit **`memory_type`**, `globs?`, `trigger_file_path?`.
12. `SessionStart` matchers: docs omit **`fork`**.
13. `UserPromptSubmit`: docs omit **`source`** and `session_title?`.
14. `PreCompact`: docs say `reason`; bundle **`trigger`** + `custom_instructions`.
15. `SessionEnd` 1.5 s is a **default per-hook timeout**, not a shared budget.
16. `updatedPermissions` is on **`PermissionRequest.decision`**, not `PreToolUse`.
17. `type:"prompt"` output `{ok, reason, impossible}` — **unverified**.
18. Docs say 28 events; bundle has **31**.
19. Undocumented entirely: **`async`/`asyncRewake`/`rewakeMessage`/`rewakeSummary`**,
    the `async_hook_response` late-delivery attachment,
    `SessionStart.initialUserMessage`, `SessionStart.reloadSkills`,
    `PostToolUse.updatedToolOutput`/`classifierContext`, `terminalSequence`,
    `permissionDecision:"defer"`, and `type:"mcp_tool"` hooks.

## 8. Competing injection surfaces

| Mechanism | Pushed without model choice? | Capacity | Verdict |
|---|---|---|---|
| **Hook `additionalContext`** | yes | 10,000 chars/string, many strings | **best** — the only query-conditioned channel |
| **Pinned auto-memory** (`~/.claude/projects/<slug>/memory/*.md`, `metadata:{pinned:true}`) | yes | **max 4 files**, 200 lines / 25,000 B each | strong zero-process fallback; undocumented, machine-local. Rendered as `<pinned-memory path="…">` under `# Pinned memories` |
| **CLAUDE.md / `.claude/rules/*.md`** | yes | 4 MiB skip; warn at `max(40000, ctx*0.05*4)` chars | **the only thing that survives compaction automatically**; `paths:` frontmatter enables glob-triggered mid-session load; `@path` imports depth 4 |
| **Output style** | yes | no cap found | system-prompt placement (stronger than CLAUDE.md) but one at a time, needs `/clear`, and removes built-in coding instructions unless `keep-coding-instructions: true` |
| **MCP server `instructions`** | yes | **2048 chars/server** | small but real; needs a running server |
| **MCP tool / resource / prompt** | no | — | pull-only; model must choose |
| **Skills** | no | desc 1536 chars; listing budget 1% of context | cheapest up-front; a skill's `hooks:` frontmatter can bootstrap session hooks |

## 9. Design decisions this forces

**Capture — `Stop` with `async: true`.** Fires once per assistant turn, carries
`session_id`, `transcript_path`, `cwd`, `prompt_id`, and `last_assistant_message`.
`async: true` makes it free. Also worth capturing:

- `UserPromptSubmit` (async, separate entry from the retrieval one) — the raw user ask.
- `PostCompact` — `compact_summary` is a free, pre-digested, model-authored summary.
- `SubagentStop` — `agent_transcript_path` for work that is otherwise invisible.
- `PostToolUse` gated by `matcher: "Edit|Write|NotebookEdit"` + `if`, async — never
  spawn a process per `Read`.
- `PostToolBatch` — one call per parallel batch instead of N.
- `InstructionsLoaded` — dedupe our memories against guidance already loaded.

**Retrieval — `UserPromptSubmit` primary, `SessionStart` for priming.**
`UserPromptSubmit` is the only place we can retrieve conditioned on the actual query.
Hard 30 s timeout, fully blocking the turn, so target **< 300 ms** and never spawn a
cold interpreter — talk to a resident daemon over a unix socket, or use
`type:"http"` to localhost, or `type:"mcp_tool"` against an already-connected server.

`SessionStart` additionally offers **`initialUserMessage`**, which is auto-submitted as
the first user turn — a way to make the agent *act* on memory rather than just read
it. Use sparingly.

**Latency budget**

| Path | Budget | Target |
|---|---|---|
| `UserPromptSubmit` retrieval | hard 30 s, blocking | **< 300 ms** |
| `SessionStart` retrieval | default 600 s (!), blocking startup | set `timeout: 8`; < 1 s |
| `PreToolUse` retrieval | default 600 s, blocks the tool | set `timeout`; < 100 ms |
| `Stop` / `PostToolUse` capture | default 600 s | `async: true` -> ~0 ms |
| `SessionEnd` | **1500 ms** | enqueue and exit |

Process spawn alone is 50–150 ms for bash+python, which is why the fast path must be a
thin client, not an interpreter.

**Failure posture: fail open, always `exit 0`.** A timeout is silent to the model but
still costs the full wait. Exit 2 blocks the user's agent. Malformed JSON discards the
whole output. And on `SessionStart`/`UserPromptSubmit`, *any* stray stdout starting
with `{` is parsed as protocol JSON and will fail validation, destroying our real
output — so emit exactly one JSON object and nothing else, and keep all logging on
stderr.

## 10. Not verified

- Whether `qn()` in the merge customizer is deep-equality or reference dedupe
  (`_zl`'s command-level dedupe makes the observable behavior "runs once" regardless).
- The complete set of call sites refreshing the hooks snapshot (`vgt`) — so I cannot
  claim *every* mid-session `settings.json` edit re-arms hooks without a reload.
- Maximum allowed `timeout` (schema is only "positive int seconds"; no ceiling found).
- Full output->result mapping for `prompt`/`agent`/`http`/`mcp_tool` types (the
  `command` path was traced exhaustively; the others were sampled).
- Skill post-compaction re-attach budgets; plugin output-style byte limit.
- Whether `paths:` frontmatter on a top-level CLAUDE.md actually defers injection.
