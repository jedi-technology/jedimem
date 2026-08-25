//! The jedimem command line.
//!
//! Arg parsing is hand-rolled. The surface is small, and `clap` would add a
//! dependency tree that executes on every teammate's machine -- supply chain is
//! threat T1 in our own SECURITY.md.

use jedimem::compiler;
use jedimem::config::{self, Config};
use jedimem::importers::{self, SOURCES};
use jedimem::install;
use jedimem::memory::{kind_info, ulid, Memory, KINDS, LOCAL_ONLY_KINDS};
use jedimem::migrate;
use jedimem::repo;
use jedimem::store::Store;
use jedimem::update;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// ------------------------------------------------------------------ styling

struct Style {
    bold: &'static str,
    dim: &'static str,
    red: &'static str,
    grn: &'static str,
    yel: &'static str,
    off: &'static str,
}

fn style() -> Style {
    let plain = std::env::var("NO_COLOR").is_ok() || !is_tty();
    if plain {
        Style {
            bold: "",
            dim: "",
            red: "",
            grn: "",
            yel: "",
            off: "",
        }
    } else {
        Style {
            bold: "\x1b[1m",
            dim: "\x1b[2m",
            red: "\x1b[31m",
            grn: "\x1b[32m",
            yel: "\x1b[33m",
            off: "\x1b[0m",
        }
    }
}

#[cfg(unix)]
fn is_tty() -> bool {
    // Avoid a libc dependency for one check: if stdout is a terminal, writing
    // to /dev/tty succeeds. Cheap and good enough for colour selection.
    std::env::var("TERM").map(|t| t != "dumb").unwrap_or(false) && Path::new("/dev/tty").exists()
}
#[cfg(not(unix))]
fn is_tty() -> bool {
    false
}

// -------------------------------------------------------------- arg parsing

#[derive(Default)]
struct Args {
    cmd: String,
    positional: Vec<String>,
    flags: Vec<String>,
    opts: BTreeMap<String, Vec<String>>,
}

impl Args {
    fn has(&self, f: &str) -> bool {
        self.flags.iter().any(|x| x == f)
    }
    fn many(&self, k: &str) -> Vec<String> {
        self.opts.get(k).cloned().unwrap_or_default()
    }
    fn one(&self, k: &str) -> Option<String> {
        self.opts.get(k).and_then(|v| v.first().cloned())
    }
    fn num(&self, k: &str, default: usize) -> usize {
        self.one(k).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
}

/// Options that take a value. Everything else is a boolean flag.
/// Options that take a value.
const VALUE_OPTS: [&str; 10] = [
    "--root",
    "--from",
    "--limit",
    "--show",
    "--approve",
    "--reject",
    "--kind",
    "--remote",
    "--agent",
    "--pin",
];

/// Boolean flags. Anything not in either list is a typo, and a typo must fail
/// loudly: silently treating `--agnet codex` as a no-op flag is how a user ends
/// up believing they scoped a command that actually ran against everything.
const BOOL_FLAGS: [&str; 12] = [
    "--all",
    "--check",
    "--dry-run",
    "--stage",
    "--commit",
    "--list-sources",
    "--approve-all",
    "--force",
    "--json",
    "--quiet",
    "--check-only",
    "--uninstall",
];

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut i = 0;
    while i < argv.len() {
        let t = &argv[i];
        if let Some(stripped) = t.strip_prefix("--") {
            let (name, inline) = match stripped.split_once('=') {
                Some((n, v)) => (format!("--{}", n), Some(v.to_string())),
                None => (t.clone(), None),
            };
            if VALUE_OPTS.contains(&name.as_str()) {
                let val = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        argv.get(i)
                            .cloned()
                            .ok_or_else(|| format!("{} needs a value", name))?
                    }
                };
                a.opts
                    .entry(name.trim_start_matches("--").to_string())
                    .or_default()
                    .push(val);
            } else if BOOL_FLAGS.contains(&name.as_str()) {
                a.flags.push(name);
            } else {
                return Err(format!("unknown option {}", name));
            }
        } else if a.cmd.is_empty() {
            a.cmd = t.clone();
        } else {
            a.positional.push(t.clone());
        }
        i += 1;
    }
    Ok(a)
}

// ------------------------------------------------------------------ context

struct Ctx {
    root: PathBuf,
    cfg: Config,
    store: Store,
}

fn ctx(a: &Args) -> Result<Ctx, String> {
    let root = match a.one("root") {
        Some(r) => PathBuf::from(r)
            .canonicalize()
            .map_err(|e| format!("--root: {}", e))?,
        None => repo::toplevel(None).map_err(|_| {
            "not inside a git repository\n\
             jedimem stores memory in git. Run `git init` first, or pass --root."
                .to_string()
        })?,
    };
    let cfg = config::load(&root);
    let store = Store::new(&root, cfg.get("staging_ref"));
    Ok(Ctx { root, cfg, store })
}

/// Resolve a user-typed handle. Exact id, then suffix, then prefix.
///
/// Suffix first is deliberate: the first 10 chars of a ULID are the millisecond
/// timestamp, so same-batch memories share them and a prefix cannot disambiguate.
fn match_ids(ids: &[String], want: &str) -> Vec<String> {
    let want = want.trim().to_uppercase();
    if ids.contains(&want) {
        return vec![want];
    }
    let suffix: Vec<String> = ids.iter().filter(|i| i.ends_with(&want)).cloned().collect();
    if !suffix.is_empty() {
        return suffix;
    }
    ids.iter()
        .filter(|i| i.starts_with(&want))
        .cloned()
        .collect()
}

