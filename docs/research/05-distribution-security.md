# Distribution, installation, self-update, and security

**Method.** Claude Code claims are reverse-engineered from the installed bundle at
`~/.local/share/claude/versions/2.1.241` (342 MB Bun single-file ELF, not
stripped; `grep -a` / `dd` on byte offsets) **and** confirmed by running the real CLI
against a throwaway repo and observing what it wrote to disk. Every global mutation
made during testing was backed up and restored; the machine is in its original state.
Git behaviour was measured locally on git 2.43.0. Where a claim is only reasoned, it is
labelled.

Labels used throughout: **VERIFIED** (read in the bundle or measured on this machine),
**INFERRED** (follows from verified facts but not directly observed), **UNVERIFIED**
(from docs or reputation only).

Companion docs: [`01-hooks.md`](01-hooks.md) (hook internals — read first),
[`02-codex.md`](02-codex.md) (Codex hook internals), [`04-git-format.md`](04-git-format.md).

---

## 1. Executive summary

1. **There is a one-command install, and there is also a zero-command install.** For
   Claude Code the zero-command path is a committed `.claude/settings.json` declaring
   `extraKnownMarketplaces` + `enabledPlugins` pointing at a **relative path inside the
   repo**. VERIFIED end-to-end: a teammate clones, opens the repo, accepts the trust
   dialog, and from the *second* session onward the in-repo plugin's hooks execute with
   `CLAUDE_PLUGIN_ROOT` pointing at the in-repo directory. No `claude plugin install`,
   no copy into the plugin cache, no network.
2. **That same mechanism is the single largest security hole in the design.** VERIFIED
   by experiment: after the directory is trusted once, editing the in-repo
   `hooks/hooks.json` causes the *new* command to execute on the next session — no
   re-prompt, no hash check, no diff shown. A hostile PR that touches the plugin
   directory is remote code execution on every teammate who has ever trusted the repo.
   Codex is strictly better here: it content-pins each in-repo hook by SHA-256 in the
   *user's* config and revokes trust on any edit (see `02-codex.md`).
3. **Therefore jedimem must not ship an in-repo Claude Code plugin whose hook body is
   an in-repo script**, in the default configuration. The recommended default is the
   inverse: the executable lives outside the repo (installed under `~/.claude/`, version
   pinned), and only *data* (memory markdown) lives in the repo. See §7 and §9.
4. **Self-update must not use the GitHub REST API.** MEASURED on this machine: the
   unauthenticated limit is 60 requests/hour/IP, and **`304 Not Modified` responses to
   conditional (ETag) requests still decrement the quota** — 5 conditional requests
   consumed exactly 5 units. A shared office NAT burns 60/hour across all users.
   `git ls-remote` (~0.6 s, not REST-rate-limited) plus a TTL cache is the right check.
5. **The install script must never block the agent loop and must never write to a hook
   array it did not create.** Claude Code hook arrays **concatenate** across settings
   scopes (VERIFIED in bundle and by experiment: a user-scope, a project-scope and a
   plugin-scope `SessionStart` hook all fired in the same session), so appending is
   safe but re-appending duplicates. Idempotency has to come from a marker, not from
   "did I already write something here".
6. **Windows/PowerShell is out of scope for v0.1**, stated honestly in §5.6.

---

## 2. Claude Code plugin format (VERIFIED)

### 2.1 `claude plugin` subcommands

Enumerated with `--help` on 2.1.241 (VERIFIED, verbatim):

| Subcommand | Purpose (verbatim) |
|---|---|
| `install\|i <plugin>` | "Install a plugin from available marketplaces (use plugin@marketplace for specific marketplace)" |
| `uninstall\|remove <plugin>` | flags: `--keep-data`, `--prune`, `-s/--scope`, `-y/--yes` |
| `update <plugin>` | "Update a plugin to the latest version (restart required to apply)" |
| `enable` / `disable` | `-s/--scope <user\|project\|local>` (default: auto-detect) |
| `list` | `--json`, `--available` |
| `details <name>` | "Show a plugin's component inventory and projected token cost" |
| `validate <path>` | "Validate a plugin or marketplace manifest…"; `--strict` for CI |
| `init\|new <name>` | "Scaffold a new plugin at ~/.claude/skills/<name>/ (auto-loads next session as `<name>@skills-dir`)"; `--with skills,agents,hooks,mcp,lsp,output-style,channel` |
| `tag [path]` | "Create a `{name}--v{version}` git tag for a plugin release, validating that plugin.json and any enclosing marketplace entry agree"; `--push`, `--remote` |
| `prune\|autoremove` | remove orphaned auto-installed dependencies |
| `eval [target]` | plugin eval harness |
| `marketplace add\|list\|remove\|update` | "Add a marketplace from a URL, path, or GitHub repo" |

Install scope, verbatim from `claude plugin install --help`:

> `-s, --scope <scope>   Installation scope: user, project, or local (default: "user")`

`claude plugin update --help` additionally accepts `managed`.

Two undocumented-in-`plugin`-namespace sideload flags exist on the top-level CLI
(VERIFIED, `claude --help`):

```
--plugin-dir <path>   Load a plugin from a directory or .zip for this session only (repeatable)
--plugin-url <url>    Fetch a plugin .zip from a URL for this session only (repeatable)
```

The bundle gates these behind `disableSideloadFlags` and logs
`"disableSideloadFlags: dropping @inline plugin specs at load time"` (VERIFIED).

### 2.2 `.claude-plugin/plugin.json` — the manifest schema

VERIFIED: assembled in the bundle from a base object `qQ_` plus ~13 partial mixins,
composed at `Wtr`:

```js
Wtr = ye({ ...qQ_().shape,          // identity — the only required part
           ...VQ_().partial().shape, // hooks
           ...YQ_().partial().shape, // commands
           ...XQ_().partial().shape, // agents
           ...JQ_().partial().shape, // skills
           ...wKu().partial().shape, // outputStyles
           ...AKu().partial().shape, // themes
           ...teb().shape,           // workflows
           ...ieb().partial().shape, // channels
           ...neb().partial().shape, // mcpServers
           ...aeb().partial().shape, // lspServers
           ...TKu().partial().shape, // monitors
           ...ueb().partial().shape, // settings
           ...oeb().partial().shape, // userConfig
           ...ceb().partial().shape, // binaries
           ...deb().partial().shape })// experimental
```

**Identity block** (`qQ_`) — the only field that is required is `name`:

| Field | Req | Constraint / verbatim description |
|---|---|---|
| `$schema` | no | "JSON Schema reference for editor autocomplete/validation; **ignored at load time**" |
| `name` | **yes** | `.min(1)`, must not contain spaces; "Unique identifier for the plugin, used for namespacing (prefer kebab-case)" |
| `displayName` | no | may contain spaces; "not used for namespacing or lookup" |
| `version` | no | "Semantic version (e.g., 1.2.3) following semver.org specification" — **optional**, which matters for updates (§6) |
| `description` | no | free text |
| `author` | no | `{name (required), email?, url?}` |
| `homepage` | no | `.url()` — must parse as a URL |
| `repository` | no | plain string, not URL-validated |
| `license` | no | "SPDX license identifier" |
| `keywords` | no | `string[]` |
| `defaultEnabled` | no | "Whether the plugin starts enabled when the user has no explicit enabled/disabled setting for it (default: true)" |
| `dependencies` | no | `["name"]` or `["name@marketplace"]`; bare names resolve against the declaring plugin's own marketplace |
| `metadata` | no | "Free-form metadata for the plugin author's own use… **Preserved on the parsed manifest but not read by Claude Code**" |

**Component blocks.** Every one of these follows the same shape — a path, an array of
paths, or (for some) an inline object — and every one carries the same load-bearing
caveat, verbatim:

