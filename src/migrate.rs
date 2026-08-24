//! Keeping an existing repo valid as the format moves.
//!
//! Memory files are committed and live in git history forever, so a format
//! change is a **migration**, not a refactor. Three rules follow:
//!
//!   1. **Never auto-commit.** A migration rewrites tracked files. Doing that
//!      silently at session start would put a diff in front of someone who did
//!      not ask for one -- the same reason the capture daemon never commits.
//!      We write the files and tell the human; committing is their act.
//!   2. **Fail loudly when the binary is too old.** On a team, someone upgrades
//!      first and commits format N+1 memories. Everyone else's older binary must
//!      say "jedimem is out of date" rather than "unknown format" or, worse,
//!      quietly skipping memories it cannot parse.
//!   3. **Idempotent and ordered.** Migrations run oldest-first and re-running
//!      is a no-op, because they will be run from a hook on an unknown schedule.

use crate::config::Config;
use crate::memory::Memory;
use std::path::{Path, PathBuf};

/// The repo layout/format version this binary understands.
pub const SUPPORTED_FORMAT: u32 = 1;

pub struct Migration {
    /// The repo format version this upgrades *from*.
    pub from: u32,
    pub id: &'static str,
    pub description: &'static str,
    /// Returns the paths it changed. Must be safe to run twice.
    pub apply: fn(&Path, &Config) -> Result<Vec<String>, String>,
}

/// Registered migrations, ordered by `from`.
///
/// Empty at format 1 -- there is nothing older to upgrade from yet. The
/// machinery exists and is tested with a synthetic migration so that the first
/// real format change is a data-only change, not a scramble to build this.
pub const MIGRATIONS: &[Migration] = &[];

#[derive(Debug)]
pub enum Status {
    /// Repo matches this binary.
    UpToDate { format: u32 },
    /// Repo is older; these migrations would run.
    Pending { from: u32, ids: Vec<String> },
    /// Repo was written by a NEWER jedimem than this one.
    BinaryTooOld { repo_format: u32, supported: u32 },
    /// No .jedimem here.
    NotInitialised,
}

pub fn repo_format(root: &Path) -> Option<u32> {
    let cfg = root.join(".jedimem").join("config.yml");
    if !cfg.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&cfg).ok()?;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("format:") {
            return v.trim().parse().ok();
        }
    }
    Some(1) // a config without a stamp predates stamping
}

pub fn status(root: &Path) -> Status {
    let fmt = match repo_format(root) {
        Some(f) => f,
        None => return Status::NotInitialised,
    };
    if fmt > SUPPORTED_FORMAT {
        return Status::BinaryTooOld {
            repo_format: fmt,
            supported: SUPPORTED_FORMAT,
        };
    }
    let ids: Vec<String> = MIGRATIONS
        .iter()
        .filter(|m| m.from >= fmt && m.from < SUPPORTED_FORMAT)
        .map(|m| m.id.to_string())
        .collect();
    if ids.is_empty() && fmt == SUPPORTED_FORMAT {
        Status::UpToDate { format: fmt }
    } else if ids.is_empty() {
        // Version gap with no migration registered: still bump the stamp, but
        // say so rather than pretending nothing happened.
        Status::Pending {
            from: fmt,
            ids: vec![format!("stamp-only {} -> {}", fmt, SUPPORTED_FORMAT)],
        }
    } else {
        Status::Pending { from: fmt, ids }
    }
}

/// Detect memory files this binary cannot parse, which is the symptom a
/// teammate sees when they are behind. Returns (path, format) pairs.
pub fn unreadable_memories(root: &Path) -> Vec<(PathBuf, u32)> {
    let dir = root.join(crate::store::MEM_DIR);
    let mut out = Vec::new();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    paths.sort();
    for p in paths {
        if p.extension().map(|x| x != "md").unwrap_or(true) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&p) {
            if let Ok(m) = Memory::from_text(&text) {
                if m.format > crate::memory::FORMAT_VERSION {
                    out.push((p, m.format));
                }
            }
        }
    }
    out
}

fn set_format_stamp(root: &Path, version: u32) -> Result<(), String> {
    let path = root.join(".jedimem").join("config.yml");
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in text.lines() {
        if line.starts_with("format:") {
            lines.push(format!("format: {}", version));
            replaced = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !replaced {
        lines.insert(0, format!("format: {}", version));
    }
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).map_err(|e| e.to_string())
}

#[derive(Debug)]
pub struct Applied {
    pub ran: Vec<String>,
    pub changed: Vec<String>,
    pub from: u32,
    pub to: u32,
}

/// Run every pending migration. Writes files; never commits.
pub fn run(root: &Path, cfg: &Config, dry_run: bool) -> Result<Applied, String> {
    let from = match repo_format(root) {
        Some(f) => f,
        None => return Err("not initialised: run `jedimem init`".into()),
    };
    if from > SUPPORTED_FORMAT {
        return Err(format!(
            "this repo is at format {} but this jedimem only understands {}.\n\
             A teammate upgraded first. Run `jedimem update` to catch up --\n\
             do NOT downgrade the repo, and do not edit memories until you have.",
            from, SUPPORTED_FORMAT
        ));
    }
    let mut ran = Vec::new();
    let mut changed = Vec::new();
    for m in MIGRATIONS
        .iter()
        .filter(|m| m.from >= from && m.from < SUPPORTED_FORMAT)
    {
        ran.push(format!("{} ({})", m.id, m.description));
        if !dry_run {
            changed.extend((m.apply)(root, cfg)?);
        }
    }
    if !dry_run && from != SUPPORTED_FORMAT {
        set_format_stamp(root, SUPPORTED_FORMAT)?;
        changed.push(".jedimem/config.yml".into());
    }
    Ok(Applied {
        ran,
        changed,
        from,
        to: SUPPORTED_FORMAT,
    })
}