// ----------------------------------------------------------------- commands

fn cmd_init(a: &Args, c: &Ctx) -> Result<i32, String> {
    let s = style();
    let d = c.root.join(".jedimem");
    if d.join("config.yml").exists() && !a.has("--force") {
        println!(
            "already initialised at {}  (use --force to overwrite config)",
            d.display()
        );
        return Ok(0);
    }
    std::fs::create_dir_all(d.join("memories")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(d.join("local")).map_err(|e| e.to_string())?;
    let rid = ulid();
    let cfg_text = format!(
        "# jedimem repo configuration -- committed and shared.\nformat: 1\n\n\
         # Authoritative repo identity. Written once, never derived: a root-commit\n\
         # hash changes under a shallow clone and a remote URL has several spellings.\n\
         repo_id: {}\n\n\
         budgets:\n  always_chars: 6000\n  scoped_chars_per_glob: 4000\n\n\
         compile:\n  targets: [AGENTS.md, CLAUDE.md]\n  \
         marker_begin: \"<!-- BEGIN jedimem -->\"\n  marker_end: \"<!-- END jedimem -->\"\n\n\
         capture:\n  batch_window_minutes: 30\n  staging_ref: refs/jedimem/log\n",
        rid
    );
    std::fs::write(d.join("config.yml"), cfg_text).map_err(|e| e.to_string())?;

    let gi = c.root.join(".gitignore");
    let cur = std::fs::read_to_string(&gi).unwrap_or_default();
    if !cur.contains(".jedimem/local/") {
        let next = format!(
            "{}\n\n# jedimem per-user, per-machine state\n.jedimem/local/\n",
            cur.trim_end_matches('\n')
        );
        std::fs::write(&gi, next.trim_start_matches('\n')).map_err(|e| e.to_string())?;
    }
    println!("{}initialised{} {}", s.grn, s.off, d.display());
    println!("  repo_id: {}", rid);
    println!(
        "\nNext: {}jedimem import{} to see what this repo already knows.",
        s.bold, s.off
    );
    Ok(0)
}

fn cmd_import(a: &Args, c: &Ctx) -> Result<i32, String> {
    let s = style();
    if a.has("--list-sources") {
        println!("import sources:");
        for (name, doc) in SOURCES {
            println!("  {:14} {}", name, doc);
        }
        return Ok(0);
    }
    let sources: Vec<String> = {
        let picked = a.many("from");
        if picked.is_empty() {
            SOURCES.iter().map(|(n, _)| n.to_string()).collect()
        } else {
            picked
        }
    };
    let existing = c.store.hashes();
    let (mems, stats) = importers::run_import(&c.root, &sources, &existing, a.num("limit", 0))?;

    println!("{}Scanned{} {}", s.bold, s.off, c.root.display());
    for name in &sources {
        if let Some(st) = stats.get(name) {
            let extra = if st.redacted > 0 {
                format!(", {} redacted", st.redacted)
            } else {
                String::new()
            };
            println!(
                "  {:14} {:4} found  {}{:4} new{}  {}{} dup{}{}",
                name, st.found, s.grn, st.new, s.off, s.dim, st.duplicate, extra, s.off
            );
        }
    }
    if mems.is_empty() {
        println!(
            "\n{}Nothing new to import.{} Either this repo has no instruction files, \
             ADRs, CODEOWNERS or reverts, or they are already imported.",
            s.yel, s.off
        );
        return Ok(0);
    }

    // --- two warnings that matter more than the listing itself --------------

    // 1. Importing from a file we also COMPILE INTO would duplicate content:
    //    the agent already reads that file, and compile would write it back.
    let targets: Vec<String> = c.cfg.targets().iter().map(|t| t.to_lowercase()).collect();
    let mut overlap: Vec<String> = mems
        .iter()
        .filter_map(|m| {
            m.provenance
                .get("evidence")
                .map(|e| e.split(':').next().unwrap_or("").to_lowercase())
        })
        .filter(|e| targets.contains(e))
        .collect();
    overlap.sort();
    overlap.dedup();
    if !overlap.is_empty() {
        let verb = if overlap.len() > 1 { "are" } else { "is" };
        println!(
            "\n{}Note:{} {} {} both an import source and a compile target.",
            s.yel,
            s.off,
            overlap.join(", "),
            verb
        );
        println!(
            "  Your agents already read it, so importing adds structure (scope, kind,\n  \
             provenance, review) but NOT reach. After approving, delete the original\n  \
             hand-written lines or you will carry the same rule twice."
        );
    }

    // 2. Volume. Unbounded memory measurably LOWERS agent accuracy, so a
    //    wholesale import is the junk-accumulation failure mode with extra steps.
    if mems.len() > 40 {
        println!(
            "\n{}That is a lot of memories.{} More memory is not better: in a controlled\n  \
             study, unbounded growth lowered accuracy for 4 of 4 agents (one 16.75% -> 13.05%).",
            s.yel, s.off
        );
        println!("  Import a slice you can actually review, e.g.:");
        println!(
            "    {}jedimem import --stage --from instructions --limit 25{}",
            s.bold, s.off
        );
        println!("  or start with the kinds that carry the most value per line:");
        println!(
            "    {}jedimem import --stage --from adr --from git{}   {}# decisions + reverts{}",
            s.bold, s.off, s.dim, s.off
        );
    }

    let mut by_kind: BTreeMap<&str, Vec<&Memory>> = BTreeMap::new();
    for m in &mems {
        by_kind.entry(m.kind.as_str()).or_default().push(m);
    }
    let show = a.num("show", 5);
    println!("\n{}{} candidate memories{}", s.bold, mems.len(), s.off);
    for (kind, group) in &by_kind {
        let flag = if kind_info(kind).map(|(_, h)| h).unwrap_or(false) {
            format!(" {}(needs human approval){}", s.yel, s.off)
        } else {
            String::new()
        };
        println!("\n{}{}{} × {}{}", s.bold, kind, s.off, group.len(), flag);
        for m in group.iter().take(show) {
            let head: String = m.headline().chars().take(110).collect();
            println!("  • {}", head);
            println!(
                "    {}{}  scope={}{}",
                s.dim,
                m.provenance.get("evidence").cloned().unwrap_or_default(),
                m.scope,
                s.off
            );
        }
        if group.len() > show {
            println!("  {}… {} more{}", s.dim, group.len() - show, s.off);
        }
    }

    if a.has("--commit") {
        for m in &mems {
            let mut m = m.clone();
            m.status = "active".into();
            c.store.write(&m).map_err(|e| e.to_string())?;
        }
        println!(
            "\n{}Wrote {} memories{} to {}",
            s.grn,
            mems.len(),
            s.off,
            c.store.mem_dir().display()
        );
        println!(
            "{}Note:{} --commit skips review. Check `git diff` before committing.",
            s.yel, s.off
        );
        let all = c.store.all(false).map_err(|e| e.to_string())?;
        for t in compiler::compile_repo(&c.root, &all, &c.cfg, false).map_err(|e| e.to_string())? {
            println!("  recompiled -> {}", t);
        }
        return Ok(0);
    }

    if !a.has("--stage") {
        println!("\n{}Dry run: nothing written.{}", s.dim, s.off);
        let from: Vec<String> = sources.iter().map(|x| format!("--from {}", x)).collect();
        println!(
            "Stage them for review with: {}jedimem import --stage {}{}",
            s.bold,
            from.join(" "),
            s.off
        );
        return Ok(0);
    }

    c.store
        .stage(&mems, &format!("jedimem: import {} candidates", mems.len()))
        .map_err(|e| e.to_string())?;
    println!(
        "\n{}Staged {} candidates{} on {}",
        s.grn,
        mems.len(),
        s.off,
        c.cfg.get("staging_ref")
    );
    println!("  Nothing was written to your working tree and your git status is unchanged.");
    println!("  Review with: {}jedimem review{}", s.bold, s.off);
    Ok(0)
}

fn cmd_compile(a: &Args, c: &Ctx) -> Result<i32, String> {
    let s = style();
    let mems = c.store.all(false).map_err(|e| e.to_string())?;
    let check = a.has("--check");
    let changed =
        compiler::compile_repo(&c.root, &mems, &c.cfg, check).map_err(|e| e.to_string())?;
    if check {
        if !changed.is_empty() {
            println!(
                "{}STALE{}: {}  (run `jedimem compile`)",
                s.red,
                s.off,
                changed.join(", ")
            );
            return Ok(1);
        }
        println!("up to date ({} active memories)", mems.len());
        return Ok(0);
    }
    for ch in &changed {
        println!("compiled -> {}", ch);
    }
    if changed.is_empty() {
        println!("up to date ({} active memories)", mems.len());
    }
    Ok(0)
}

fn cmd_status(_a: &Args, c: &Ctx) -> Result<i32, String> {
    let s = style();
    let active = c.store.all(false).map_err(|e| e.to_string())?;
    let all = c.store.all(true).map_err(|e| e.to_string())?;
    let pending = c.store.pending();
    println!(
        "{}jedimem{} {}   repo {}",
        s.bold,
        s.off,
        jedimem::VERSION,
        c.root.display()
    );
    let rid = c.cfg.get("repo_id");
    println!(
        "  repo_id     {}",
        if rid.is_empty() {
            format!("{}unset (run jedimem init){}", s.dim, s.off)
        } else {
            rid.to_string()
        }
    );
    println!("  memories    {} active, {} total", active.len(), all.len());
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for m in &all {
        *counts.entry(m.status.as_str()).or_default() += 1;
    }
    if !counts.is_empty() {
        let parts: Vec<String> = counts.iter().map(|(k, v)| format!("{} {}", v, k)).collect();
        println!("              {}{}{}", s.dim, parts.join(", "), s.off);
    }
    print!("  pending     {} awaiting review", pending.len());
    if !pending.is_empty() {
        print!("  {}-> jedimem review{}", s.yel, s.off);
    }
    println!();
    let used: usize = active
        .iter()
        .filter(|m| m.tier == "always")
        .map(|m| m.headline().len() + 20)
        .sum();
    let budget = c.cfg.num("always_chars", 6000);
    let bar = if used > budget {
        "over budget".to_string()
    } else {
        format!("{} chars headroom", budget - used)
    };
    println!("  always tier {}/{} chars ({})", used, budget, bar);
    let stale =
        compiler::compile_repo(&c.root, &active, &c.cfg, true).map_err(|e| e.to_string())?;
    println!(
        "  compiled    {}",
        if stale.is_empty() {
            format!("{}up to date{}", s.grn, s.off)
        } else {
            format!("{}stale: {}{}", s.red, stale.join(", "), s.off)
        }
    );
    if c.cfg.flag("paused") {
        println!("  capture     {}PAUSED{}", s.yel, s.off);
    }
    Ok(0)
}

fn cmd_review(a: &Args, c: &Ctx) -> Result<i32, String> {
    let s = style();
    let pending = c.store.pending();
    if pending.is_empty() {
        println!("nothing pending");
        return Ok(0);
    }
    let known: Vec<String> = pending.iter().map(|m| m.id.clone()).collect();
    let approve = a.many("approve");
    let reject = a.many("reject");

    if !approve.is_empty() || !reject.is_empty() {
        let mut ok_ids = Vec::new();
        let mut no_ids = Vec::new();
        for (wants, bucket) in [(&approve, &mut ok_ids), (&reject, &mut no_ids)] {
            for w in wants.iter() {
                let hits = match_ids(&known, w);
                if hits.len() != 1 {
                    let which = if hits.is_empty() { "no" } else { "ambiguous" };
                    return Err(format!("{} pending memory {:?}", which, w));
                }
                bucket.push(hits[0].clone());
            }
        }
        if !ok_ids.is_empty() {
            c.store
                .promote(&ok_ids, "active")
                .map_err(|e| e.to_string())?;
            println!(
                "{}approved {}{} -> {}",
                s.grn,
                ok_ids.len(),
                s.off,
                c.store.mem_dir().display()
            );
        }
        if !no_ids.is_empty() {
            c.store
                .clear_pending(&no_ids, "jedimem: reject")
                .map_err(|e| e.to_string())?;
            println!("rejected {}", no_ids.len());
        }
        let all = c.store.all(false).map_err(|e| e.to_string())?;
        for t in compiler::compile_repo(&c.root, &all, &c.cfg, false).map_err(|e| e.to_string())? {
            println!("  recompiled -> {}", t);
        }
        return Ok(0);
    }

    if a.has("--approve-all") {
        c.store
            .promote(&known, "active")
            .map_err(|e| e.to_string())?;
        println!("{}approved all {}{}", s.grn, known.len(), s.off);
        let all = c.store.all(false).map_err(|e| e.to_string())?;
        for t in compiler::compile_repo(&c.root, &all, &c.cfg, false).map_err(|e| e.to_string())? {
            println!("  recompiled -> {}", t);
        }
        return Ok(0);
    }

    println!(
        "{}{} pending{}  {}(approve with `jedimem review --approve <handle>`){}\n",
        s.bold,
        pending.len(),
        s.off,
        s.dim,
        s.off
    );
    for m in &pending {
        let human = if m.needs_human() {
            format!(" {}[needs human]{}", s.yel, s.off)
        } else {
            String::new()
        };
        println!(
            "{}{}{}  {}/{}  scope={}{}",
            s.bold,
            m.short(),
            s.off,
            m.kind,
            m.tier,
            m.scope,
            human
        );
        let head: String = m.headline().chars().take(140).collect();
        println!("  {}", head);
        println!(
            "  {}from {} ({}){}\n",
            s.dim,
            m.provenance
                .get("evidence")
                .cloned()
                .unwrap_or_else(|| "?".into()),
            m.provenance
                .get("source")
                .cloned()
                .unwrap_or_else(|| "?".into()),
            s.off
        );
    }
    Ok(0)
}

fn cmd_list(a: &Args, c: &Ctx) -> Result<i32, String> {
    let s = style();
    let mut mems = c.store.all(a.has("--all")).map_err(|e| e.to_string())?;
    if let Some(k) = a.one("kind") {
        mems.retain(|m| m.kind == k);
    }
    for m in &mems {
        let head: String = m.headline().chars().take(80).collect();
        println!(
            "{}  {:10} {:11} {:10} {}",
            m.short(),
            m.status,
            m.kind,
            m.tier,
            head
        );
    }
    println!("{}{} memories{}", s.dim, mems.len(), s.off);
    Ok(0)
}

fn cmd_why(a: &Args, c: &Ctx) -> Result<i32, String> {
    let s = style();
    let query = a.positional.first().cloned().unwrap_or_default();
    if query.is_empty() {
        return Err("usage: jedimem why <text-or-handle>".into());
    }
    let q = query.to_lowercase();
    let all = c.store.all(true).map_err(|e| e.to_string())?;
    let hits: Vec<&Memory> = all
        .iter()
        .filter(|m| m.id.to_lowercase().contains(&q) || m.body.to_lowercase().contains(&q))
        .collect();
    if hits.is_empty() {
        println!("no memory matches {:?}", query);
        return Ok(1);
    }
    for m in hits.iter().take(5) {
        println!(
            "{}{}{}  {}/{}  status={}",
            s.bold, m.id, s.off, m.kind, m.tier, m.status
        );
        println!("  {}", m.headline());
        println!("  {}provenance{}", s.bold, s.off);
        for (k, v) in &m.provenance {
            println!("    {:12} {}", k, v);
        }
        if !m.supersedes.is_empty() {
            println!("    {:12} {}", "supersedes", m.supersedes.join(", "));
        }
        println!();
    }
    Ok(0)
}

fn cmd_contest(a: &Args, c: &Ctx) -> Result<i32, String> {
    let s = style();
    let id = a.positional.first().cloned().unwrap_or_default();
    let reason = a.positional.get(1).cloned().unwrap_or_default();
    if id.is_empty() || reason.is_empty() {
        return Err("usage: jedimem contest <handle> \"<reason>\"".into());
    }
    let all = c.store.all(true).map_err(|e| e.to_string())?;
    let ids: Vec<String> = all.iter().map(|m| m.id.clone()).collect();
    let hits = match_ids(&ids, &id);
    if hits.len() != 1 {
        let which = if hits.is_empty() { "no" } else { "ambiguous" };
        return Err(format!("{} memory {:?}", which, id));
    }
    let m = c
        .store
        .set_status(&hits[0], "contested", &reason)
        .map_err(|e| e.to_string())?;
    println!("{}contested{} {}: {}", s.yel, s.off, m.id, reason);
    println!("  It stops being delivered but is NOT deleted -- history is kept.");
    let active = c.store.all(false).map_err(|e| e.to_string())?;
    for t in compiler::compile_repo(&c.root, &active, &c.cfg, false).map_err(|e| e.to_string())? {
        println!("  recompiled -> {}", t);
    }
    Ok(0)
}

fn cmd_lint(_a: &Args, c: &Ctx) -> Result<i32, String> {
    let s = style();
    let dir = c.store.mem_dir();
    let mut bad = 0usize;
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    paths.sort();
    for p in paths {
        if p.extension().map(|x| x != "md").unwrap_or(true) {
            continue;
        }
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        match Memory::load(&p) {
            Err(e) => {
                println!("{}FAIL{} {}: {}", s.red, s.off, name, e);
                bad += 1;
            }
            Ok(m) => {
                if let Some(prev) = seen.get(&m.content_hash()) {
                    println!(
                        "{}WARN{} {}: duplicate content of {}",
                        s.yel, s.off, name, prev
                    );
                }
                seen.insert(m.content_hash(), name.clone());
                if LOCAL_ONLY_KINDS.contains(&m.kind.as_str()) {
                    println!(
                        "{}FAIL{} {}: kind {:?} must stay in .jedimem/local/, never committed",
                        s.red, s.off, name, m.kind
                    );
                    bad += 1;
                }
            }
        }
    }
    println!("{} memories checked, {} invalid", seen.len(), bad);
    Ok(if bad > 0 { 1 } else { 0 })
}

fn cmd_pause(state: &str, c: &Ctx) -> Result<i32, String> {
    let local = c.root.join(".jedimem").join("local");
    std::fs::create_dir_all(&local).map_err(|e| e.to_string())?;
    let p = local.join("config.yml");
    let cur = std::fs::read_to_string(&p).unwrap_or_default();
    let val = if state == "pause" { "true" } else { "false" };
    let mut lines: Vec<String> = cur
        .lines()
        .filter(|l| !l.starts_with("paused:"))
        .map(String::from)
        .collect();
    lines.push(format!("paused: {}", val));
    std::fs::write(&p, format!("{}\n", lines.join("\n"))).map_err(|e| e.to_string())?;
    println!(
        "capture {} (local only, not committed)",
        if val == "true" { "PAUSED" } else { "resumed" }
    );
    Ok(0)
}

fn usage() -> String {
    let kinds: Vec<&str> = KINDS.iter().map(|(k, _, _)| *k).collect();
    format!(
        "jedimem {} -- team memory for coding agents, stored as files in your repo.

USAGE
  jedimem <command> [options]

COMMANDS
  init                     create .jedimem/ in this repo
  install [--agent X]      wire jedimem into the agents you have installed
  uninstall                remove those hooks (keeps your memories)
  import                   bootstrap memory from what this repo already knows
  review                   approve or reject pending candidates
  compile [--check]        regenerate the AGENTS.md / CLAUDE.md sections
  status                   what is captured, pending, and compiled
  list [--all] [--kind K]  list memories
  why <text|handle>        where did this memory come from?
  contest <handle> <why>   mark a memory disputed (never deletes)
  lint                     validate memory files (for CI)
  migrate [--check]        bring this repo's files up to the current format
  update [--force]         check whether a newer jedimem exists (never installs)
  doctor                   diagnose this installation
  session-start [--json]   hook entry point: always exits 0, never blocks
  pause | resume           stop or restart capture in this repo

IMPORT OPTIONS
  --from <source>          repeatable; default: all. --list-sources to see them
  --stage                  stage candidates for review (default is a dry run)
  --commit                 write straight to .jedimem/memories, skipping review
  --limit N, --show N

GLOBAL
  --root <path>            repo root (default: discover from cwd)
  --version, --help

MEMORY KINDS
  {}
",
        jedimem::VERSION,
        kinds.join(", ")
    )
}

/// Restore the default SIGPIPE disposition.
///
/// Rust sets SIGPIPE to SIG_IGN at startup, so a write to a closed pipe returns
/// EPIPE and the standard print macros *panic*. That turns the ordinary
/// `jedimem list | head` into a crash with a backtrace. Every well-behaved CLI
/// resets this.
#[cfg(unix)]
fn reset_sigpipe() {
    // SAFETY: setting a signal disposition to the default is sound, and this
    // runs before any other thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}
#[cfg(not(unix))]
fn reset_sigpipe() {}

fn main() -> ExitCode {
    reset_sigpipe();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let s = style();

    if argv.is_empty() || argv.iter().any(|a| a == "--help" || a == "-h") {
        print!("{}", usage());
        return ExitCode::from(0);
    }
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        println!("jedimem {}", jedimem::VERSION);
        return ExitCode::from(0);
    }

    let args = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}{}{}", s.red, e, s.off);
            return ExitCode::from(2);
        }
    };

    // These must work outside a repo: session-start is a hook and must never
    // fail, and update is about the binary, not the checkout.
    if args.cmd == "session-start" {
        return ExitCode::from(cmd_session_start(&args) as u8);
    }
    if args.cmd == "update" && ctx(&args).is_err() {
        return match cmd_update(&args, None) {
            Ok(code) => ExitCode::from(code as u8),
            Err(e) => {
                eprintln!("{}{}{}", s.red, e, s.off);
                ExitCode::from(1)
            }
        };
    }

    // `import --list-sources` must work without a repo.
    if args.cmd == "import" && args.has("--list-sources") {
        println!("import sources:");
        for (name, doc) in SOURCES {
            println!("  {:14} {}", name, doc);
        }
        return ExitCode::from(0);
    }

    let c = match ctx(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}{}{}", s.red, e, s.off);
            return ExitCode::from(2);
        }
    };

    let result = match args.cmd.as_str() {
        "init" => cmd_init(&args, &c),
        "import" => cmd_import(&args, &c),
        "compile" => cmd_compile(&args, &c),
        "status" => cmd_status(&args, &c),
        "review" => cmd_review(&args, &c),
        "list" => cmd_list(&args, &c),
        "why" => cmd_why(&args, &c),
        "contest" => cmd_contest(&args, &c),
        "lint" => cmd_lint(&args, &c),
        "migrate" => cmd_migrate(&args, &c),
        "update" => cmd_update(&args, Some(&c)),
        "doctor" => cmd_doctor(&args, &c),
        "install" => cmd_install(&args, &c),
        "merge-driver" => cmd_merge_driver(&args, &c),
        "uninstall" => {
            let mut a2 = args;
            a2.flags.push("--uninstall".into());
            cmd_install(&a2, &c)
        }
        "pause" => cmd_pause("pause", &c),
        "resume" => cmd_pause("resume", &c),
        other => Err(format!("unknown command {:?}\n\n{}", other, usage())),
    };

    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("{}{}{}", s.red, e, s.off);
            ExitCode::from(1)
        }
    }
}

