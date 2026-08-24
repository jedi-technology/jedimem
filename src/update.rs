//! Update checking.
//!
//! Constraints, all from docs/research/05-distribution-security.md §5:
//!
//!   * **Never in the agent's critical path.** The check runs detached; the
//!     session never waits on it. A memory tool that adds latency to every
//!     prompt gets uninstalled.
//!   * **Not the GitHub API.** Unauthenticated REST is 60/hr *per IP*, and
//!     conditional `304 Not Modified` responses still decrement the quota. A
//!     team behind one NAT would exhaust it. `git ls-remote` has its own budget
//!     and needs no token.
//!   * **TTL-gated with jitter.** One check per day per machine, jittered so a
//!     team that all start work at 09:00 does not stampede.
//!   * **No-op offline.** No prompting, no hanging, no error in the user's face.
//!   * **Never auto-installs.** We tell; the human decides. Applying an update
//!     silently would mean arbitrary new code executing on a machine whose owner
//!     did not choose the moment -- threat T1.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub const UPSTREAM: &str = "https://github.com/jedi-technology/jedimem";
const TTL_SECS: u64 = 24 * 60 * 60;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn cache_path() -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| Path::new(&h).join(".cache"))
                .unwrap_or_else(|_| std::env::temp_dir())
        });
    base.join("jedimem").join("update.txt")
}

#[derive(Debug, Default, Clone)]
pub struct Cached {
    pub checked_at: u64,
    pub latest: String,
    pub error: String,
}

pub fn read_cache() -> Cached {
    let mut c = Cached::default();
    if let Ok(text) = std::fs::read_to_string(cache_path()) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                match k {
                    "checked_at" => c.checked_at = v.trim().parse().unwrap_or(0),
                    "latest" => c.latest = v.trim().to_string(),
                    "error" => c.error = v.trim().to_string(),
                    _ => {}
                }
            }
        }
    }
    c
}

fn write_cache(c: &Cached) {
    let p = cache_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        &p,
        format!(
            "checked_at={}\nlatest={}\nerror={}\n",
            c.checked_at, c.latest, c.error
        ),
    );
}

/// Deterministic per-machine jitter, so a team starting work together does not
/// all hit the remote in the same minute.
fn jitter(seed: &str) -> u64 {
    let h = seed
        .bytes()
        .fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
    h % (4 * 60 * 60) // up to 4h
}

fn machine_seed() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| "jedimem".into())
}

pub fn is_stale(c: &Cached) -> bool {
    now().saturating_sub(c.checked_at) > TTL_SECS + jitter(&machine_seed())
}

/// Parse a `vMAJOR.MINOR.PATCH` tag into comparable parts.
fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let t = v.trim().trim_start_matches('v');
    let mut it = t.split(['.', '-', '+']);
    let a = it.next()?.parse().ok()?;
    let b = it.next().unwrap_or("0").parse().ok()?;
    let c = it.next().unwrap_or("0").parse().ok()?;
    Some((a, b, c))
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// Ask the remote for its newest release tag. Blocking; callers decide where.
pub fn fetch_latest_tag(remote: &str) -> Result<String, String> {
    let out = Command::new("git")
        .args(["ls-remote", "--tags", "--refs", "--sort=-v:refname", remote])
        // Never prompt, never hang waiting for credentials on a private remote.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -oBatchMode=yes -oStrictHostKeyChecking=accept-new",
        )
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git unavailable: {}", e))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr)
            .lines()
            .next()
            .unwrap_or("ls-remote failed")
            .to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut best: Option<(u32, u32, u32)> = None;
    let mut best_tag = String::new();
    for line in text.lines() {
        if let Some(tag) = line.split("refs/tags/").nth(1) {
            if let Some(v) = parse_version(tag) {
                if best.is_none() || Some(v) > best {
                    best = Some(v);
                    best_tag = tag.trim().to_string();
                }
            }
        }
    }
    if best_tag.is_empty() {
        Err("remote has no version tags".into())
    } else {
        Ok(best_tag)
    }
}

/// Perform the check and record the result. Errors are cached too, so an
/// offline machine does not retry on every single session.
pub fn check_now(remote: &str) -> Cached {
    let mut c = Cached {
        checked_at: now(),
        ..Default::default()
    };
    match fetch_latest_tag(remote) {
        Ok(tag) => c.latest = tag,
        Err(e) => c.error = e.replace('\n', " "),
    }
    write_cache(&c);
    c
}

/// Spawn the check as a detached child and return immediately.
///
/// This is what a SessionStart hook calls. The parent exits at once; the child
/// is reparented to init and writes the cache for *next* session to read. The
/// current session is never delayed, which is the whole point.
pub fn spawn_background_check() {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return,
    };
    // Claim the TTL window *before* spawning. Otherwise every session started
    // before the child finishes writing sees a stale cache and spawns another
    // checker. The child overwrites this with the real result.
    let mut claim = read_cache();
    claim.checked_at = now();
    write_cache(&claim);
    let _ = Command::new(exe)
        .args(["update", "--check-only", "--quiet"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
