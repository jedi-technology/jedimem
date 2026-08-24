//! Git access.
//!
//! Every write path here is deliberately conservative, because the measurements
//! in docs/research/04-git-format.md showed the obvious approaches lose data:
//!
//!   * `git add` + `git commit` from a background process lost 13 of 20
//!     memories to .git/index.lock contention, silently.
//!   * A lock-free commit onto the checked-out branch was then silently
//!     reverted by the user's next ordinary `git commit` (their index was
//!     stale), leaving spurious `D` entries in their status.
//!
//! So: we never touch the index, never advance refs/heads/*, and never write to
//! the working tree except when a human explicitly asks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const STAGING_REF: &str = "refs/jedimem/log";

#[derive(Debug)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for GitError {}

pub type R<T> = Result<T, GitError>;

fn base(cwd: Option<&Path>) -> Command {
    let mut c = Command::new("git");
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    c
}

/// Run git, returning (stdout, exit_ok).
pub fn run(args: &[&str], cwd: Option<&Path>) -> R<(String, bool)> {
    let out = base(cwd)
        .args(args)
        .output()
        .map_err(|e| GitError(format!("git not available: {}", e)))?;
    Ok((
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        out.status.success(),
    ))
}

pub fn check(args: &[&str], cwd: Option<&Path>) -> R<String> {
    let out = base(cwd)
        .args(args)
        .output()
        .map_err(|e| GitError(format!("git not available: {}", e)))?;
    if !out.status.success() {
        return Err(GitError(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn toplevel(cwd: Option<&Path>) -> R<PathBuf> {
    let (out, ok) = run(&["rev-parse", "--show-toplevel"], cwd)?;
    if !ok {
        return Err(GitError("not inside a git repository".into()));
    }
    Ok(PathBuf::from(out))
}

/// Per-worktree. The right home for per-checkout state.
pub fn git_dir(cwd: Option<&Path>) -> R<PathBuf> {
    Ok(PathBuf::from(check(
        &["rev-parse", "--absolute-git-dir"],
        cwd,
    )?))
}

/// Shared by all worktrees of a repo. The local scope key.
pub fn common_dir(cwd: Option<&Path>) -> R<PathBuf> {
    Ok(PathBuf::from(check(
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        cwd,
    )?))
}

pub fn worktrees(cwd: Option<&Path>) -> R<Vec<BTreeMap<String, String>>> {
    let out = check(&["worktree", "list", "--porcelain"], cwd)?;
    let mut trees = Vec::new();
    let mut cur: BTreeMap<String, String> = BTreeMap::new();
    for line in out.lines() {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                trees.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        cur.insert(k.to_string(), v.to_string());
    }
    if !cur.is_empty() {
        trees.push(cur);
    }
    Ok(trees)
}

pub fn head_commit(cwd: Option<&Path>) -> String {
    run(&["rev-parse", "HEAD"], cwd)
        .map(|(s, ok)| if ok { s } else { String::new() })
        .unwrap_or_default()
}

pub fn current_branch(cwd: Option<&Path>) -> String {
    run(&["rev-parse", "--abbrev-ref", "HEAD"], cwd)
        .map(|(s, ok)| if ok { s } else { String::new() })
        .unwrap_or_default()
}

/// Strip the incidental differences between spellings of the same remote.
pub fn normalized_remote(cwd: Option<&Path>) -> String {
    let (url, ok) = match run(&["remote", "get-url", "origin"], cwd) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    if !ok || url.is_empty() {
        return String::new();
    }
    let mut u = url.as_str();
    for pre in ["git+ssh://", "ssh://", "https://", "http://", "git://"] {
        if let Some(rest) = u.strip_prefix(pre) {
            u = rest;
        }
    }
    let mut s = u.to_string();
    if let Some(host) = s.split('/').next() {
        if host.contains('@') {
            s = s.split_once('@').map(|x| x.1).unwrap_or(&s).to_string();
        }
    }
    if !s.split(':').next().unwrap_or("").contains('/') {
        s = s.replacen(':', "/", 1);
    }
    if let Some(t) = s.strip_suffix(".git") {
        s = t.to_string();
    }
    s.trim_end_matches('/').to_lowercase()
}

/// NOT a stable identity: a --depth 1 clone yields a different root commit.
pub fn root_commit(cwd: Option<&Path>) -> String {
    match run(&["rev-list", "--max-parents=0", "HEAD"], cwd) {
        Ok((s, true)) => s.lines().next().unwrap_or("").to_string(),
        _ => String::new(),
    }
}

// ------------------------------------------------------------- staging ref

fn write_blob(content: &str, cwd: Option<&Path>) -> R<String> {
    use std::io::Write;
    let mut child = base(cwd)
        .args(["hash-object", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GitError(format!("git hash-object: {}", e)))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| GitError("no stdin".into()))?
        .write_all(content.as_bytes())
        .map_err(|e| GitError(e.to_string()))?;
    let out = child
        .wait_with_output()
        .map_err(|e| GitError(e.to_string()))?;
    if !out.status.success() {
        return Err(GitError(String::from_utf8_lossy(&out.stderr).into_owned()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn ref_exists(r: &str, cwd: Option<&Path>) -> String {
    match run(&["rev-parse", "--verify", "-q", r], cwd) {
        Ok((s, true)) => s,
        _ => String::new(),
    }
}

/// A temporary index path. NAME ONLY -- git refuses an existing zero-byte index
/// file, so we must not create it first.
fn temp_index_path() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "jedimem-idx-{}-{}",
        std::process::id(),
        memory_nonce()
    ));
    p
}

fn memory_nonce() -> String {
    let mut b = [0u8; 6];
    let _ = getrandom::getrandom(&mut b);
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn build_tree(files: &BTreeMap<String, String>, parent: &str, cwd: Option<&Path>) -> R<String> {
    let idx = temp_index_path();
    let idx_s = idx.to_string_lossy().into_owned();
    let result = (|| -> R<String> {
        if !parent.is_empty() {
            let out = base(cwd)
                .args(["read-tree", parent])
                .env("GIT_INDEX_FILE", &idx_s)
                .output()
                .map_err(|e| GitError(e.to_string()))?;
            if !out.status.success() {
                return Err(GitError(String::from_utf8_lossy(&out.stderr).into_owned()));
            }
        }
        for (path, blob) in files {
            let out = base(cwd)
                .args([
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{},{}", blob, path),
                ])
                .env("GIT_INDEX_FILE", &idx_s)
                .output()
                .map_err(|e| GitError(e.to_string()))?;
            if !out.status.success() {
                return Err(GitError(String::from_utf8_lossy(&out.stderr).into_owned()));
            }
        }
        let out = base(cwd)
            .arg("write-tree")
            .env("GIT_INDEX_FILE", &idx_s)
            .output()
            .map_err(|e| GitError(e.to_string()))?;
        if !out.status.success() {
            return Err(GitError(String::from_utf8_lossy(&out.stderr).into_owned()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    })();
    let _ = std::fs::remove_file(&idx);
    result
}

fn empty_tree(cwd: Option<&Path>) -> R<String> {
    check(&["hash-object", "-w", "-t", "tree", "--stdin"], cwd)
        .or_else(|_| Ok("4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string()))
}

/// Commit `files` onto a side ref without touching index/HEAD/worktree.
///
/// Compare-and-swap on the ref, retrying on contention. The parent MUST be read
/// before the tree is built: doing it the other way round passes the CAS while
/// committing a stale tree, which silently drops other writers' work.
pub fn stage_files(
    files: &BTreeMap<String, String>,
    message: &str,
    r: &str,
    cwd: Option<&Path>,
) -> R<String> {
    let mut blobs = BTreeMap::new();
    for (p, c) in files {
        blobs.insert(p.clone(), write_blob(c, cwd)?);
    }
    for _ in 0..100 {
        let parent = ref_exists(r, cwd);
        let tree = build_tree(&blobs, &parent, cwd)?;
        let commit = if parent.is_empty() {
            check(&["commit-tree", &tree, "-m", message], cwd)?
        } else {
            check(&["commit-tree", &tree, "-p", &parent, "-m", message], cwd)?
        };
        let (_, ok) = run(&["update-ref", r, &commit, &parent], cwd)?;
        if ok {
            return Ok(commit);
        }
    }
    Err(GitError(format!(
        "could not update {} after 100 attempts",
        r
    )))
}

pub fn staged_files(r: &str, cwd: Option<&Path>, prefix: &str) -> Vec<String> {
    if ref_exists(r, cwd).is_empty() {
        return vec![];
    }
    match run(&["ls-tree", "-r", "--name-only", r], cwd) {
        Ok((out, true)) => out
            .lines()
            .filter(|p| p.starts_with(prefix))
            .map(String::from)
            .collect(),
        _ => vec![],
    }
}

pub fn staged_content(path: &str, r: &str, cwd: Option<&Path>) -> R<String> {
    check(&["show", &format!("{}:{}", r, path)], cwd)
}

/// Rewrite the staging ref without `drop` (after promote/reject).
pub fn drop_staged(drop: &[String], r: &str, cwd: Option<&Path>, message: &str) -> R<String> {
    let mut keep: BTreeMap<String, String> = BTreeMap::new();
    for p in staged_files(r, cwd, "") {
        if !drop.contains(&p) {
            let c = staged_content(&p, r, cwd)?;
            keep.insert(p, format!("{}\n", c));
        }
    }
    let mut blobs = BTreeMap::new();
    for (p, c) in &keep {
        blobs.insert(p.clone(), write_blob(c, cwd)?);
    }
    let parent = ref_exists(r, cwd);
    let tree = if blobs.is_empty() {
        empty_tree(cwd)?
    } else {
        build_tree(&blobs, "", cwd)?
    };
    let commit = if parent.is_empty() {
        check(&["commit-tree", &tree, "-m", message], cwd)?
    } else {
        check(&["commit-tree", &tree, "-p", &parent, "-m", message], cwd)?
    };
    check(&["update-ref", r, &commit, &parent], cwd)?;
    Ok(commit)
}