// ------------------------------------------------------- update & migration

fn cmd_migrate(a: &Args, c: &Ctx) -> Result<i32, String> {
    let s = style();
    let dry = a.has("--dry-run");
    if a.has("--check") {
        return match migrate::status(&c.root) {
            migrate::Status::UpToDate { format } => {
                println!("up to date (format {})", format);
                Ok(0)
            }
            migrate::Status::NotInitialised => {
                println!("not initialised here");
                Ok(0)
            }
            migrate::Status::Pending { from, ids } => {
                println!(
                    "{}PENDING{}: format {} -> {} ({})",
                    s.yel,
                    s.off,
                    from,
                    migrate::SUPPORTED_FORMAT,
                    ids.join(", ")
                );
                Ok(1)
            }
            migrate::Status::BinaryTooOld {
                repo_format,
                supported,
            } => {
                println!(
                    "{}jedimem is out of date{}: repo is format {}, this binary \
                          understands {}",
                    s.red, s.off, repo_format, supported
                );
                Ok(1)
            }
        };
    }
    let applied = migrate::run(&c.root, &c.cfg, dry)?;
    if applied.ran.is_empty() && applied.from == applied.to {
        println!("up to date (format {})", applied.to);
        return Ok(0);
    }
    println!(
        "{}format {} -> {}{}",
        s.bold, applied.from, applied.to, s.off
    );
    for r in &applied.ran {
        println!("  {} {}", if dry { "would run" } else { "ran" }, r);
    }
    if dry {
        println!("\n{}Dry run: nothing written.{}", s.dim, s.off);
        return Ok(0);
    }
    if applied.changed.is_empty() {
        println!("  no files needed changing");
    } else {
        for ch in &applied.changed {
            println!("  changed {}", ch);
        }
        // Migrations rewrite tracked files. Committing is the human's act --
        // the same rule that stops the capture path touching your branch.
        println!(
            "\n{}Review and commit these changes.{} jedimem does not commit for you.",
            s.yel, s.off
        );
    }
    Ok(0)
}

