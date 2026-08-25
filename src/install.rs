//! Wiring jedimem into the agents a developer actually has installed.
//!
//! Rules, each learned from the reverse-engineering in docs/research/:
//!
//!   * **Detect, never assume.** Install only for agents present; skip the rest
//!     and say so.
//!   * **Never clobber.** People have their own hooks. Claude Code and Codex
//!     both *concatenate* hook arrays across settings scopes, so appending is
//!     safe -- but duplicating is not, so we de-duplicate by command string.
//!   * **Idempotent.** Running install twice equals running it once.
//!   * **Offline.** Nothing here touches the network.
//!   * **Reversible.** `uninstall` removes exactly what we added and leaves the
//!     memories, which are the team's files, not ours.

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub const MARKER: &str = "jedimem";

#[derive(Debug, Clone, PartialEq)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    Pi,
}

impl AgentKind {
    pub fn name(&self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude-code",
            AgentKind::Codex => "codex",
            AgentKind::Pi => "pi",
        }
    }
    pub fn binary(&self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Pi => "pi",
        }
    }
    /// The in-repo, committable config file this agent reads.
    pub fn config_path(&self, root: &Path) -> PathBuf {
        match self {
            AgentKind::ClaudeCode => root.join(".claude").join("settings.json"),
            AgentKind::Codex => root.join(".codex").join("hooks.json"),
            AgentKind::Pi => root.join(".pi").join("settings.json"),
        }
    }
}

pub const ALL_AGENTS: [AgentKind; 3] = [AgentKind::ClaudeCode, AgentKind::Codex, AgentKind::Pi];

/// Is the agent's binary on PATH?
pub fn is_installed(a: &AgentKind) -> bool {
    which(a.binary()).is_some()
}

fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let c = dir.join(bin);
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

/// The one command every hook runs. Resolving the repo root at execution time
/// (rather than baking an absolute path) is what makes a committed hook work
/// across clones, machines and worktrees -- the idiom Codex's own examples use.
fn hook_command(event: &str) -> String {
    match event {
        "SessionStart" => "jedimem session-start --json 2>/dev/null || true".to_string(),
        other => format!(
            "jedimem capture {} 2>/dev/null || true",
            other.to_lowercase()
        ),
    }
}

fn jedimem_hook_entry(event: &str) -> Value {
    json!({
        "hooks": [{
            "type": "command",
            "command": hook_command(event),
            "statusMessage": MARKER,
            "timeout": 5
        }]
    })
}

/// Events we register. SessionStart is synchronous because it INJECTS context;
/// an async hook is fire-and-forget and cannot.
pub const EVENTS: [&str; 1] = ["SessionStart"];

fn read_json(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(&text).map_err(|e| {
        format!(
            "{} is not valid JSON ({}). jedimem will not overwrite a file it \
             cannot parse -- fix or move it, then re-run.",
            path.display(),
            e
        )
    })
}

fn write_json(path: &Path, v: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    std::fs::write(path, format!("{}\n", text)).map_err(|e| format!("{}: {}", path.display(), e))
}

fn is_ours(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains(MARKER))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Add our hook entries into a `hooks` map, replacing any previous jedimem
/// entry and leaving every other entry untouched.
fn merge_hooks(hooks: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    for event in EVENTS {
        let arr = hooks
            .entry(event.to_string())
            .or_insert_with(|| Value::Array(vec![]));
        let list = match arr.as_array_mut() {
            Some(l) => l,
            None => continue, // someone's hand-written config: leave it alone
        };
        let before = list.len();
        list.retain(|e| !is_ours(e)); // de-dupe: appending twice is the bug
        let removed = before != list.len();
        list.push(jedimem_hook_entry(event));
        changed = changed || removed || before == 0 || true;
    }
    changed
}

fn strip_hooks(hooks: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    let keys: Vec<String> = hooks.keys().cloned().collect();
    for k in keys {
        if let Some(list) = hooks.get_mut(&k).and_then(|v| v.as_array_mut()) {
            let before = list.len();
            list.retain(|e| !is_ours(e));
            if list.len() != before {
                changed = true;
            }
            if list.is_empty() {
                hooks.remove(&k);
            }
        }
    }
    changed
}