> "When set, the `<x>`/ directory is **not** auto-loaded — list its files here if you
> want both."

with one deliberate exception, `skills`:

> "Loaded **in addition to** the skills/ directory (except: for a marketplace entry whose
> source resolves to the marketplace root, declaring a specific subdirectory replaces the
> skills/ scan)."

| Key | Accepts | Notes |
|---|---|---|
| `hooks` | path \| inline hooks object \| array of either | "in addition to those in `hooks/hooks.json`, if it exists" |
| `commands` | path \| path[] \| `{name: {source\|content, description?, argumentHint?, model?, allowedTools?}}` | command name becomes `/plugin:name` |
| `agents` | path \| path[] | |
| `skills` | dir \| dir[] (`"."`/`"./"` = plugin root) | additive |
| `outputStyles`, `themes`, `workflows` | path \| path[] | |
| `mcpServers` | path \| MCPB file/URL \| `{name: serverConfig}` \| array | "in addition to those in the `.mcp.json` file, if it exists" |
| `lspServers` | `.lsp.json` path \| `{name: cfg}` \| array | |
| `channels` | array of `{server, displayName?, userConfig?}` | binds an MCP server as a message channel |
| `monitors` | path \| array of `{name, command, description, when}` | see §2.3 — **this is a second code-execution surface** |
| `settings` | `{key: value}` | "Settings to merge into the user settings while this plugin is enabled. **Only the documented allowlisted keys are applied.**" The allowlist itself was **not** located in the bundle — UNVERIFIED which keys. |
| `userConfig` | `{KEY: fieldSpec}` | see §2.4 |
| `binaries` | object | "sha256-pinned files to fetch into `bin/` **at install time**, keyed by basename (target triple encoded in the name)" |
| `experimental` | passthrough object | themes / hljsLanguages / monitors / outputStyles / `evals` — "may change without a deprecation cycle" |

Auto-loaded component directories, VERIFIED from the literal array in the bundle:

```js
r8a = ["commands","skills","agents","hooks","themes","output-styles","monitors","workflows"]
RmS = ["SKILL.md",".mcp.json",".lsp.json"]
wpr = [".claude-plugin", ...r8a, ...RmS]
```

So a plugin with **only** `.claude-plugin/plugin.json` + a `hooks/hooks.json` needs no
`hooks` key at all — the directory is scanned. (VERIFIED by experiment: our test plugin
declared `"hooks": "./hooks/hooks.json"` explicitly and it worked; the bundle string
"in addition to those in hooks/hooks.json, if it exists" establishes the implicit path.)

`hooks/hooks.json` content is "the hooks provided by the plugin, **in the same format as
the one used for settings**" (verbatim) — i.e. exactly the schema in `01-hooks.md` §6.

### 2.3 Two code-execution surfaces besides hooks

- **`monitors`** (VERIFIED, verbatim): "Shell command to run as a persistent background
  monitor. Each stdout line is delivered to the model as a `<task_notification>` event;
  the process runs for the session lifetime… **unsandboxed, same trust tier as hooks**".
  Armed `"always"` (at session start / plugin reload) or `"on-skill-invoke:<skill>"`.
  This is a *daemon-shaped* execution surface that a plugin gets for free, and is
  directly relevant to jedimem's capture daemon (see `07-monitor-runtime.md`).
- **`binaries`** — "sha256-pinned files to fetch into `bin/` at install time". This is
  an install-time network fetch, content-pinned. **INFERRED**: this, plus a `command`
  plugin source (§2.5), are the only install-time execution paths; there is no
  `postinstall` script hook in the manifest schema. I found no `postinstall`/`scripts`
  key anywhere in the plugin manifest schema (VERIFIED absence within the composed
  schema `Wtr`).

**Answer to "can a plugin ship a hook that runs on install?"** Not as an install hook —
but it does not need one: a `SessionStart` hook runs on the very next session, and a
`monitors` entry with `when: "always"` starts a long-lived process at session start.
Effectively yes, one session later. (VERIFIED by experiment for `SessionStart`.)

### 2.4 `userConfig` — where secrets are supposed to go

VERIFIED, verbatim: "User-configurable values this plugin needs. Prompted at enable
time. Non-sensitive values saved to `settings.json`; **sensitive values to secure
storage**. Available as `${user_config.KEY}` in MCP/LSP server config, hook commands,
and (non-sensitive only) skill/agent content."