fn cmd_update(a: &Args, c: Option<&Ctx>) -> Result<i32, String> {
    let s = style();
    let quiet = a.has("--quiet");
    let remote = a
        .one("remote")
        .unwrap_or_else(|| update::UPSTREAM.to_string());

    if a.has("--check-only") {
        // The detached path: perform the network check, cache it, say nothing.
        let got = update::check_now(&remote);
        if !quiet {
            if got.error.is_empty() {
                println!("latest: {}", got.latest);
            } else {
                println!("check failed: {}", got.error);
            }
        }
        return Ok(0);
    }

    let mut cached = update::read_cache();
    if update::is_stale(&cached) || a.has("--force") {
        if !quiet {
            println!("checking {} …", remote);
        }
        cached = update::check_now(&remote);
    }
    if !cached.error.is_empty() {
        println!(
            "{}could not check for updates:{} {}",
            s.dim, s.off, cached.error
        );
        println!("  (offline is fine -- jedimem never requires the network)");
        return Ok(0);
    }
    if cached.latest.is_empty() {
        println!("no release tags found upstream");
        return Ok(0);
    }
    if !update::is_newer(&cached.latest, jedimem::VERSION) {
        println!(
            "jedimem {} is current (latest {})",
            jedimem::VERSION,
            cached.latest
        );
    } else {
        println!(
            "{}jedimem {} is available{} (you have {})",
            s.grn,
            cached.latest,
            s.off,
            jedimem::VERSION
        );
        println!(
            "\n  cargo install --git {} --tag {} --force",
            remote, cached.latest
        );
        println!(
            "\n{}jedimem never installs itself.{} New code should run on your \
                  machine when you\n  choose the moment, not when a background \
                  process does.",
            s.bold, s.off
        );
    }
    // Whether or not the binary is current, the repo may need migrating.
    if let Some(c) = c {
        match migrate::status(&c.root) {
            migrate::Status::Pending { from, .. } => println!(
                "\n{}This repo is at format {} and needs migrating{} -> run `jedimem migrate`",
                s.yel, from, s.off
            ),
            migrate::Status::BinaryTooOld {
                repo_format,
                supported,
            } => println!(
                "\n{}This repo is at format {} but this binary understands {}{} -- upgrade first.",
                s.red, repo_format, supported, s.off
            ),
            _ => {}
        }
    }
    Ok(0)
}