/// Register the merge driver for the generated instruction files.
///
/// Committed generated files conflict on every concurrent memory addition --
/// two teammates each regenerate the same block. Git merge drivers live in
/// per-clone config and are deliberately never cloned, so this must run per
/// clone, which is exactly when `jedimem install` runs.
pub fn register_merge_driver(root: &Path, targets: &[String]) -> Result<bool, String> {
    let set = |k: &str, v: &str| -> Result<(), String> {
        let out = std::process::Command::new("git")
            .args(["config", "--local", k, v])
            .current_dir(root)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    };
    set("merge.jedimem.name", "jedimem generated-section merge")?;
    set("merge.jedimem.driver", "jedimem merge-driver %A %O %B")?;

    // .gitattributes IS committed, so teammates inherit the intent; the driver
    // itself still has to be registered locally by their own `jedimem install`.
    let ga = root.join(".gitattributes");
    let cur = std::fs::read_to_string(&ga).unwrap_or_default();
    let mut lines: Vec<String> = cur.lines().map(String::from).collect();
    let mut added = false;
    for t in targets {
        let want = format!("{} merge=jedimem", t);
        if !lines.iter().any(|l| l.trim() == want) {
            if !added {
                lines.push(String::new());
                lines.push("# Generated by jedimem: regenerate on merge rather than".into());
                lines.push(
                    "# conflicting. Needs `jedimem install` in each clone to register".into(),
                );
                lines.push("# the driver -- git never clones merge drivers, by design.".into());
                added = true;
            }
            lines.push(want);
        }
    }
    if added {
        std::fs::write(
            &ga,
            format!("{}\n", lines.join("\n").trim_start_matches('\n')),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(added)
}

/// Git hooks that restore the canonical compiled state after git moves files.
///
/// The merge driver alone is not enough. Git does not guarantee that a newly
/// merged memory file is in the working tree *before* it merges the generated
/// file that summarises it, so a driver that regenerates from the worktree can
/// emit a section that is missing a teammate's just-merged memory. Observed,
/// not theorised.
///
/// Compiled output is derived, so the fix is simply to regenerate once git has
/// finished moving files. These hooks are per-clone (git never clones hooks),
/// which is again why `jedimem install` is the right place.
pub fn install_git_hooks(root: &Path) -> Result<Vec<String>, String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--git-path", "hooks"])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("could not locate hooks directory".into());
    }
    let dir = root.join(String::from_utf8_lossy(&out.stdout).trim());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let body = "#!/bin/sh\n\
                # jedimem: regenerate the compiled instruction files after git\n\
                # moves things around. Derived output, so this is always safe.\n\
                # Fail-open: never block a git operation.\n\
                command -v jedimem >/dev/null 2>&1 || exit 0\n\
                jedimem compile >/dev/null 2>&1 || true\n\
                exit 0\n";

    let mut written = Vec::new();
    for name in ["post-merge", "post-checkout", "post-rewrite"] {
        let path = dir.join(name);
        if path.exists() {
            let cur = std::fs::read_to_string(&path).unwrap_or_default();
            if cur.contains("jedimem") {
                continue; // already ours
            }
            // Never clobber someone's existing hook.
            if !cur.trim().is_empty() {
                written.push(format!("{} (skipped: yours)", name));
                continue;
            }
        }
        std::fs::write(&path, body).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
        written.push(name.to_string());
    }
    Ok(written)
}

#[derive(Debug)]
pub struct Report {
    pub agent: String,
    pub path: String,
    pub action: &'static str, // installed | updated | unchanged | removed | skipped
    pub note: String,
}

pub fn install_agent(a: &AgentKind, root: &Path, pin: &str) -> Result<Report, String> {
    let path = a.config_path(root);
    let before = std::fs::read_to_string(&path).unwrap_or_default();

    let mut doc = read_json(&path)?;
    let obj = doc
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", path.display()))?;

    match a {
        AgentKind::ClaudeCode | AgentKind::Codex => {
            let hooks = obj
                .entry("hooks".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            let map = hooks
                .as_object_mut()
                .ok_or_else(|| format!("{}: \"hooks\" is not an object", path.display()))?;
            merge_hooks(map);
        }
        AgentKind::Pi => {
            // pi provisions teammates automatically from committed project
            // settings, so the install is one pinned package reference.
            let pkgs = obj
                .entry("packages".to_string())
                .or_insert_with(|| Value::Array(vec![]));
            let list = pkgs
                .as_array_mut()
                .ok_or_else(|| format!("{}: \"packages\" is not an array", path.display()))?;
            let want = format!("git:github.com/jedi-technology/jedimem@{}", pin);
            list.retain(|v| {
                !v.as_str()
                    .map(|s| s.contains("jedi-technology/jedimem"))
                    .unwrap_or(false)
            });
            list.push(Value::String(want));
        }
    }

    write_json(&path, &doc)?;
    let after = std::fs::read_to_string(&path).unwrap_or_default();
    let action = if before.is_empty() {
        "installed"
    } else if before == after {
        "unchanged"
    } else {
        "updated"
    };
    Ok(Report {
        agent: a.name().to_string(),
        path: path.display().to_string(),
        action,
        note: String::new(),
    })
}

pub fn uninstall_agent(a: &AgentKind, root: &Path) -> Result<Report, String> {
    let path = a.config_path(root);
    if !path.exists() {
        return Ok(Report {
            agent: a.name().to_string(),
            path: path.display().to_string(),
            action: "skipped",
            note: "nothing installed".into(),
        });
    }
    let mut doc = read_json(&path)?;
    let mut changed = false;
    if let Some(obj) = doc.as_object_mut() {
        if let Some(map) = obj.get_mut("hooks").and_then(|h| h.as_object_mut()) {
            changed |= strip_hooks(map);
            if map.is_empty() {
                obj.remove("hooks");
            }
        }
        if let Some(list) = obj.get_mut("packages").and_then(|p| p.as_array_mut()) {
            let before = list.len();
            list.retain(|v| {
                !v.as_str()
                    .map(|s| s.contains("jedi-technology/jedimem"))
                    .unwrap_or(false)
            });
            changed |= list.len() != before;
            if list.is_empty() {
                obj.remove("packages");
            }
        }
        // If we created the file and nothing else is in it, remove it entirely
        // rather than leaving `{}` litter behind.
        if obj.is_empty() {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
            return Ok(Report {
                agent: a.name().to_string(),
                path: path.display().to_string(),
                action: "removed",
                note: "file was ours alone".into(),
            });
        }
    }
    if changed {
        write_json(&path, &doc)?;
    }
    Ok(Report {
        agent: a.name().to_string(),
        path: path.display().to_string(),
        action: if changed { "removed" } else { "unchanged" },
        note: String::new(),
    })
}
