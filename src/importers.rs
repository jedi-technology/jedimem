//! Bootstrapping memory for an existing ("brownfield") repository.
//!
//! A mature repo is not a blank slate. It already encodes years of convention
//! in places nobody thinks of as memory: a hand-written CLAUDE.md, Cursor
//! rules, ADRs, a CODEOWNERS file, and a git history full of reverts that each
//! mark something the team tried and abandoned.
//!
//! Importing that is what makes jedimem useful on day one instead of day
//! ninety. Waiting to accumulate memories from live sessions means the tool is
//! worthless for exactly as long as it takes people to uninstall it.
//!
//! Design rules, all load-bearing:
//!
//!   * **Deterministic and offline.** No importer calls an LLM. A team must be
//!     able to run `jedimem import` on a private repo, read every proposal, and
//!     diff it -- with no model in the loop and nothing leaving the machine.
//!   * **Everything lands as `proposed`.** Import is a suggestion engine, never
//!     an author. A hand-written rule was written by a human, but it was not
//!     reviewed *as a memory*, and the two are different acts.
//!   * **Idempotent.** Re-running imports nothing new.
//!   * **Traceable.** Every imported memory records the file and line it came
//!     from, so `jedimem why` answers honestly.

use crate::memory::{content_hash, Memory, NewMemory};
use crate::redact::redact;
use crate::repo;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Files that already hold agent instructions.
const INSTRUCTION_FILES: [&str; 8] = [
    "CLAUDE.md",
    "AGENTS.md",
    "GEMINI.md",
    "CONVENTIONS.md",
    ".windsurfrules",
    ".clinerules",
    ".rules",
    ".github/copilot-instructions.md",
];

/// Rule *directories*. Teams outgrow a single CLAUDE.md and split it up; an
/// import that misses these misses most of the actual content. (Measured: 24
/// candidates vs 554 on a real repo.)
const INSTRUCTION_DIRS: [(&str, &str); 7] = [
    (".cursor/rules", "mdc"),
    (".github/instructions", "md"),
    (".clinerules", "md"),
    (".claude/rules", "md"),
    (".agents/rules", "md"),
    (".codex/rules", "md"),
    ("docs/conventions", "md"),
];

const ADR_DIRS: [&str; 6] = [
    "docs/adr",
    "docs/decisions",
    "doc/adr",
    "adr",
    "docs/architecture/decisions",
    "docs/rfc",
];

const MIN_LEN: usize = 25;
const MAX_LEN: usize = 600;

struct KindRule {
    kind: &'static str,
    re: Regex,
}

fn kind_rules() -> &'static Vec<KindRule> {
    static R: OnceLock<Vec<KindRule>> = OnceLock::new();
    R.get_or_init(|| {
        let mk = |kind: &'static str, p: &str| KindRule {
            kind,
            re: Regex::new(p).expect("static kind pattern must compile"),
        };
        // Deliberately biased toward `convention`, the least dangerous kind to
        // get wrong: a wrong convention costs one non-idiomatic patch, whereas
        // a wrong `requirement` gets treated as law.
        vec![
            mk("requirement", r"(?i)\b(must|shall|is required|are required|mandatory)\b"),
            mk("constraint", r"(?i)\b(never|forbidden|do not ever|under no circumstances|security|compliance|gdpr|pci|hipaa)\b"),
            // Word-anchored on BOTH sides: an unanchored `\brun` matched
            // "cross-runtime" and misfiled repo-map entries as workflow.
            mk("workflow", r"(?i)\b(run|runs|command|commands|before committing|to deploy|to test|npm|yarn|pnpm|make|pytest|cargo)\b"),
            mk("style", r"(?i)\b(format|indent|quote|naming|lint|prettier|eslint|line length|semicolon)\b"),
            mk("gotcha", r"(?i)\b(gotcha|careful|beware|note that|watch out|common mistake|footgun)\b"),
            mk("negative", r"(?i)\b(we tried|don't use|do not use|avoid|deprecated|instead of|rather than)\b"),
        ]
    })
}

fn topic_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^`?[\w@/.-]+/`?\s*[-\u{2014}:]").expect("static topic pattern must compile")
    })
}

fn bullet_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^\s*([-*+]|\d+[.)])\s+").expect("static bullet pattern must compile")
    })
}

fn generated_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?s)<!-- BEGIN jedimem -->.*?<!-- END jedimem -->")
            .expect("static generated-section pattern must compile")
    })
}