/// Everything a SessionStart hook does.
///
/// Contract: **always exit 0, never touch the network, never block.** A memory
/// tool that occasionally injects nothing is a minor disappointment; one that
/// occasionally breaks the agent is uninstalled the same day.
fn cmd_session_start(a: &Args) -> i32 {
    let json = a.has("--json");
    let mut notes: Vec<String> = Vec::new();

    // Everything below is best-effort. Any failure degrades to "no notes".
    if let Ok(c) = ctx(a) {
        if !c.cfg.flag("paused") {
            match migrate::status(&c.root) {
                migrate::Status::BinaryTooOld {
                    repo_format,
                    supported,
                } => notes.push(format!(
                    "jedimem is OUT OF DATE: this repo's memories are format {} but the \
                     installed jedimem understands {}. Memories may be skipped. \
                     Run `jedimem update`, then `jedimem migrate`.",
                    repo_format, supported
                )),
                migrate::Status::Pending { from, .. } => notes.push(format!(
                    "jedimem: this repo is at format {} and needs migrating to {}. \
                     Run `jedimem migrate` (it writes files for you to review; it does \
                     not commit).",
                    from,
                    migrate::SUPPORTED_FORMAT
                )),
                _ => {}
            }

            let unreadable = migrate::unreadable_memories(&c.root);
            if !unreadable.is_empty() {
                notes.push(format!(
                    "jedimem: {} memory file(s) use a newer format than this binary and \
                     are being ignored. Run `jedimem update`.",
                    unreadable.len()
                ));
            }

            let pending = c.store.pending().len();
            if pending > 0 {
                notes.push(format!(
                    "jedimem: {} captured memory candidate(s) awaiting review \
                     (`jedimem review`).",
                    pending
                ));
            }

            if let Ok(stale) = compiler::compile_repo(
                &c.root,
                &c.store.all(false).unwrap_or_default(),
                &c.cfg,
                true,
            ) {
                if !stale.is_empty() {
                    notes.push(format!(
                        "jedimem: {} is stale -- run `jedimem compile`.",
                        stale.join(", ")
                    ));
                }
            }
        }
    }

    // The update check is the ONLY part that would touch the network, so it is
    // detached: this process never waits on it, and the result is read next
    // session from cache.
    let cached = update::read_cache();
    if update::is_stale(&cached) {
        update::spawn_background_check();
    } else if !cached.latest.is_empty()
        && cached.error.is_empty()
        && update::is_newer(&cached.latest, jedimem::VERSION)
    {
        notes.push(format!(
            "jedimem {} is available (you have {}). Run `jedimem update` for the command.",
            cached.latest,
            jedimem::VERSION
        ));
    }

    if notes.is_empty() {
        return 0;
    }
    if json {
        // The hook protocol shared by Claude Code and Codex. Keep it small:
        // injected context competes with the user's actual task.
        let body = notes.join(" ");
        let escaped = body
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', " ");
        println!(
            "{{\"hookSpecificOutput\":{{\"hookEventName\":\"SessionStart\",\
             \"additionalContext\":\"{}\"}}}}",
            escaped
        );
    } else {
        for n in &notes {
            println!("{}", n);
        }
    }
    0
}