Field spec (`HKu`, `.strict()`): `type` ∈ `string|number|boolean|directory|file`,
`title`, `description`, `required?`, `default?`, `multiple?`, **`sensitive?`** ("masks
dialog input and stores value in secure storage (keychain/credentials file) instead of
settings.json"), `min?`, `max?`.

Key regex: `/^[A-Za-z_]\w*$/` — "they become `CLAUDE_PLUGIN_OPTION_<KEY>` env vars in
hooks" (verbatim).

> **This is the answer to "where does an API key live" (§9).** If jedimem ever needs a
> key on the Claude Code side, it declares `userConfig` with `sensitive: true` and reads
> `$CLAUDE_PLUGIN_OPTION_<KEY>` in the hook. It never reads a key out of a repo file.
> `claude plugin install --config <key=value>` sets these non-interactively (VERIFIED
> from `--help`).

### 2.5 Marketplace manifest `.claude-plugin/marketplace.json`

VERIFIED schema (`qtr`):

| Field | Req | Notes |
|---|---|---|
| `$schema` | no | ignored at load |
| `name` | **yes** | no `/`, `\`, `..`, not `"."`; rejected if it "impersonates an official Anthropic/Claude marketplace"; reserved-name list enforced |
| `owner` | **yes** | `{name, email?, url?}` — note: **required**, unlike in `plugin.json` |
| `version`, `description` | no | |
| `plugins` | **yes** | array of entries (below) |
| `metadata.pluginRoot` | no | "Base directory for bare plugin source names… e.g. `./plugins` resolves `"source": "formatter"` as `./plugins/formatter`" |
| `forceRemoveDeletedPlugins` | no | auto-uninstall on delisting |
| `allowCrossMarketplaceDependenciesOn` | no | "**no transitive trust**" |
| `renames` | no | "Append-only map of old plugin name → current name (or null when removed). The loader follows this on plugin-not-found and **migrates user settings to the new name**" |

Plugin **entry** (`RMn`) = the full plugin manifest, all fields optional, plus:
`name` (required), `source` (required), `category?`, `tags?`, `headers?`,
`headersHelper?`, `relevance?`, and

> `strict: boolean (default true)` — "Require the plugin manifest to be present in the
> plugin folder. If false, the marketplace entry provides the manifest."

### 2.6 Plugin source types (`xKu`) — VERIFIED, complete list

| `source` | Fields | Notes |
|---|---|---|
| *(bare string)* | `"./plugins/foo"` | relative to marketplace root (or `metadata.pluginRoot`). `"."` normalises to `"./"` |
| `npm` | `package`, `version?`, `registry?` | anything npm accepts |
| `url` | `url`, `ref?`, `sha?` | git URL despite the name |
| `github` | `repo` (`owner/repo`), `ref?`, `sha?` | |
| `git-subdir` | `url`, `path`, `ref?`, `sha?` | "Cloned sparsely using partial clone (`--filter=tree:0`)… Only the specified subdirectory is materialized" |
| `archive` | `url` (https), `sha256?` | "When set, every download is verified against it and the install is **refused on mismatch**". Caveat, verbatim: "the update signal is the version string… changing only the digest while a version is declared does not trigger an update" |
| `command` | `command`, `timeout?` (≤600 s), `mode?: copy\|link` | **arbitrary shell** — "Runs through the platform shell… from the user's home directory". Prints one absolute path. Re-run on install, update, **and once per session in the background** |
| `unsupported` | | parse placeholder, never authored |

Marketplace source types (`gNr`): `url`, `github`, `git`, `npm`, `file`, `directory`,
plus policy-only sentinels `skills-dir`, `hostPattern`, `pathPattern`, and `settings`
(an inline marketplace declared directly in a settings file).

`github`/`git` support `sparsePaths` (cone-mode sparse checkout) and `skipLfs`.
Git fetches use `git fetch --depth 1 origin <ref>` (VERIFIED string in bundle).

### 2.7 Where things land on disk (VERIFIED by experiment)

| Path | Contents |
|---|---|
| `~/.claude/plugins/known_marketplaces.json` | `{name: {source, installLocation, lastUpdated, autoUpdate?}}` |
| `~/.claude/plugins/marketplaces/<name>/` | clone of a remote marketplace |
| `~/.claude/plugins/cache/<marketplace>/<plugin>/<version>/` | **copy** of the installed plugin, versioned |
| `~/.claude/plugins/installed_plugins.json` | schema **v2**: `{version:2, plugins:{ "<plugin>@<marketplace>": [ {scope, projectPath?, installPath, version, installedAt, lastUpdated, gitCommitSha?, resolvedVersion?, auto?} ] }}` — an **array** per id, one entry per scope |
| `~/.claude/plugins/data/<sanitised-id>/` | `CLAUDE_PLUGIN_DATA`; survives uninstall with `--keep-data` |
| `~/.claude.json` → `projects[<abs path>].hasTrustDialogAccepted` | the trust gate (§4) |

Overridable by env (VERIFIED): `CLAUDE_CODE_PLUGIN_CACHE_DIR`,
`CLAUDE_CODE_PLUGIN_SEED_DIR` (`PATH`-style list of extra plugin roots),
`CLAUDE_CODE_USE_COWORK_PLUGINS` (switches the dir name to `cowork_plugins`).

A **`.claude-plugin-link`** file is also recognised: `{ "target": "<absolute local
path>" }`, validated as absolute and local, rendering as `[mode: link]` (VERIFIED
string + schema). This is the local-development linking mechanism.

### 2.8 Installing from a path inside the user's own repo — VERIFIED, it works

This is the question that decides jedimem's shape, so it was tested for real.

Test repo layout:

```
testrepo/
  .claude/settings.json
  .jedimem/plugins/.claude-plugin/marketplace.json   # name: jedimem-local
  .jedimem/plugins/jedimem/.claude-plugin/plugin.json
  .jedimem/plugins/jedimem/hooks/hooks.json          # SessionStart -> touch a marker
```

**Path A — one command.** From inside the repo:

```
$ claude plugin marketplace add ./.jedimem/plugins
✔ Successfully added marketplace: jedimem-local (declared in user settings)
$ claude plugin install jedimem@jedimem-local --scope project
✔ Successfully installed plugin: jedimem@jedimem-local (scope: project)
```

Observed effects (VERIFIED):
- the relative path is **resolved to absolute** and stored in
  `~/.claude/plugins/known_marketplaces.json` with
  `installLocation` = the in-repo directory itself (**no copy of the marketplace**);
- `marketplace add` also wrote `extraKnownMarketplaces` into **`~/.claude/settings.json`**
  ("declared in user settings") — a global side effect the installer must be aware of;
- `install --scope project` wrote **both** `enabledPlugins` and `extraKnownMarketplaces`
  into the repo's **`.claude/settings.json`** — i.e. it produced exactly the committable
  artifact we want;
- and it **copied** the plugin to `~/.claude/plugins/cache/jedimem-local/jedimem/0.1.0`,
  recording `{"scope":"project","projectPath":"<repo>"}` in `installed_plugins.json`.

**Path B — zero commands, committed settings only.** With `~/.claude` reset to its
pre-test state and the repo containing only:

```json
{
  "extraKnownMarketplaces": {
    "jedimem-local": { "source": { "source": "directory", "path": "./.jedimem/plugins" } }
  },
  "enabledPlugins": { "jedimem@jedimem-local": true }
}
```

- **Session 1** in the repo: the marketplace is silently auto-registered into
  `~/.claude/plugins/known_marketplaces.json`, relative path resolved to absolute. The
  plugin's hooks do **not** fire this session.
- **Session 2 onward**: the hook fires. `CLAUDE_PLUGIN_ROOT` =
  `<repo>/.jedimem/plugins/jedimem` — the **in-repo directory**, not a cache copy. No
  entry is ever added to `installed_plugins.json` and nothing is copied into
  `~/.claude/plugins/cache/`.

So: a **relative** `directory` source works, the committed settings file is
machine-independent, and the plugin executes out of the working tree. One-session lag
on first use; running `claude plugin marketplace add ./.jedimem/plugins` removes the lag.

The settings key is documented for exactly this, verbatim:

> `extraKnownMarketplaces` — "Additional marketplaces to make available for this
> repository. **Typically used in repository `.claude/settings.json` to ensure team
> members have required plugin sources.**"

and precedence, verbatim from `enabledPlugins`:

> "Settings precedence is user < project < local < flag < policy, so to disable a plugin
> that project settings enable, set it to `false` in `.claude/settings.local.json` —
> setting `false` in `~/.claude/settings.json` is overridden by the project."

> **Per-project enablement without global config: YES.** `--scope project` writes to the
> repo; `--scope local` writes to `.claude/settings.local.json` (untracked). Nothing has
> to touch `~/.claude/settings.json` — except that `claude plugin marketplace add` does
> so as a side effect, which the installer should undo or avoid by writing the project
> settings file directly.

### 2.9 Enterprise / policy gates that can silently disable us

VERIFIED settings keys (managed/policy scope):
`strictKnownMarketplaces` (+ alias `allowedMarketplaces`), `blockedMarketplaces` —
both match sources exactly, support `{"source":"github","repo":"owner/*"}` wildcards,
and "The check happens **BEFORE** downloading, so blocked sources never touch the
filesystem"; `disableCommandPluginSources`; `strictPluginOnlyCustomization`;
`disableAllHooks` / `allowManagedHooksOnly` (see `01-hooks.md` §6).
`--safe-mode` disables all customizations and sets `CLAUDE_CODE_SAFE_MODE=1`.
`--setting-sources <user,project,local>` restricts which settings files load at all.

A `pathPattern` policy entry exists specifically to allow filesystem marketplaces —
`".*"` for all, or e.g. `"^/opt/approved/"`. **An enterprise that sets
`strictKnownMarketplaces` without a `pathPattern` entry will block jedimem's in-repo
marketplace outright** (INFERRED from the verbatim semantics).

### 2.10 Workspace trust — the gate, and where it stops (VERIFIED by experiment)

Claude Code has a VS Code-style workspace-trust gate. It is recorded per absolute path
in `~/.claude.json` as `projects["<abs path>"].hasTrustDialogAccepted: true`.

Four experiments, each run against the throwaway repo, global state restored between
runs:

| # | Setup | Result |
|---|---|---|
| 1 | committed project settings, **no** trust entry, `claude -p` | marketplace **not** registered; hook did **not** fire; no `projects` entry created |
| 2 | same + `hasTrustDialogAccepted: true` | marketplace auto-registered silently; second session, hook fired |
| 3 | trust granted, then **edit `hooks/hooks.json` in the repo** | the **new** command executed on the next session. No prompt, no diff, no hash check |
| 4 | trust granted, hook added directly to project `.claude/settings.json` **and** a plugin hook present | **both** fired in the same session, alongside the user-scope `SessionStart` hook |

Experiment 1 corrects the CLI help text. `claude --help` says of `-p/--print`:

> "The workspace trust dialog is **skipped** when Claude is run in non-interactive mode
> (via -p, or when stdout is not a TTY…). Only use this in directories you trust.
> Settings files that fail validation are silently ignored in this mode."

"Skipped" reads as "assumed trusted". Measured behaviour is the opposite: the dialog is
not shown **and trust is not granted**, so project-scope customizations — including
plugins and hooks — do not load. **This is a fail-closed default and it is good**, but
it means a CI job running `claude -p` in a fresh checkout gets **no** jedimem, silently.
The installer must make that explicit rather than let CI quietly run without memory.

Experiment 3 is the finding that shapes the whole threat model: **trust is granted to a
directory, once, forever, and is not re-evaluated when the executable content inside
that directory changes.** Compare Codex, which pins each in-repo hook by SHA-256 in the
user's config (`02-codex.md` §"The hook system"): editing the file revokes trust until
the user re-approves. Claude Code has no equivalent for plugin hooks.

Experiment 4 confirms, at runtime, the concatenation semantics derived from the bundle in
`01-hooks.md` §6 (`RMe` returns `qn([...e, ...t])` for arrays):

> **Hook arrays concatenate across every settings scope and across plugins. Nothing
> overrides.** Appending is therefore safe; *re-*appending duplicates. Command hooks are
> deduped in `_zl` on `(pluginRoot||skillRoot) \0 shell \0 command \0 JSON(args) \0 if`,
> so a byte-identical command from two scopes runs once — but a command that differs by
> a single character (a changed path, a version bump) runs twice.

---

## 3. Codex install story (distribution angle only)

Hook internals are in [`02-codex.md`](02-codex.md); this section covers only how a
third-party extension gets onto a machine. All facts here were established by running
the real CLI against a throwaway `CODEX_HOME` and a throwaway repo; the user's real
`~/.codex/config.toml` was verified byte-identical afterwards.

> **pi is deliberately out of scope in this document.** The pi distribution story is
> deferred; `03-pi.md` covers what is known so far.

### 3.1 Two channels, and only one is repo-scoped

| Channel | Install steps | Scope | Trust required | Travels with `git clone`? |
|---|---|---|---|---|
| **Plugin** (marketplace → `codex plugin add`) | 2 CLI commands | **user-global, always** | no | no — code is copied into a version-keyed cache |
| **Repo drop-in — skills** (`<repo>/.codex/skills/`, `<repo>/.agents/skills/`) | **none** | project | **no** | **yes** |
| **Repo drop-in — hooks** (`<repo>/.codex/hooks.json`) | none, but needs approval | project | **yes** (`trust_level` + per-hook SHA-256) | yes |
| **Repo drop-in — config** (`<repo>/.codex/config.toml`) | none | project | **yes** (`trust_level`) | yes |
| **MCP** (`codex mcp add`) | 1 CLI command | user-global (no `--scope`) | no | no |

VERIFIED A/B on the trust gate: with `[projects."<repo>"] trust_level = "trusted"` in the
*user's* config, `codex mcp list` from that cwd includes a server declared only in the
repo's `.codex/config.toml`; remove the trust stanza and it vanishes. Same for hooks
(`hooks/list` returns one entry with `"source":"project"` when trusted, `[]` when not).
**Skills are not gated at all** — `skills/list` returns repo skills with `scope: "repo"`
in an untrusted repo.

> **The single most useful Codex fact for jedimem: `<repo>/.codex/skills/` and
> `<repo>/.agents/skills/` are a zero-install, zero-trust, git-native extension surface.**
> It buys no code execution — which is exactly why it needs no trust — but it is the
> right home for anything jedimem wants an agent to *read*.

### 3.2 Plugins are irreducibly user-global (VERIFIED)

```
$ codex plugin marketplace add <repo-root> --json
{"marketplaceName":"jedimem-repo","installedRoot":"<repo-root>","alreadyAdded":false}
$ codex plugin add jedimem@jedimem-repo --json
{"pluginId":"jedimem@jedimem-repo","version":"0.1.0",
 "installedPath":"$CODEX_HOME/plugins/cache/jedimem-repo/jedimem/0.1.0","authPolicy":"ON_USE"}
```

Both writes land in **`$CODEX_HOME/config.toml`**:

```toml
[marketplaces.jedimem-repo]
source_type = "local"
source = "/abs/path/to/repo"

[plugins."jedimem@jedimem-repo"]
enabled = true
```

and the plugin body is **recursively copied** into
`$CODEX_HOME/plugins/cache/<marketplace>/<plugin>/<version>/`. There is no per-project
install, and the binary carries the string `"repository-scoped plugin migration is not
allowed"`. `codex plugin add` never accepts a path or URL — everything is
`PLUGIN@MARKETPLACE`, so shipping a plugin from a repo means shipping a
`marketplace.json` too.

`codex plugin marketplace add <SOURCE>` accepts "a local path, `owner/repo[@ref]`, HTTPS
Git URL, or SSH Git URL", with `--ref` and repeatable `--sparse`.

**Codex reads Claude Code's manifests.** The loader probes `plugin.json`,
`.codex-plugin/plugin.json`, **`.claude-plugin/plugin.json`**, and
`.cursor-plugin/plugin.json`, plus a vendor-neutral
`https://agent-plugins.org/schemas/1.0.0/plugin.schema.json`; marketplace probe list
includes `.claude-plugin/marketplace.json`. **INFERRED**: one directory could serve both
tools. **UNVERIFIED**: whether Codex tolerates a Claude-shaped manifest — Codex's own
validator requires an `interface` block (`displayName`, `shortDescription`,
`longDescription`, `developerName`, `category`, `capabilities`, `defaultPrompt`) that
Claude Code does not have, and **rejects a `hooks` key outright**
(`"Validation rejects unsupported manifest fields such as \`hooks\`"`), even though the
runtime's `HookSource` enum includes `"plugin"` and `HookMetadata` carries `pluginId`.

### 3.3 Idempotency — the CLI is already idempotent; hand-editing TOML is not

VERIFIED: re-running `codex plugin marketplace add` returns `"alreadyAdded": true` and
leaves `config.toml` byte-identical; re-running `codex plugin add` likewise. The
`[plugins]` key is version-free, so version bumps do not orphan entries.

> **Therefore: shell out to `codex plugin marketplace add` + `codex plugin add`. Never
> hand-edit `config.toml`.** Codex's own docs agree: "Marketplace manipulation should
> happen through commands, not by hand-editing `marketplace.json` or `config.toml`."

Concrete failure modes of hand-editing (all VERIFIED or directly evidenced):

1. The file is a nested-table minefield — real keys look like
   `[hooks.state."/abs/path/.codex/hooks.json:post_tool_use:0:0"]`, a quoted dotted key
   containing `/`, `.` and `:`. `grep -q '^\[plugins\.'` is wrong the moment another
   plugin exists.
2. Appending a bare `key = value` reparents it into whatever table happens to be last.
3. `codex --strict-config` turns a misspelled key from "silently ignored" into fatal.
4. Removal requires deleting a header *and* an unknown-length body up to the next `[`.
5. **Codex's own writer destroys comments.** MEASURED: a `# comment` on line 1 and an
   inline comment on a value were both gone after `codex mcp add`. So you cannot use
   comment markers to delimit a managed block, *and* any jedimem-triggered CLI write
   silently eats the user's config comments. That is a side effect to disclose.

### 3.4 The SHA-256 hook trust allow-list — better than Claude Code, and still bypassable

`HookTrustStatus` ∈ `managed | untrusted | trusted | modified`.

**VERIFIED end-to-end, and this is the security headline:** an installer can pre-approve
its own hook by writing plain TOML into the user's config — no prompt, no signature, no
keystore, no owner check:

```toml
[hooks.state."/abs/repo/.codex/hooks.json:post_tool_use:0:0"]
trusted_hash = "sha256:1bfa3c61…"
```

after which `hooks/list` reports `trustStatus: trusted`. Editing the hooks file
afterwards flips it to `modified` — so the model protects against **post-approval
tampering by a third party**, not against **the installer the user already ran**. There
is also `codex --dangerously-bypass-hook-trust`: "Run enabled hooks without requiring
persisted hook trust for this invocation. DANGEROUS."

**What the hash covers** (VERIFIED by three controlled mutations):

| Change | `currentHash` |
|---|---|
| minify the whole file, semantics identical | **unchanged** |
| insert a different hook group at index 0, pushing ours to index 1 | **unchanged** (at the new key `…:post_tool_use:1:0`) |
| add `"statusMessage":"x"` to the hook itself | **changed** |

So the digest is over the **canonicalised individual hook definition** —
whitespace- and position-independent — while the *key* is position-dependent, so
reordering the file breaks the trust entry even though the hash still matches.

**The canonicalisation is not publicly reproducible.** ~900 candidate preimages were
brute-forced (field orders, key spellings, separator styles, per-file / per-group /
per-hook scopes) against two real hashes: zero matches; the whole-file SHA-256 is a
different value. An installer therefore **cannot compute** the hash. It can only
(a) hard-code a value captured once — brittle across Codex versions, (b) read
`currentHash` from the app-server `hooks/list` at install time and write it back, or
(c) leave the hook untrusted and let the user approve in the TUI.

> **jedimem's position (see §8): option (c), always.** Options (a) and (b) are the tool
> silently granting itself execution rights on the user's machine. We will not do it,
> and we will say in the README that a one-time TUI approval is required.

### 3.5 Versioning, update, uninstall

- Codex itself: atomic-symlink-swap over versioned release dirs —
  `~/.codex/packages/standalone/current -> releases/<ver>-<target>/`, with
  `codex-package.json` (`layoutVersion`, `version`, `target`, `entrypoint`) and
  `~/.codex/version.json` (`latest_version`, `last_checked_at`, `dismissed_version`).
  `codex update` takes **no `--version`**, so there is no CLI pin for the tool itself;
  config keys `check_for_update_on_startup`, managed `autoUpdateEnabled` /
  `managedCodexVersion` exist.
- Plugins: version = `plugin.json.version`, **strict semver, required**, and it is the
  cache directory name. Bumping the version re-copies and **deletes the old version
  dir** (VERIFIED). Local marketplaces re-copy on every `plugin add` even without a bump
  (MEASURED — this contradicts the vendor's cachebuster guidance, so treat the
  documented `<base>+codex.<cachebuster>` convention as belt-and-braces).
  Git marketplaces: `codex plugin marketplace upgrade [NAME]`. Pinning is **`--ref` only**
  — no version constraints or ranges.
- Skills: no CLI, no version metadata, no upgrade, and `skill-installer` "Aborts if the
  destination skill directory already exists" ⇒ **not idempotent**; a re-install must
  `rm -rf` first.
- Uninstall leaves residue: `codex plugin remove` leaves an empty
  `plugins/cache/<marketplace>/` and leaves `[marketplaces.<name>]`;
  `codex plugin marketplace remove` leaves stray blank lines; **neither touches
  `[hooks.state]` or `[projects.*]`**.

> **A stale `[hooks.state]` entry is a latent vulnerability**: it means a *future* file
> at that exact path with that exact hook definition is pre-trusted. jedimem's uninstall
> must remove its own `[hooks.state]` keys, and must **not** remove
> `[projects."<repo>"].trust_level` (the user may have set it themselves).

### 3.6 What jedimem's Codex install actually needs to write

```
<repo>/.codex/hooks.json           the capture hook (trust-gated, user approves once)
<repo>/.codex/skills/jedimem/      SKILL.md — zero-trust, zero-install read surface
<repo>/AGENTS.md                   the compiled memory section (already the design)
```

and **nothing in `~/.codex/`** in the default configuration. The plugin channel is
available but is user-global and copies code out of the repo, which defeats the
"git clone and it's current" property; it is the right channel only if jedimem later
ships an MCP server that must be registered globally.


---

## 4. `install.sh` specification

### 4.1 Design stance

The install script's job is **not** to install jedimem's runtime. Its job is to write
the smallest possible set of *declarations* into the right files and get out. Every byte
it writes is a byte someone has to audit and a byte that can conflict with a teammate's
setup.

Three delivery modes must all work:

| Mode | Invocation | Constraint |
|---|---|---|
| **clone** | `git clone … && ./install.sh` | the normal path; the script can read sibling files |
| **curl-pipe** | `curl -fsSL …/install.sh \| sh` | **`$0` is `sh`, there are no sibling files, and stdin is the script itself** — so the script may not read stdin and must fetch or vendor anything else it needs |
| **in-repo** | `.jedimem/bin/install` committed in the target repo | already inside the repo; must not re-fetch |

The curl-pipe mode is offered because it is what users type, **but it is not the
documented recommended path** (see §8). `SECURITY.md` already commits to that.

### 4.2 Detection requirements

| Thing to detect | Method (VERIFIED on this machine) | Failure mode |
|---|---|---|
| POSIX shell | `#!/bin/sh`, no bashisms; run under `sh` not `bash` | macOS ships bash 3.2; do not rely on `declare -A`, `mapfile`, `[[ ]]` |
| git repo root | `git rev-parse --show-toplevel` | **empty/exit≠0 ⇒ not a repo ⇒ refuse.** jedimem is repo-scoped by definition |
| **worktree** | `git rev-parse --git-dir` ≠ `git rev-parse --git-common-dir` | in a worktree `--git-dir` = `<main>/.git/worktrees/<name>`, `--git-common-dir` = `<main>/.git`, and **`.git` is a file, not a directory** — measured |
| where untracked ignores go | **`$(git rev-parse --git-common-dir)/info/exclude`** | measured: the common dir's `info/exclude` applies in *every* worktree; a per-worktree `info/exclude` is not a thing to rely on |
| submodule | `git rev-parse --show-superproject-working-tree` non-empty | installing into a submodule almost certainly means the user is in the wrong directory — warn |
| bare repo | `git rev-parse --is-bare-repository` | refuse |
| Claude Code | `command -v claude` **and** `claude --version` parses | a `claude` on `PATH` that is a shell alias/wrapper is common |
| Claude Code version | `claude --version`; require ≥ the version where `--scope project` exists | project scope, `extraKnownMarketplaces`, and plugin `monitors` are all version-gated. **UNVERIFIED**: the exact minimum version — must be established by testing older releases before shipping |
| Codex | `command -v codex` + `codex --version` | see §3 |
| policy lockdown | `claude plugin list --json` succeeds and our plugin appears after install | catches `strictKnownMarketplaces` / `--safe-mode` / `disableAllHooks` silently dropping us |

### 4.3 What it writes, and nothing else

```
<repo>/.claude/settings.json          merge: extraKnownMarketplaces + enabledPlugins
<repo>/.codex/hooks.json              merge: the capture hook            (see §3)
<repo>/.jedimem/config.yml            created if absent
$(git rev-parse --git-common-dir)/info/exclude   append: .jedimem/local/
```

It must **not** write `~/.claude/settings.json`, must **not** write
`~/.codex/config.toml` hook-trust hashes (§7 threat T2), and must **not** run
`git add`, `git commit`, or any network command.

### 4.4 Idempotency contract

Idempotency cannot be "check whether something is already there", because hook arrays
concatenate and a near-identical entry is a *second* entry (§2.10). The contract is:

1. **Every object jedimem writes into a shared array carries a marker key.** For Claude
   Code hook entries, `"statusMessage"` is a schema-legal free-text field but a poor
   marker; the reliable marker is that jedimem writes **no** hook entries into
   `settings.json` at all — its hooks live inside the plugin, addressed by the single
   `enabledPlugins` key. *Prefer designs where the idempotency unit is a map key, not
   an array element.* Map keys are idempotent for free.
2. Where an array append is unavoidable (Codex `.codex/hooks.json`), the entry is
   identified by an exact-match on a canonical JSON serialisation of the entry jedimem
   would write **now**, and by a `"description"` field containing the literal token
   `jedimem`. On re-run: remove every entry whose description contains `jedimem`, then
   append the current one. This makes re-run converge rather than accumulate.
3. **A JSON edit is a parse → modify → atomic-write, never a text patch.** Requirement:
   the script needs a JSON tool. Order of preference: `python3` → `jq` → refuse with a
   clear message. Do **not** hand-roll JSON in `sed` — a repo's `.claude/settings.json`
   may contain comments (Claude Code's parser tolerates JSONC in places) and will
   certainly contain user content.
4. **Atomic write**: write to `<file>.jedimem.tmp` in the same directory, `chmod` to the
   original's mode, then `mv`. Never truncate-in-place.
5. **Backup**: before the first modification of any file, copy it to
   `<file>.bak.jedimem-<epoch>` and print the path. Uninstall restores nothing
   automatically (§4.6) but the user has a file to diff.
6. `--dry-run` prints the exact resulting file contents to stdout and writes nothing.
   This is mandatory, not a nicety: it is the only way a security-conscious team can
   review the change before it happens.

### 4.5 Ordering and the one-session lag

After writing project settings, run `claude plugin marketplace add ./.jedimem/plugins`
**if `claude` is present and the repo is already trusted**, to eliminate the
session-1 lag documented in §2.8. Caveat, VERIFIED: that command also writes
`extraKnownMarketplaces` into `~/.claude/settings.json`. Either accept that (and remove
it on uninstall) or skip the command and accept the lag. **Recommendation: skip it**;
one session of lag is cheaper than a global side effect the user did not ask for, and
the script should say so in its output.

### 4.6 Uninstall

`install.sh --uninstall` must:

1. remove `enabledPlugins["jedimem@…"]` and `extraKnownMarketplaces["jedimem-…"]` from
   `<repo>/.claude/settings.json`, and delete the file **only if it becomes `{}` and was
   created by us** (recorded in `.jedimem/local/install-receipt.json`);
2. remove the jedimem entry from `<repo>/.codex/hooks.json`, same rule;
3. remove the `.jedimem/local/` line from `info/exclude`;
4. run `claude plugin uninstall jedimem@jedimem-local --scope project` if an install
   receipt records that a real install was performed;
5. **leave `.jedimem/memories/` alone, always.** Memory is the user's data. Print where
   it is.
6. **not** revoke workspace trust and **not** touch `~/.claude.json`.

An **install receipt** at `.jedimem/local/install-receipt.json` records: jedimem version,
timestamp, every file touched, whether each was created or modified, and the backup path.
Uninstall reads the receipt; with no receipt it runs in a conservative mode that only
removes keys it can positively identify.

### 4.7 Every place it could damage a user's setup

| Risk | Why | Guard |
|---|---|---|
| Clobbering `.claude/settings.json` | it is a shared, hand-edited, committed file | parse+merge only; never rewrite wholesale; backup + `--dry-run` |
| Reformatting the whole settings file | `json.dump` normalises key order, indentation, and drops comments | preserve input indentation where detectable; **warn** that comments will be lost; prefer appending only the two keys |
| Duplicating hooks | arrays concatenate, dedupe is byte-exact | marker-based remove-then-append (§4.4.2) |
| Writing to `~/.claude/settings.json` | a global change for a repo-local tool | forbidden; do not call `plugin marketplace add` by default |
| Corrupting `~/.codex/config.toml` | TOML with `[hooks.state]` sections; naive append can land inside another table | never edit it (§3); if unavoidable, require a TOML parser and refuse otherwise |
| Adding to `.gitignore` | `.gitignore` is committed — a per-user path in it is a diff for everyone | per-user ignores go in `info/exclude` (§9) |
| Running in a worktree and writing to the wrong `.git` | `.git` is a file there | use `--git-common-dir`, never `"$repo/.git"` |
| Running in `$HOME` | `git rev-parse` may succeed if `$HOME` is a repo | refuse if toplevel == `$HOME` |
| Symlinked repo root | `--show-toplevel` may differ from `$PWD` after `realpath` | canonicalise once, use it everywhere; trust records in `~/.claude.json` are keyed by the path Claude Code sees |
| Partial run | curl-pipe truncation (§8) | wrap the whole script in `main() { … }` with `main "$@"` on the last line so a truncated download executes nothing |
| `set -e` surprises | `git rev-parse` returning non-zero inside a `$( )` under `set -e` | `set -eu`; guard every probe with `|| true` and check explicitly |
| Silently doing nothing | policy settings can drop plugins with no error | post-install verification step + non-zero exit if the plugin does not appear |

### 4.8 Windows / PowerShell — **out of scope for v0.1, stated honestly**

The hook schema does support it: `01-hooks.md` §6 records `"shell": "bash"|"powershell"`
on command hooks, and `CLAUDE_ENV_FILE` is documented as *not* set when
`shell === "powershell"`. So a Windows plugin is possible. But:

- the capture path is `sh` + `curl` (13 ms, per the README) and has no PowerShell twin;
- `install.sh` cannot run natively; a separate `install.ps1` is a second artifact with a
  second threat surface and a second uninstall path;
- Git for Windows changes line endings, which `.gitattributes` already has to fight
  (`text eol=lf` is already committed for `.jedimem/**`).

**Position: Linux and macOS are supported. Windows is supported only under WSL2, where
it is Linux. `install.ps1` is not in v0.1.** Say this in the README rather than shipping
something that half-works.

---

## 5. Update mechanism

### 5.1 Requirements

1. Zero added latency in the agent loop. `01-hooks.md` §9 budgets `UserPromptSubmit`
   at <300 ms; an update check must contribute **0 ms** to it.
2. No phone-home per session.
3. A hard no-op when offline.
4. Version pinning must be possible and must be the default for teams.
5. Applying an update must be safe when the on-disk memory format changed.

### 5.2 The check: `git ls-remote`, not the GitHub API

**MEASURED on this machine (2026-08-24):**

```
$ time git ls-remote https://github.com/anthropics/claude-plugins-official.git HEAD
340e33ae…  HEAD
real  0m0.585s
```

```
$ curl -s https://api.github.com/rate_limit → core: {"limit": 60, …}
```

and, importantly:

```
5 × curl -H "If-None-Match: <etag>" …/releases/latest  → 304 304 304 304 304
core remaining: 35 → 30      # delta = 5
x-ratelimit-limit: 60  x-ratelimit-used: 31
```

> **VERIFIED, and contrary to the usual assumption: unauthenticated `304 Not Modified`
> responses DO consume GitHub REST quota.** ETag conditional requests buy you bandwidth,
> not rate limit. 60/hour is **per IP**, so an office behind one NAT shares it with every
> other tool on the network. A REST-based update check is unacceptable.

`git ls-remote` is not part of the REST API and is not counted by it (INFERRED — GitHub
does not publish a git-protocol rate limit; it is documented only as subject to abuse
detection). It is also the only check that works for self-hosted GitLab/Gitea, which a
team repo may well be.

### 5.3 The mechanism

```
.jedimem/local/update-check.json      # untracked, per-machine
{ "lastCheckedAt": 1787564000,
  "lastSeenRemoteSha": "…",
  "lastSeenVersion": "0.4.2",
  "consecutiveFailures": 0 }
```

- **TTL 24 h**, jittered ±25% so a team does not stampede at 09:00.
- The check runs **only** in an `async: true` hook (`01-hooks.md` §4: payload written to
  stdin, process detached and backgrounded, `{status:0, backgrounded:true}` — **zero
  blocking cost**), or inside the already-running capture daemon. Never in
  `UserPromptSubmit`, never in `SessionStart` without `async`.
- Command: `git ls-remote --tags --refs <origin> 'refs/tags/v*'`, with
  `GIT_TERMINAL_PROMPT=0`, `GIT_SSH_COMMAND='ssh -o BatchMode=yes'`, and a hard
  `timeout 5`. Any failure ⇒ increment `consecutiveFailures`, write the file, exit 0.
  After 3 consecutive failures back off to a 7-day TTL. **Offline is indistinguishable
  from up-to-date, by design.**
- Result surfacing: **never** as a blocking message, never as an `additionalContext`
  injection that costs the user context window. Write a one-line note into
  `.jedimem/local/` that `jedimem status` prints. At most, a `SessionStart`
  `systemMessage` — which `01-hooks.md` §3 established reaches the **TUI only** and
  never the model, making it exactly the right channel for a human-facing nag.

### 5.4 Pinning

The committed pin lives in `.jedimem/config.yml`:

```yaml
jedimem:
  version: "0.4.2"          # exact, or a tag
  channel: "pinned"          # pinned | minor | latest
```

`pinned` is the default and the only one recommended for a team repo. The rationale is
threat T1 (§7): pinning does not prevent a compromise, but it converts "every teammate
is compromised at 09:00 tomorrow" into "one person reviews a diff and decides".

For the Claude Code plugin specifically, pinning is also expressible in the marketplace
entry: `source: {source: "git-subdir", url: …, path: …, sha: "<40-hex>"}` — the schema
requires a full 40-character lowercase SHA (VERIFIED, `Xta`). A `sha` pin is
strictly stronger than a `ref` pin because tags are mutable.

### 5.5 Applying an update, and the format-migration story

Every artifact jedimem writes carries a **format version stamp**, separate from the
tool version:

```
.jedimem/config.yml        format: 1
.jedimem/memories/*.md     front-matter:  jedimem_format: 1
refs/jedimem/log commits   trailer:       Jedimem-Format: 1
```

Rules:

1. **A newer jedimem reading an older format migrates it**, in one direction only,
   never automatically committing the result — it writes the migrated files and tells
   the user to review and commit.
2. **An older jedimem reading a newer format refuses to write and degrades to read-only**,
   printing the required version. It must not silently drop fields it does not
   understand: the union-merge experiment in `04-git-format.md` already showed how
   silently-resurrected content becomes a correctness bug.
3. Migrations are **separate committed scripts** (`.jedimem/migrations/001-….sh`),
   idempotent, and re-runnable, so a team that updates at different times converges.
4. **A format bump is a major version bump.** No exceptions.
5. The update check compares *format* versions too, and warns when a teammate on the
   pinned version could not read what you are about to commit. This is the real hazard
   in a team tool: the person who updates first writes files nobody else can read.

### 5.6 Degrade-to-no-op checklist

| Condition | Behaviour |
|---|---|
| no network | `timeout 5` on ls-remote, exit 0, bump failure counter |
| no `git` on PATH | skip permanently, record it |
| repo has no remote | skip permanently |
| remote requires auth | `BatchMode=yes` + `GIT_TERMINAL_PROMPT=0` ⇒ fails fast, never prompts |
| TTL not expired | no process spawned at all — the check is a file read |
| `channel: pinned` | check still runs (to *inform*) but never applies |

---

# 6. THREAT MODEL

> **Provenance note.** §§1–5 above were produced by a research agent working
> directly against the installed CLIs; its session was cut off before it could
> write §§6–8, which are reconstructed here from its reported findings plus
> independent verification. Claims it measured behaviourally are attributed as
> such; claims re-verified here are marked VERIFIED with the check used.

jedimem's premise — memory files living in the repo, delivered through the
instruction files agents auto-load — puts it on the wrong side of three security
boundaries at once. It ships executable content through a repo, it writes content
that becomes agent instructions, and it reads transcripts that contain secrets.
None of these is hypothetical, and two of them have no technical fix.

## 6.1 The table

| # | Threat | Vector | Impact | Mitigation | Residual |
|---|---|---|---|---|---|
| **T1** | Supply chain | malicious upstream commit runs at every teammate's session start | RCE, all machines | pin a tag/SHA, never a branch; no post-install scripts; no `curl \| sh` as the documented path; review the diff on update | **Real.** Pinning delays, never prevents. Whoever updates gets the new code. |
| **T2** | **Repo-borne RCE** | hostile PR edits the in-repo hook command; next session executes it | RCE, per-teammate | Codex: per-hook SHA-256 pinned in the *user's* config, flips to `modified` on any edit. Claude Code: **no equivalent** — see 6.2. `CODEOWNERS` on `.jedimem/**`; keep the hook to one reviewed script path | **Real and primary.** Every control collapses to "a human skimmed the diff." |
| **T3** | Prompt injection via memory | a memory file *is* an instruction, auto-loaded every session | agent takes attacker-chosen actions, persistently | human review gate; memories are data and can never grant capability or tool permission; provenance on every memory; `jedimem contest` | **Real, unsolved industry-wide.** See 6.3. |
| **T4** | Secret capture | a token in a transcript becomes a memory, then git history | permanent credential exposure | redact before *any* write incl. the staging ref; digest tool payloads, never store verbatim; `.jedimem/local/` never committed | **Real.** Redaction cannot work on prose — see 6.4. **Rotate, don't rewrite.** |
| **T5** | Surveillance | memory becomes a performance-review artifact | trust collapse, adoption failure | schema forbids a person as a memory's *subject*; personal kinds stay local; `jedimem pause`; `jedimem status`; no telemetry | Social. Git history is readable regardless. |
| **T6** | **Silent non-coverage** | workspace trust is fail-closed; project-scope plugins/hooks don't load untrusted | jedimem silently absent in CI and on fresh clones | detect and *report* rather than assume; `jedimem status` must state "not trusted here" | Low impact, high confusion cost if unreported. |
| **T7** | Update-check side channel | a per-session network call reveals repo/user activity | metadata leak, latency | 24 h jittered TTL, `async: true`, `git ls-remote` only, no API, no identifiers | Low. Must degrade to no-op offline. |

## 6.2 T2 in detail — the tension at the heart of the design

The research agent verified experimentally that **a committed
`.claude/settings.json` carrying `extraKnownMarketplaces` (relative `directory`
source) plus `enabledPlugins` causes an in-repo plugin's hooks to fire**, with
`CLAUDE_PLUGIN_ROOT` pointing into the working tree — no copy step, no install
command, no network. (VERIFIED here at the string level: `extraKnownMarketplaces`,
`enabledPlugins`, and `CLAUDE_PLUGIN_ROOT` are all present in the 2.1.241 binary.)

That is the zero-command install jedimem wants, and it is the same shape as pi's
committed `.pi/settings.json`. It is also, in the agent's words, **"a standing RCE
primitive"**: once the directory is trusted, editing the in-repo `hooks/hooks.json`
executes the new command on the next session — *no prompt, no hash check, no diff
shown*. The agent notes this is the behaviour class of CVE-2025-54136 ("MCPoison"),
except that here it is intended rather than a bug.

Codex is materially better: it pins each in-repo hook by SHA-256 in the **user's**
global config, so any edit flips it to `modified` and revokes trust until
re-approved. That is the design to prefer wherever we get to choose.

**The consequence is a genuine design conflict that a human has to settle.** The
stated product goal is that the plugin lives in the repo root so a teammate can
clone and go. The security finding says the opposite: ship the executable part
*outside* the repo, version-pinned, and keep only *data* in-repo — making
`--in-repo` an opt-in flag rather than the default.

Both are defensible, and the split is clean because **capture and delivery have
different risk profiles**:

| Component | In-repo? | Why |
|---|---|---|
| memory files (`.jedimem/memories/`) | **yes** | data, not code; the whole point; reviewed as a diff |
| compiled `AGENTS.md`/`CLAUDE.md` | **yes** | data; generated; CI-checked |
| config (`.jedimem/config.yml`) | **yes** | declarative, no execution |
| **capture hook script** | **contested** | executes on every teammate's machine |
| **plugin/extension code** | **contested** | executes on every teammate's machine |

A defensible middle path: **the in-repo hook is a fixed, tiny, reviewed shim that
only signals a daemon installed out-of-repo and version-pinned.** The repo then
contains no logic worth attacking — editing the shim buys an attacker a
`curl --unix-socket` call, not arbitrary payload execution. That preserves
clone-and-go for *data* while removing the standing RCE primitive for *code*.

This is flagged for decision, not decided here.

## 6.3 T3 in detail — why prompt injection has no fix

Memory content becomes agent instructions by design. That makes a malicious
memory an injected instruction with **persistence across every future session** —
strictly worse than a one-shot injection in a fetched web page.

Two aggravating factors the research agent surfaced:

- The industry has declined to own this class. Cursor and GitHub both formally
  treated instruction-file injection as not-a-vulnerability, and the "Rules File
  Backdoor" technique received no CVE.
- **Compiled memory is the one thing auto-reloaded after compaction.** Prior
  research established that no hook can inject after a compaction boundary —
  only `CLAUDE.md` is re-read. So the injection surface jedimem creates is
  precisely the one that survives context compaction, i.e. maximally persistent
  within a session.

What we can honestly claim: the human review gate is a real control, because
unlike a fetched web page a memory must pass a person before it is shared.
What we must not claim: that jedimem is safe against a teammate who approves a
poisoned diff.

**Design rules that follow:**
- A memory may never carry capability — no tool permissions, no allow-list
  entries, no hook definitions, no paths to execute. Memories are prose plus
  metadata, and the schema enforces it.
- Compiled output is generated by *our* code from *validated* frontmatter, never
  by pasting memory bodies into an instruction file unfiltered.
- Delimiters are ours; content outside them is the human's and always wins.

## 6.4 T4 in detail — why redaction is not a control

The research agent's finding here is the most useful negative result in the
document: **existing secret scanners are structurally unable to protect a memory
file.**

- gitleaks' default entropy threshold (3.5) discards human-chosen passwords —
  exactly the kind a developer pastes into a terminal.
- trufflehog's verification step needs a provider API per secret type, which
  gives it no opinion on anything bespoke.
- GitHub push protection covers, by its own documentation, a subset of the most
  identifiable patterns, and **skips paths that look like tests, mocks, or specs**.

None of these was built for what a memory file actually contains: prose,
hostnames, internal service names, business context, and half-quoted commands.

Therefore:

1. Redaction runs **before any write**, including to the staging ref — because a
   secret that reaches the side ref is already in the object store.
2. Tool payloads are **digested, never stored verbatim.** The prior project
   measured that transcripts carry full untruncated tool output; that is the
   richest secret source available and must never be copied wholesale.
3. The documentation must say plainly: **rotate, don't rewrite.** History rewrite
   is disruptive, incomplete (forks, clones, caches, forge-side copies), and slower
   than an attacker. Rotation is the only remedy that actually works.

## 6.5 T6 — fail-closed trust, reported honestly

The agent measured that **workspace trust is not granted under `-p`**, contrary to
`claude --help`, which says the trust dialog is "skipped". The correct reading is
that trust is *absent*, so project-scope plugins and hooks **silently do not load**.

The default is right — a headless agent should not auto-trust a repo it just
cloned. The problem is silence. jedimem must therefore:

- never assume its hooks are live; verify and report,
- make `jedimem status` state *"not trusted in this workspace — capture is off"*,
- and treat CI as a delivery-only environment: compiled `AGENTS.md` still works
  there, because it is just a file.

---

# 7. Config and secrets layering

Precedence, strongest last (later overrides earlier):

| Order | Layer | Location | Committed | Contains |
|---|---|---|---|---|
| 1 | repo defaults | `.jedimem/config.yml` | **yes** | `repo_id`, budgets, enabled kinds, compile targets |
| 2 | repo-generated | `.jedimem/compiled/`, delimited sections | **yes** | derived only; CI-checked for staleness |
| 3 | user, per-repo | `.jedimem/local/config.yml` | **no** (`.gitignore`) | model choice, batch window, pause state |
| 4 | user, global | `~/.config/jedimem/config.yml` | n/a | defaults across repos |
| 5 | machine | `.jedimem/local/machine.json` | **no** | machine id, offsets, queue |
| 6 | environment | `JEDIMEM_*` | n/a | overrides for CI and one-off runs |

Rules that make the layering safe:

- **A committed layer may never narrow a user's privacy.** Config can enable a
  memory *kind*; it can never disable `pause`, enable telemetry (there is none),
  or force a runtime that requires a key. A repo you clone must not be able to
  change what your machine sends.
- **Per-user ignore patterns go in `$(git rev-parse --git-common-dir)/info/exclude`**,
  not `.gitignore` — verified shared across all worktrees, and never committed, so
  a personal ignore is not imposed on teammates.
- **Per-worktree state** lives under `$(git rev-parse --git-dir)/jedimem/`;
  per-repo shared state under `--git-common-dir`.

## 7.1 Keys

**The default requires no key at all.** The extraction runtime borrows the host
agent's existing login (measured: `claude -p` inherits OAuth; cost and batching
consequences in `07-monitor-runtime.md`). This is the single most valuable
security property available to us, because a config field that holds a secret is a
field that ends up committed by someone, eventually.

If a user opts into direct-API mode:

- the key is read from the **environment only** — `ANTHROPIC_API_KEY` or an
  explicit `api_key_command` that shells out to a password manager;
- jedimem **never writes a key to a file it controls**, and never to any committed
  path;
- if a key is somehow found in a committed file, `jedimem lint` fails the build.

pi is the exception worth noting: it is BYO-key (`--api-key` "defaults to env
vars", provider defaults to `google`), so the zero-secret story does not extend to
using pi as the extraction runtime. Detect and fall back rather than prompt.

---

# 8. What we must refuse to do

Stated as commitments, because each is a thing a reasonable person will ask for:

1. **No `curl | sh` as the documented install path.** It defeats review, which is
   the only real control we have against T1.
2. **No floating version refs.** Pinned tag or SHA, always. `pi update --all`
   reconciling pinned refs is the model.
3. **No post-install scripts**, and nothing executed during install that was
   fetched during install.
4. **No writes outside** `.jedimem/`, the two delimited instruction sections, the
   three per-agent config files, and the staging ref.
5. **No memory that grants capability** — no tool permissions, no allow-lists, no
   hook definitions, no executable paths. Ever.
6. **No automatic commits to a checked-out branch**, and no automatic PRs.
7. **No telemetry**, not even aggregate, not even opt-out.
8. **No storing a key in any file jedimem writes.**
9. **No claiming redaction is a control.** Documentation says *rotate*.
10. **No silent failure.** If jedimem is not trusted, not compiled, or not
    capturing, `jedimem status` says so. The fail-open contract governs the
    *agent's* session, not our honesty about our own state.

## 8.1 The three risks a reader should leave with

1. **Repo-borne RCE via an approved PR (T2).** Unmitigated in Claude Code, which
   re-executes edited in-repo hooks with no re-prompt. Codex's SHA-256 pinning is
   the better model. **Decision required:** in-repo plugin (clone-and-go) versus
   out-of-repo pinned plugin (safer default) — see 6.2 for the proposed shim
   compromise.
2. **Prompt injection through memory content (T3).** No technical fix exists, and
   compiled memory is specifically the surface that survives compaction.
3. **Secret capture into git history (T4).** Redaction is structurally incapable
   for prose. Rotate, don't rewrite.