/// `bullet` is the line on its own; `context` adds the enclosing heading.
///
/// The split matters: the repo-map check is `^`-anchored, so it must see the
/// bullet alone. Prefixing the heading (as an earlier version did) silently
/// disabled it and filed every layout entry as a convention.
fn infer_kind(bullet: &str, context: &str) -> &'static str {
    // A repo-map entry describes where code lives; it is not an instruction.
    // Checked first, because such lines often contain incidental verbs.
    if topic_re().is_match(bullet) {
        return "topic";
    }
    let combined = format!("{} {}", context, bullet);
    for r in kind_rules() {
        if r.re.is_match(&combined) {
            return r.kind;
        }
    }
    "convention"
}

fn clean(text: &str) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = bullet_re().replace(&t, "").to_string();
    t.trim().to_string()
}

fn usable(text: &str) -> bool {
    let n = text.chars().count();
    if !(MIN_LEN..=MAX_LEN).contains(&n) {
        return false;
    }
    if text.starts_with('#')
        || text.starts_with("```")
        || text.starts_with('|')
        || text.starts_with("<!--")
    {
        return false;
    }
    let lower = text.to_lowercase();
    for p in ["todo", "tbd", "wip", "see also", "table of contents"] {
        if lower.starts_with(p) {
            return false;
        }
    }
    true
}

fn frontmatter(text: &str) -> (BTreeMap<String, String>, String) {
    if !text.starts_with("---") {
        return (BTreeMap::new(), text.to_string());
    }
    let rest = text.trim_start_matches("---").trim_start_matches('\n');
    match rest.split_once("\n---") {
        Some((fm, body)) => {
            let mut map = BTreeMap::new();
            for line in fm.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    map.insert(
                        k.trim().to_string(),
                        v.trim().trim_matches('"').trim_matches('\'').to_string(),
                    );
                }
            }
            (map, body.trim_start_matches('\n').to_string())
        }
        None => (BTreeMap::new(), text.to_string()),
    }
}