fn cmd_doctor(_a: &Args, c: &Ctx) -> Result<i32, String> {
    let s = style();
    println!("{}jedimem {}{}", s.bold, jedimem::VERSION, s.off);
    println!(
        "  binary        {}",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "?".into())
    );
    println!("  repo          {}", c.root.display());
    println!(
        "  repo_id       {}",
        if c.cfg.get("repo_id").is_empty() {
            "unset".into()
        } else {
            c.cfg.get("repo_id").to_string()
        }
    );

    let git_ok = repo::run(&["--version"], None)
        .map(|(v, ok)| if ok { v } else { String::new() })
        .unwrap_or_default();
    println!(
        "  git           {}",
        if git_ok.is_empty() {
            format!(
                "{}NOT FOUND -- jedimem cannot work without git{}",
                s.red, s.off
            )
        } else {
            git_ok
        }
    );

    print!("  format        ");
    match migrate::status(&c.root) {
        migrate::Status::UpToDate { format } => {
            println!("{}{} (up to date){}", s.grn, format, s.off)
        }
        migrate::Status::NotInitialised => {
            println!("{}not initialised -- run `jedimem init`{}", s.yel, s.off)
        }
        migrate::Status::Pending { from, ids } => println!(
            "{}{} -> {} pending: {}{}",
            s.yel,
            from,
            migrate::SUPPORTED_FORMAT,
            ids.join(", "),
            s.off
        ),
        migrate::Status::BinaryTooOld {
            repo_format,
            supported,
        } => println!(
            "{}repo is {} but this binary understands {} -- UPGRADE{}",
            s.red, repo_format, supported, s.off
        ),
    }

    let unreadable = migrate::unreadable_memories(&c.root);
    if !unreadable.is_empty() {
        println!(
            "  {}{} memory file(s) too new to read{}",
            s.red,
            unreadable.len(),
            s.off
        );
    }

    let cached = update::read_cache();
    print!("  updates       ");
    if cached.checked_at == 0 {
        println!("never checked ({}) ", update::cache_path().display());
    } else if !cached.error.is_empty() {
        println!("{}last check failed: {}{}", s.dim, cached.error, s.off);
    } else if update::is_newer(&cached.latest, jedimem::VERSION) {
        println!("{}{} available{}", s.grn, cached.latest, s.off);
    } else {
        println!("current (latest seen: {})", cached.latest);
    }

    println!("  staging ref   {}", c.cfg.get("staging_ref"));
    println!(
        "  capture       {}",
        if c.cfg.flag("paused") {
            format!("{}PAUSED{}", s.yel, s.off)
        } else {
            "active".into()
        }
    );

    // Worktrees share memory but keep separate local state; say which we are in.
    if let Ok(wts) = repo::worktrees(Some(&c.root)) {
        if wts.len() > 1 {
            println!(
                "  worktrees     {} (memory is shared across all of them)",
                wts.len()
            );
        }
    }
    Ok(0)
}