/// Cursor `.mdc` uses `globs:`; Copilot instructions use `applyTo:`.
fn scope_from_frontmatter(fm: &BTreeMap<String, String>) -> String {
    for key in ["globs", "applyTo", "apply_to"] {
        if let Some(v) = fm.get(key) {
            let first = v
                .split(',')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches(|c| c == '[' || c == ']' || c == '\'' || c == '"');
            if !first.is_empty() && first != "**/*" {
                return first.to_string();
            }
        }
    }
    "**".to_string()
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn collect_files(root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = INSTRUCTION_FILES
        .iter()
        .map(|f| root.join(f))
        .filter(|p| p.is_file())
        .collect();
    for (dir, ext) in INSTRUCTION_DIRS {
        let d = root.join(dir);
        if !d.is_dir() {
            continue;
        }
        let mut stack = vec![d];
        while let Some(cur) = stack.pop() {
            if let Ok(rd) = std::fs::read_dir(&cur) {
                let mut entries: Vec<PathBuf> =
                    rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
                entries.sort();
                for p in entries {
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().map(|x| x == ext).unwrap_or(false) {
                        paths.push(p);
                    }
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

/// Existing agent-instruction files -- the highest-value import by far.
pub fn from_instructions(root: &Path) -> Vec<Memory> {
    let mut found = Vec::new();
    for p in collect_files(root) {
        let relp = rel(root, &p);
        let raw = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(_) => continue,
        };
        // Never re-import our own generated output: that is the file-level
        // version of the self-amplification loop.
        let text = generated_re().replace_all(&raw, "").into_owned();
        let (fm, body) = frontmatter(&text);
        let default_scope = scope_from_frontmatter(&fm);

        let mut section = String::new();
        for (i, line) in body.lines().enumerate() {
            let line = line.trim_end();
            if line.starts_with('#') {
                section = line.trim_start_matches('#').trim().to_string();
                continue;
            }
            if !bullet_re().is_match(line) {
                continue;
            }
            let t = clean(line);
            if !usable(&t) {
                continue;
            }
            let kind = infer_kind(&t, &section);
            let body_md =
                if !section.is_empty() && !t.to_lowercase().contains(&section.to_lowercase()) {
                    format!(
                        "{}\n\n**Context:** {} (imported from `{}`)",
                        t, section, relp
                    )
                } else {
                    t.clone()
                };
            if let Ok(m) = Memory::create(NewMemory {
                kind,
                body: &body_md,
                scope: &default_scope,
                source: "import",
                evidence: &format!("{}:{}", relp, i + 1),
                confirmed_by: "human",
                ..Default::default()
            }) {
                found.push(m);
            }
        }
    }
    found
}

/// Architecture Decision Records: the pre-AI form of exactly this idea.
pub fn from_adrs(root: &Path) -> Vec<Memory> {
    static STATUS: OnceLock<Regex> = OnceLock::new();
    static DECISION: OnceLock<Regex> = OnceLock::new();
    let status_re = STATUS.get_or_init(|| {
        Regex::new(r"(?im)^\s*(?:##\s*)?status\s*:?\s*\n?\s*(\w+)")
            .expect("static ADR status pattern must compile")
    });
    let decision_re = DECISION.get_or_init(|| {
        Regex::new(r"(?is)##\s*decision\s*\n(.+?)(?:\n##|\z)")
            .expect("static ADR decision pattern must compile")
    });
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for d in ADR_DIRS {
        let dir = root.join(d);
        if !dir.is_dir() {
            continue;
        }
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        paths.sort();
        for p in paths {
            if !p.is_file()
                || p.extension().map(|x| x != "md").unwrap_or(true)
                || !seen.insert(p.clone())
            {
                continue;
            }
            let relp = rel(root, &p);
            let text = match std::fs::read_to_string(&p) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let (_, body) = frontmatter(&text);
            let title = body
                .lines()
                .find(|l| l.starts_with('#'))
                .map(|l| l.trim_start_matches('#').trim().to_string())
                .unwrap_or_default();
            let status = status_re
                .captures(&body)
                .and_then(|c| c.get(1).map(|m| m.as_str().to_lowercase()))
                .unwrap_or_default();
            // Superseded/rejected ADRs are still knowledge -- as `negative`.
            let kind = if matches!(status.as_str(), "superseded" | "rejected" | "deprecated") {
                "negative"
            } else {
                "decision"
            };
            let mut decision = decision_re
                .captures(&body)
                .and_then(|c| c.get(1).map(|m| clean(m.as_str())))
                .unwrap_or_default();
            if decision.is_empty() {
                let no_title: String = body.lines().skip(1).collect::<Vec<_>>().join(" ");
                decision = clean(&no_title);
            }
            decision = decision.chars().take(MAX_LEN).collect();
            if decision.chars().count() < MIN_LEN {
                continue;
            }
            let mut b = if title.is_empty() {
                decision
            } else {
                format!("{}: {}", title, decision)
            };
            if !status.is_empty() {
                b.push_str(&format!("\n\n**Status:** {} (ADR `{}`)", status, relp));
            }
            if let Ok(m) = Memory::create(NewMemory {
                kind,
                body: &b,
                source: "import",
                evidence: &relp,
                confirmed_by: "human",
                ..Default::default()
            }) {
                out.push(m);
            }
        }
    }
    out
}

/// CODEOWNERS: who to ask about a path, already machine-readable.
pub fn from_codeowners(root: &Path) -> Vec<Memory> {
    for cand in ["CODEOWNERS", ".github/CODEOWNERS", "docs/CODEOWNERS"] {
        let p = root.join(cand);
        if !p.is_file() {
            continue;
        }
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let mut out = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            let pattern = parts[0];
            let owners = parts[1..].join(", ");
            let scope = pattern.trim_start_matches('/');
            if let Ok(m) = Memory::create(NewMemory {
                kind: "ownership",
                body: &format!(
                    "Changes to `{}` are owned by {}. Ask them for review or context.",
                    pattern, owners
                ),
                scope: if scope.is_empty() { "**" } else { scope },
                source: "import",
                evidence: &format!("{}:{}", cand, i + 1),
                confirmed_by: "human",
                ..Default::default()
            }) {
                out.push(m);
            }
        }
        return out;
    }
    vec![]
}

/// Reverts are the cheapest negative knowledge a repo contains.
///
/// A revert is a durable, machine-readable record that the team tried something
/// and backed it out. Nobody writes those down, and every team relitigates them.
pub fn from_git_history(root: &Path) -> Vec<Memory> {
    let out = match repo::run(
        &[
            "log",
            "-2000",
            "--no-merges",
            "--format=%H%x00%s%x00%b%x00%an%x1e",
        ],
        Some(root),
    ) {
        Ok((s, true)) => s,
        _ => return vec![],
    };
    static REVERT: OnceLock<Regex> = OnceLock::new();
    static STRIP: OnceLock<Regex> = OnceLock::new();
    let revert_re = REVERT.get_or_init(|| {
        Regex::new(r#"(?i)^revert\b|^revert ""#).expect("static revert pattern must compile")
    });
    let strip_re = STRIP.get_or_init(|| {
        Regex::new(r#"(?i)^revert\s*"?"#).expect("static revert-strip pattern must compile")
    });
    let mut mems = Vec::new();
    for rec in out.split('\x1e') {
        let rec = rec.trim_matches('\n');
        if rec.is_empty() {
            continue;
        }
        let parts: Vec<&str> = rec.split('\x00').collect();
        if parts.len() < 3 {
            continue;
        }
        let (sha, subject, body) = (parts[0], parts[1], parts[2]);
        if !revert_re.is_match(subject) {
            continue;
        }
        let reverted = strip_re
            .replace(subject, "")
            .trim_end_matches('"')
            .to_string();
        let reason = body
            .lines()
            .map(|l| l.trim())
            .find(|l| {
                !l.is_empty()
                    && !l.to_lowercase().starts_with("this reverts")
                    && !l.to_lowercase().starts_with("co-authored")
                    && !l.to_lowercase().starts_with("signed-off")
            })
            .unwrap_or("");
        let short: String = sha.chars().take(8).collect();
        let mut b = format!("Reverted: {}.", reverted);
        if !reason.is_empty() {
            b.push_str(&format!("\n\n**Reason given:** {}", reason));
        }
        b.push_str(&format!(
            "\n\n**Why this is here:** a revert records something the team tried and \
             backed out. Confirm it still applies before relying on it (commit `{}`).",
            short
        ));
        if b.chars().count() > MAX_LEN * 2 {
            continue;
        }
        if let Ok(m) = Memory::create(NewMemory {
            kind: "negative",
            body: &b,
            source: "import",
            commit: &sha.chars().take(12).collect::<String>(),
            evidence: &format!("commit {}", short),
            confirmed_by: "agent",
            ..Default::default()
        }) {
            mems.push(m);
        }
    }
    mems
}

pub const SOURCES: [(&str, &str); 4] = [
    (
        "adr",
        "ADRs: accepted become decisions, superseded become negative knowledge",
    ),
    (
        "codeowners",
        "CODEOWNERS: who to ask about a path, scoped to their files",
    ),
    ("git", "revert commits: what the team tried and backed out"),
    (
        "instructions",
        "CLAUDE.md, AGENTS.md, .cursor/rules, .claude/rules, copilot-instructions",
    ),
];

#[derive(Debug, Default, Clone)]
pub struct SourceStats {
    pub found: usize,
    pub new: usize,
    pub duplicate: usize,
    pub redacted: usize,
}

pub fn run_import(
    root: &Path,
    sources: &[String],
    existing: &BTreeMap<String, Memory>,
    limit: usize,
) -> Result<(Vec<Memory>, BTreeMap<String, SourceStats>), String> {
    let mut seen: BTreeSet<String> = existing.keys().cloned().collect();
    let mut out: Vec<Memory> = Vec::new();
    let mut stats: BTreeMap<String, SourceStats> = BTreeMap::new();

    for name in sources {
        let got = match name.as_str() {
            "instructions" => from_instructions(root),
            "adr" => from_adrs(root),
            "codeowners" => from_codeowners(root),
            "git" => from_git_history(root),
            other => {
                let names: Vec<&str> = SOURCES.iter().map(|(n, _)| *n).collect();
                return Err(format!(
                    "unknown import source {:?}; choose from {}",
                    other,
                    names.join(", ")
                ));
            }
        };
        let mut st = SourceStats {
            found: got.len(),
            ..Default::default()
        };
        for mut m in got {
            let (cleaned, labels) = redact(&m.body);
            if !labels.is_empty() {
                st.redacted += 1;
                m.body = cleaned;
                m.extra.insert("redacted".into(), labels.join(","));
            }
            let h = content_hash(&m.body);
            if seen.contains(&h) {
                st.duplicate += 1;
                continue;
            }
            seen.insert(h);
            out.push(m);
            st.new += 1;
            if limit > 0 && out.len() >= limit {
                break;
            }
        }
        stats.insert(name.clone(), st);
        if limit > 0 && out.len() >= limit {
            break;
        }
    }
    Ok((out, stats))
}