// ------------------------------------------------------------------ install

fn cmd_install(a: &Args, c: &Ctx) -> Result<i32, String> {
    let s = style();
    let uninstall = a.has("--uninstall");
    let pin = a
        .one("pin")
        .unwrap_or_else(|| format!("v{}", jedimem::VERSION));

    let wanted: Vec<install::AgentKind> = match a.one("agent") {
        Some(name) => install::ALL_AGENTS
            .iter()
            .filter(|x| x.name() == name)
            .cloned()
            .collect(),
        None => install::ALL_AGENTS.to_vec(),
    };
    if wanted.is_empty() {
        return Err(format!(
            "unknown agent; choose from {}",
            install::ALL_AGENTS
                .iter()
                .map(|a| a.name())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let mut acted = 0;
    for agent in &wanted {
        let present = install::is_installed(agent);
        // Detect, never assume: don't wire an agent this machine doesn't have,
        // unless the user is explicitly preparing the repo for teammates.
        if !present && !a.has("--all") && a.one("agent").is_none() {
            println!(
                "  {:12} {}not installed on this machine -- skipped{}",
                agent.name(),
                s.dim,
                s.off
            );
            continue;
        }
        let r = if uninstall {
            install::uninstall_agent(agent, &c.root)?
        } else {
            install::install_agent(agent, &c.root, &pin)?
        };
        let colour = match r.action {
            "installed" | "updated" | "removed" => s.grn,
            _ => s.dim,
        };
        println!(
            "  {:12} {}{}{}  {}{}",
            r.agent,
            colour,
            r.action,
            s.off,
            r.path,
            if r.note.is_empty() {
                String::new()
            } else {
                format!("  ({})", r.note)
            }
        );
        acted += 1;
    }

    if !uninstall {
        match install::register_merge_driver(&c.root, &c.cfg.targets()) {
            Ok(added) => println!(
                "  {:12} {}{}{}  merge driver for {}",
                "merge",
                s.grn,
                if added { "registered" } else { "already set" },
                s.off,
                c.cfg.targets().join(", ")
            ),
            Err(e) => println!(
                "  {:12} {}could not register: {}{}",
                "merge", s.yel, e, s.off
            ),
        }
        match install::install_git_hooks(&c.root) {
            Ok(hooks) if !hooks.is_empty() => println!(
                "  {:12} {}installed{}  {}",
                "git hooks",
                s.grn,
                s.off,
                hooks.join(", ")
            ),
            Ok(_) => println!("  {:12} {}already present{}", "git hooks", s.dim, s.off),
            Err(e) => println!("  {:12} {}skipped: {}{}", "git hooks", s.yel, e, s.off),
        }
    }
    if uninstall {
        println!(
            "\n{}Uninstalled.{} Your memories in {} were left alone --",
            s.grn,
            s.off,
            c.store.mem_dir().display()
        );
        println!("  they are your team's files, not ours.");
        return Ok(0);
    }

    if acted == 0 {
        println!(
            "\n{}No supported agent found on this machine.{}",
            s.yel, s.off
        );
        println!("  Use --all to write the config anyway (useful when preparing a repo");
        println!("  for teammates who do have them).");
        return Ok(0);
    }

    // Initialise if needed, so `install` is the only command a newcomer needs.
    if migrate::repo_format(&c.root).is_none() {
        println!(
            "\n{}Not initialised yet.{} Run: {}jedimem init{}",
            s.yel, s.off, s.bold, s.off
        );
    }
    println!(
        "\n{}Installed.{} Commit these files so your teammates get them:",
        s.grn, s.off
    );
    println!("  git add .claude .codex .pi .jedimem 2>/dev/null; git commit -m \"add jedimem\"");
    println!(
        "\nNext: {}jedimem import{}  (dry run -- shows what this repo already knows)",
        s.bold, s.off
    );
    Ok(0)
}

/// `git merge-file`-compatible driver: `jedimem merge-driver %A %O %B`.
///
/// Registered by `jedimem install`. Merge drivers live in per-clone git config
/// and are deliberately NOT cloned, which is why install (which runs per clone)
/// is the right place to set it up.
fn cmd_merge_driver(a: &Args, c: &Ctx) -> Result<i32, String> {
    if a.positional.len() < 3 {
        return Err("usage: jedimem merge-driver <ours> <base> <theirs>".into());
    }
    let mems = c.store.all(false).map_err(|e| e.to_string())?;
    let clean = compiler::merge_driver(
        Path::new(&a.positional[0]),
        Path::new(&a.positional[1]),
        Path::new(&a.positional[2]),
        &c.root,
        &mems,
        &c.cfg,
    )
    .map_err(|e| e.to_string())?;
    // Non-zero tells git a conflict remains -- only ever for the human-written
    // part, since the generated section is always regenerated cleanly.
    Ok(if clean { 0 } else { 1 })
}
