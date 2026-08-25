//! jedimem test suite, ported from the Python implementation it replaces.
//!
//! These tests ARE the acceptance spec for the port: each one pins a behaviour
//! that was measured, or a bug that was found the hard way. No network, no LLM.

use jedimem::compiler::{self, RenderOpts};
use jedimem::config;
use jedimem::importers;
use jedimem::memory::{content_hash, ulid, Memory, NewMemory};
use jedimem::redact::redact;
use jedimem::repo;
use jedimem::store::Store;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    // target/<profile>/deps/<test> -> target/<profile>/jedimem
    let mut p = std::env::current_exe().expect("test exe");
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("jedimem")
}

fn git(args: &[&str], cwd: &Path) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn new_repo() -> PathBuf {
    let d = std::env::temp_dir().join(format!("jedimem-t-{}", ulid()));
    std::fs::create_dir_all(&d).unwrap();
    Command::new("git")
        .args(["init", "-q", "."])
        .current_dir(&d)
        .output()
        .unwrap();
    git(&["config", "user.email", "t@example.com"], &d);
    git(&["config", "user.name", "Test"], &d);
    std::fs::write(d.join("f.txt"), "base\n").unwrap();
    git(&["add", "-A"], &d);
    git(&["commit", "-qm", "base"], &d);
    d
}

fn mem(kind: &str, body: &str) -> Memory {
    Memory::create(NewMemory {
        kind,
        body,
        ..Default::default()
    })
    .unwrap()
}

fn active(kind: &str, body: &str, scope: &str) -> Memory {
    Memory::create(NewMemory {
        kind,
        body,
        scope,
        status: "active",
        ..Default::default()
    })
    .unwrap()
}

fn opts<'a>() -> RenderOpts<'a> {
    RenderOpts {
        always_chars: 6000,
        scoped_chars: 4000,
        marker_begin: "<!--B-->",
        marker_end: "<!--E-->",
    }
}

// ------------------------------------------------------------------ memory

#[test]
fn roundtrip_preserves_every_field() {
    let m = Memory::create(NewMemory {
        kind: "gotcha",
        body: "Point DATABASE_URL at the docker pg.\n\n**Why:** stale schema.",
        scope: "tests/**",
        ..Default::default()
    })
    .unwrap();
    let r = Memory::from_text(&m.to_text()).unwrap();
    assert_eq!(r.id, m.id);
    assert_eq!(r.kind, "gotcha");
    assert_eq!(r.scope, "tests/**");
    assert_eq!(r.body, m.body);
    r.validate(Some(&r.id)).unwrap();
}

#[test]
fn headline_is_first_paragraph_not_first_line() {
    assert_eq!(mem("convention", "one\ntwo\n\nthree").headline(), "one two");
}

#[test]
fn ulids_are_unique_within_a_millisecond() {
    let ids: BTreeSet<String> = (0..500).map(|_| ulid()).collect();
    assert_eq!(ids.len(), 500);
}

#[test]
fn short_handle_uses_the_random_half() {
    // The first 10 chars are the timestamp; same-batch ids collide there, which
    // made `review --approve <prefix>` ambiguous in the Python version.
    let a = mem("topic", "aaaa bbbb cccc");
    let b = mem("topic", "cccc dddd eeee");
    assert_eq!(&a.id[..10], &b.id[..10], "documents the hazard");
    assert_ne!(a.short(), b.short(), "...and that we avoid it");
}

#[test]
fn identical_content_hashes_the_same_across_machines() {
    assert_eq!(
        content_hash("Use httpClient, not axios."),
        content_hash("use   HTTPCLIENT, not axios.  ")
    );
}

#[test]
fn invalid_records_are_rejected() {
    type Mutate = Box<dyn Fn(&mut Memory)>;
    let cases: Vec<Mutate> = vec![
        Box::new(|m: &mut Memory| m.kind = "nonsense".into()),
        Box::new(|m: &mut Memory| m.status = "nope".into()),
        Box::new(|m: &mut Memory| m.body = "  ".into()),
        Box::new(|m: &mut Memory| m.format = 99),
        Box::new(|m: &mut Memory| m.tier = "elsewhere".into()),
    ];
    for mutate in cases {
        let mut m = mem("convention", "something durable and long enough");
        mutate(&mut m);
        assert!(m.validate(None).is_err());
    }
}

#[test]
fn a_memory_can_never_grant_capability() {
    for banned in ["allowed_tools", "permissions", "hooks", "exec", "command"] {
        let mut m = mem("convention", "something durable and long enough");
        m.extra.insert(banned.into(), "Bash".into());
        assert!(m.validate(None).is_err(), "{} must be refused", banned);
    }
}

#[test]
fn a_memory_can_never_have_a_person_as_subject() {
    let mut m = mem("convention", "something durable and long enough");
    m.extra.insert("subject_person".into(), "alice".into());
    assert!(m.validate(None).is_err());
}

#[test]
fn weaker_provenance_cannot_supersede_stronger() {
    let human = Memory::create(NewMemory {
        kind: "convention",
        body: "a rule a human confirmed",
        confirmed_by: "human",
        ..Default::default()
    })
    .unwrap();
    let agent = Memory::create(NewMemory {
        kind: "convention",
        body: "a rule an agent guessed",
        confirmed_by: "agent",
        ..Default::default()
    })
    .unwrap();
    assert!(!human.may_be_superseded_by(&agent));
    assert!(agent.may_be_superseded_by(&human));
}

// ----------------------------------------------------------------- redact

#[test]
fn redaction_catches_common_secret_shapes() {
    for t in [
        "key sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAA",
        "token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345",
        "export API_KEY=\"hunter2hunter2\"",
        "https://user:pa55word@example.com/x",
        "aws AKIAIOSFODNN7EXAMPLE",
    ] {
        assert!(!redact(t).1.is_empty(), "missed: {}", t);
    }
}

#[test]
fn redaction_leaves_ordinary_prose_alone() {
    let t = "Use the internal httpClient wrapper, not axios, because of tracing.";
    let (out, found) = redact(t);
    assert_eq!(out, t);
    assert!(found.is_empty());
}

#[test]
fn anthropic_keys_are_labelled_specifically() {
    let (_, found) = redact("sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAA");
    assert_eq!(found, vec!["anthropic-key".to_string()]);
}

// --------------------------------------------------------------- compiler

fn sample() -> Vec<Memory> {
    vec![
        active("requirement", "Migrations reviewed by platform.", "**"),
        active("convention", "Use httpClient not axios.", "src/**"),
        active("runbook", "Deploy with make ship.", "**"),
    ]
}

#[test]
fn tiers_are_separated() {
    let out = compiler::render(&sample(), &opts());
    assert!(out.contains("## Always"));
    assert!(out.contains("## When touching matching files"));
    assert!(out.contains("## Available on request"));
}

#[test]
fn inactive_memories_are_not_delivered() {
    let m = mem("requirement", "A superseded rule that must not appear.");
    let out = compiler::render(&[m], &opts());
    assert!(!out.contains("must not appear"));
}

#[test]
fn budget_demotes_and_says_so() {
    let mut o = opts();
    o.always_chars = 10;
    let out = compiler::render(&sample(), &o);
    assert!(out.contains("demoted"), "must never truncate silently");
}

#[test]
fn splice_is_idempotent() {
    let sec = compiler::render(&sample(), &opts());
    let once = compiler::splice("# Mine\n\nhand written\n", &sec, "<!--B-->", "<!--E-->");
    let twice = compiler::splice(&once, &sec, "<!--B-->", "<!--E-->");
    assert_eq!(once, twice);
}

#[test]
fn splice_is_idempotent_when_file_is_only_our_section() {
    // The bug that made CI report AGENTS.md perpetually stale.
    let sec = compiler::render(&sample(), &opts());
    let once = compiler::splice("", &sec, "<!--B-->", "<!--E-->");
    let twice = compiler::splice(&once, &sec, "<!--B-->", "<!--E-->");
    assert_eq!(once, twice);
}

#[test]
fn handwritten_content_survives_compilation() {
    let sec = compiler::render(&sample(), &opts());
    let out = compiler::splice("# Mine\n\nkeep me\n", &sec, "<!--B-->", "<!--E-->");
    assert!(out.contains("keep me"));
    assert!(out.contains("# Mine"));
}

// ------------------------------------------------------------ staging ref

#[test]
fn staging_never_dirties_the_worktree() {
    let d = new_repo();
    let store = Store::new(&d, repo::STAGING_REF);
    store
        .stage(&[mem("gotcha", "Something worth remembering here.")], "t")
        .unwrap();
    assert_eq!(
        git(&["status", "--porcelain"], &d),
        "",
        "staging must not touch the working tree"
    );
    assert_eq!(
        git(&["rev-list", "--count", "HEAD"], &d),
        "1",
        "must not advance the branch"
    );
}

#[test]
fn pending_roundtrips_and_promotes() {
    let d = new_repo();
    let store = Store::new(&d, repo::STAGING_REF);
    let m = mem("gotcha", "Something worth remembering here.");
    store.stage(std::slice::from_ref(&m), "t").unwrap();
    assert_eq!(
        store
            .pending()
            .iter()
            .map(|p| p.id.clone())
            .collect::<Vec<_>>(),
        vec![m.id.clone()]
    );
    store
        .promote(std::slice::from_ref(&m.id), "active")
        .unwrap();
    assert!(store.pending().is_empty());
    assert_eq!(
        store
            .all(false)
            .unwrap()
            .iter()
            .map(|x| x.id.clone())
            .collect::<Vec<_>>(),
        vec![m.id]
    );
}

#[test]
fn concurrent_writers_lose_nothing() {
    // The measured failure mode: porcelain commits dropped 13 of 20 memories
    // to index.lock, silently. The CAS-on-a-side-ref path must not.
    let d = new_repo();
    let handles: Vec<_> = (0..12)
        .map(|i| {
            let d = d.clone();
            std::thread::spawn(move || {
                let store = Store::new(&d, repo::STAGING_REF);
                store
                    .stage(
                        &[mem(
                            "topic",
                            &format!("Concurrent memory number {} body.", i),
                        )],
                        "t",
                    )
                    .map(|_| ())
            })
        })
        .collect();
    for h in handles {
        h.join().expect("thread").expect("stage must succeed");
    }
    let store = Store::new(&d, repo::STAGING_REF);
    assert_eq!(store.pending().len(), 12);
}

// --------------------------------------------------------------- importers

fn brownfield() -> PathBuf {
    let d = new_repo();
    std::fs::write(
        d.join("CLAUDE.md"),
        "# Rules\n- Use the internal httpClient wrapper, never axios directly.\n\
         - All migrations must be reviewed by the platform team first.\n",
    )
    .unwrap();
    std::fs::create_dir_all(d.join(".claude/rules")).unwrap();
    std::fs::write(
        d.join(".claude/rules/api.md"),
        "# API\n- Handlers must validate input with the shared zod schemas.\n",
    )
    .unwrap();
    std::fs::create_dir_all(d.join("docs/adr")).unwrap();
    std::fs::write(
        d.join("docs/adr/0002-gql.md"),
        "# Adopt GraphQL\n## Status\nSuperseded\n## Decision\n\
         We added a GraphQL gateway to unify client access; it regressed latency badly.\n",
    )
    .unwrap();
    std::fs::create_dir_all(d.join(".github")).unwrap();
    std::fs::write(d.join(".github/CODEOWNERS"), "/billing/  @acme/payments\n").unwrap();
    d
}

#[test]
fn instructions_include_rule_directories() {
    let d = brownfield();
    let got = importers::from_instructions(&d);
    let joined: String = got
        .iter()
        .map(|m| m.body.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(joined.contains("httpClient"));
    assert!(joined.contains("zod"), "must scan .claude/rules/ too");
}

#[test]
fn kind_inference_recognises_requirements() {
    let d = brownfield();
    let got = importers::from_instructions(&d);
    let m = got
        .iter()
        .find(|m| m.headline().contains("migrations"))
        .expect("found");
    assert_eq!(m.kind, "requirement");
}

#[test]
fn repo_map_entries_are_topics_not_workflows() {
    // `\brun` used to match "cross-runtime" and misfile these as workflow.
    let d = new_repo();
    std::fs::write(
        d.join("AGENTS.md"),
        "# Layout\n- `packages/common/` — cross-runtime utilities shared by services.\n",
    )
    .unwrap();
    let got = importers::from_instructions(&d);
    assert_eq!(got[0].kind, "topic");
}

#[test]
fn superseded_adrs_become_negative_knowledge() {
    let d = brownfield();
    let got = importers::from_adrs(&d);
    assert!(!got.is_empty());
    assert_eq!(got[0].kind, "negative");
}

#[test]
fn codeowners_carries_scope() {
    let d = brownfield();
    let got = importers::from_codeowners(&d);
    assert_eq!(got[0].kind, "ownership");
    assert_eq!(got[0].scope, "billing/");
}

#[test]
fn reverts_become_negative_memories() {
    let d = brownfield();
    std::fs::write(d.join("f.txt"), "x\n").unwrap();
    git(
        &[
            "commit",
            "-qam",
            "Revert \"Move auth into the gateway\"\n\nIt broke SSO refresh.",
        ],
        &d,
    );
    let got = importers::from_git_history(&d);
    assert!(!got.is_empty());
    assert_eq!(got[0].kind, "negative");
    assert!(got[0].body.contains("SSO"));
}

#[test]
fn import_is_idempotent() {
    let d = brownfield();
    let sources = vec!["instructions".to_string()];
    let (first, _) = importers::run_import(&d, &sources, &Default::default(), 0).unwrap();
    let existing = first
        .iter()
        .map(|m| (m.content_hash(), m.clone()))
        .collect();
    let (second, stats) = importers::run_import(&d, &sources, &existing, 0).unwrap();
    assert!(second.is_empty(), "re-import must add nothing");
    assert!(stats["instructions"].duplicate > 0);
}

#[test]
fn never_reimports_our_own_generated_section() {
    let d = new_repo();
    std::fs::write(
        d.join("AGENTS.md"),
        "# A\n<!-- BEGIN jedimem -->\n\
         - **[convention]** Generated rule that must not return ever again.\n\
         <!-- END jedimem -->\n",
    )
    .unwrap();
    let got = importers::from_instructions(&d);
    assert!(!got.iter().any(|m| m.body.contains("must not return")));
}

#[test]
fn secrets_are_redacted_before_staging() {
    let d = new_repo();
    std::fs::write(
        d.join("CONVENTIONS.md"),
        "# C\n- Deploy with the token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345 in CI.\n",
    )
    .unwrap();
    let (mems, stats) =
        importers::run_import(&d, &["instructions".to_string()], &Default::default(), 0).unwrap();
    let joined: String = mems
        .iter()
        .map(|m| m.body.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(!joined.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345"));
    assert!(stats["instructions"].redacted > 0);
}

#[test]
fn unknown_import_source_is_an_error_not_a_silent_skip() {
    let d = new_repo();
    assert!(importers::run_import(&d, &["nope".to_string()], &Default::default(), 0).is_err());
}

// ------------------------------------------------------------------ config

#[test]
fn repo_config_cannot_change_privacy_settings() {
    let d = new_repo();
    std::fs::create_dir_all(d.join(".jedimem")).unwrap();
    std::fs::write(
        d.join(".jedimem/config.yml"),
        "repo_id: ABC\npaused: true\nruntime: api\nbudgets:\n  always_chars: 1234\n",
    )
    .unwrap();
    let cfg = config::load(&d);
    assert_eq!(cfg.get("repo_id"), "ABC");
    assert_eq!(cfg.num("always_chars", 0), 1234);
    // A cloned repo must not be able to set these on your machine.
    assert_eq!(cfg.get("runtime"), "auto");
    assert!(!cfg.flag("paused"));
}

#[test]
fn compile_targets_parse_as_a_list() {
    let d = new_repo();
    std::fs::create_dir_all(d.join(".jedimem")).unwrap();
    std::fs::write(
        d.join(".jedimem/config.yml"),
        "compile:\n  targets: [AGENTS.md, CLAUDE.md, GEMINI.md]\n",
    )
    .unwrap();
    assert_eq!(
        config::load(&d).targets(),
        vec!["AGENTS.md", "CLAUDE.md", "GEMINI.md"]
    );
}

// --------------------------------------------------------------------- cli

fn cli(args: &[&str], cwd: &Path) -> (String, i32) {
    // Hermetic: never read the developer's real update cache, or a pending
    // "new version available" notice leaks into unrelated assertions.
    let cache = std::env::temp_dir().join("jedimem-test-cache");
    let out = Command::new(bin())
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("XDG_CACHE_HOME", &cache)
        .output()
        .expect("run jedimem");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn full_workflow() {
    let d = new_repo();
    std::fs::write(
        d.join("CLAUDE.md"),
        "# Rules\n- Use the internal httpClient wrapper, never axios directly.\n",
    )
    .unwrap();

    assert_eq!(cli(&["init"], &d).1, 0);
    let (out, code) = cli(&["import"], &d);
    assert_eq!(code, 0);
    assert!(out.contains("candidate"), "{}", out);

    assert_eq!(cli(&["import", "--stage"], &d).1, 0);
    let (review, _) = cli(&["review"], &d);
    let handle = review
        .lines()
        .find(|l| {
            l.len() > 8
                && l.chars()
                    .take(8)
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        })
        .and_then(|l| l.split_whitespace().next())
        .expect("a pending handle");
    assert_eq!(cli(&["review", "--approve", handle], &d).1, 0);

    assert_eq!(
        cli(&["compile", "--check"], &d).1,
        0,
        "must be fresh after approve"
    );
    assert!(cli(&["status"], &d).0.contains("active"));
    assert!(cli(&["why", "httpClient"], &d).0.contains("provenance"));
    assert_eq!(cli(&["lint"], &d).1, 0);
}

#[test]
fn compile_check_fails_when_stale() {
    let d = new_repo();
    cli(&["init"], &d);
    std::fs::write(
        d.join(".jedimem/memories/01TESTTESTTESTTESTTESTTEST.md"),
        Memory {
            id: "01TESTTESTTESTTESTTESTTEST".into(),
            status: "active".into(),
            ..active("convention", "A rule that has not been compiled yet.", "**")
        }
        .to_text(),
    )
    .unwrap();
    assert_eq!(cli(&["compile", "--check"], &d).1, 1, "CI must catch drift");
    assert_eq!(cli(&["compile"], &d).1, 0);
    assert_eq!(cli(&["compile", "--check"], &d).1, 0);
}

#[test]
fn outside_a_git_repo_fails_clearly() {
    let d = std::env::temp_dir().join(format!("jedimem-nogit-{}", ulid()));
    std::fs::create_dir_all(&d).unwrap();
    let (out, code) = cli(&["status"], &d);
    assert_ne!(code, 0);
    assert!(out.to_lowercase().contains("git"), "{}", out);
}

#[test]
fn unknown_command_is_an_error_with_usage() {
    let d = new_repo();
    let (out, code) = cli(&["frobnicate"], &d);
    assert_ne!(code, 0);
    assert!(out.contains("COMMANDS"), "{}", out);
}

// ------------------------------------------------------- update & migration

use jedimem::migrate;
use jedimem::update;

#[test]
fn version_comparison_orders_releases() {
    assert!(update::is_newer("v0.2.0", "0.1.0"));
    assert!(update::is_newer("0.1.1", "0.1.0"));
    assert!(update::is_newer("v1.0.0", "0.9.9"));
    assert!(!update::is_newer("v0.1.0", "0.1.0"));
    assert!(!update::is_newer("v0.0.9", "0.1.0"));
    // Garbage must never be read as "newer", or a bad tag nags every session.
    assert!(!update::is_newer("latest", "0.1.0"));
    assert!(!update::is_newer("", "0.1.0"));
}

#[test]
fn migrate_reports_uninitialised_repos() {
    let d = new_repo();
    assert!(matches!(
        migrate::status(&d),
        migrate::Status::NotInitialised
    ));
}

#[test]
fn migrate_reports_up_to_date() {
    let d = new_repo();
    cli(&["init"], &d);
    match migrate::status(&d) {
        migrate::Status::UpToDate { format } => assert_eq!(format, migrate::SUPPORTED_FORMAT),
        other => panic!("expected up to date, got {:?}", other),
    }
}

#[test]
fn a_repo_from_the_future_is_refused_not_downgraded() {
    // The team scenario: someone upgrades first and commits format N+1. An
    // older binary must say so loudly rather than silently skipping memories.
    let d = new_repo();
    cli(&["init"], &d);
    let cfg = d.join(".jedimem/config.yml");
    let text = std::fs::read_to_string(&cfg)
        .unwrap()
        .replace("format: 1", "format: 99");
    std::fs::write(&cfg, text).unwrap();

    match migrate::status(&d) {
        migrate::Status::BinaryTooOld {
            repo_format,
            supported,
        } => {
            assert_eq!(repo_format, 99);
            assert_eq!(supported, migrate::SUPPORTED_FORMAT);
        }
        other => panic!("expected BinaryTooOld, got {:?}", other),
    }
    let cfg_loaded = config::load(&d);
    let err = migrate::run(&d, &cfg_loaded, false).unwrap_err();
    assert!(err.contains("do NOT downgrade"), "{}", err);
    // and the stamp must be untouched
    assert!(std::fs::read_to_string(&cfg)
        .unwrap()
        .contains("format: 99"));
}

#[test]
fn memories_too_new_to_parse_are_detected() {
    let d = new_repo();
    cli(&["init"], &d);
    let mut m = active("convention", "A rule written by a newer jedimem.", "**");
    m.format = 7;
    std::fs::write(
        d.join(".jedimem/memories").join(format!("{}.md", m.id)),
        m.to_text(),
    )
    .unwrap();
    let found = migrate::unreadable_memories(&d);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, 7);
}

#[test]
fn session_start_always_exits_zero() {
    // The fail-open contract. A memory tool that occasionally injects nothing
    // is a minor disappointment; one that breaks the agent is uninstalled.
    let d = new_repo();
    assert_eq!(cli(&["session-start"], &d).1, 0, "uninitialised repo");

    cli(&["init"], &d);
    assert_eq!(cli(&["session-start"], &d).1, 0, "initialised repo");

    // corrupt config
    std::fs::write(d.join(".jedimem/config.yml"), "\u{0}not: [valid\n").unwrap();
    assert_eq!(cli(&["session-start"], &d).1, 0, "corrupt config");

    // repo from the future
    std::fs::write(d.join(".jedimem/config.yml"), "format: 99\n").unwrap();
    assert_eq!(cli(&["session-start"], &d).1, 0, "future format");

    // outside a git repo entirely
    let plain = std::env::temp_dir().join(format!("jedimem-nogit-{}", ulid()));
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(cli(&["session-start"], &plain).1, 0, "not a git repo");
}

#[test]
fn session_start_is_silent_when_there_is_nothing_to_say() {
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("jedimem-test-cache"));
    let d = new_repo();
    cli(&["init"], &d);
    cli(&["compile"], &d);
    let (out, code) = cli(&["session-start"], &d);
    assert_eq!(code, 0);
    assert!(
        out.trim().is_empty(),
        "must not add noise every session: {:?}",
        out
    );
}

#[test]
fn session_start_json_is_well_formed_and_escaped() {
    let d = new_repo();
    std::fs::write(
        d.join("CLAUDE.md"),
        "# R\n- Use httpClient, never axios \"directly\".\n",
    )
    .unwrap();
    cli(&["init"], &d);
    cli(&["import", "--stage"], &d);
    let (out, code) = cli(&["session-start", "--json"], &d);
    assert_eq!(code, 0);
    let line = out.lines().find(|l| l.starts_with('{')).expect("json line");
    assert!(line.contains("\"hookEventName\":\"SessionStart\""));
    assert!(line.contains("additionalContext"));
    assert!(line.ends_with("}}"), "{}", line);
    // Naive quoting would break the payload; check quotes are escaped.
    let body = line.split("\"additionalContext\":\"").nth(1).unwrap();
    assert!(!body.trim_end_matches("\"}}").contains("\"") || body.contains("\\\""));
    // Must be one line: the hook protocol reads a single JSON object.
    assert_eq!(out.lines().filter(|l| l.starts_with('{')).count(), 1);
}

#[test]
fn session_start_reports_pending_and_staleness() {
    let d = new_repo();
    std::fs::write(
        d.join("CLAUDE.md"),
        "# R\n- Use httpClient, never axios directly.\n",
    )
    .unwrap();
    cli(&["init"], &d);
    cli(&["import", "--stage"], &d);
    let (out, _) = cli(&["session-start"], &d);
    assert!(out.contains("awaiting review"), "{}", out);
}

#[test]
fn migrate_check_is_ci_friendly() {
    let d = new_repo();
    cli(&["init"], &d);
    assert_eq!(cli(&["migrate", "--check"], &d).1, 0);
    let cfg = d.join(".jedimem/config.yml");
    let text = std::fs::read_to_string(&cfg)
        .unwrap()
        .replace("format: 1", "format: 99");
    std::fs::write(&cfg, text).unwrap();
    assert_eq!(
        cli(&["migrate", "--check"], &d).1,
        1,
        "must fail CI when out of date"
    );
}

#[test]
fn install_wires_agents_without_clobbering_existing_config() {
    let d = new_repo();
    std::fs::create_dir_all(d.join(".claude")).unwrap();
    std::fs::write(
        d.join(".claude/settings.json"),
        r#"{"permissions":{"allow":["Bash(make verify)"]},
            "hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"./warm.sh"}]}],
                     "PostToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"make fmt"}]}]}}"#,
    ).unwrap();
    cli(&["init"], &d);
    assert_eq!(cli(&["install", "--all"], &d).1, 0);

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join(".claude/settings.json")).unwrap())
            .expect("still valid JSON");
    assert!(
        v["permissions"]["allow"].is_array(),
        "their settings survived"
    );
    let ss = v["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(ss.len(), 2, "ours appended, theirs kept");
    assert!(
        v["hooks"]["PostToolUse"].is_array(),
        "unrelated hooks untouched"
    );

    // Idempotent: running twice must not duplicate our entry.
    assert_eq!(cli(&["install", "--all"], &d).1, 0);
    let v2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(v2["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
}

#[test]
fn install_registers_merge_driver_and_git_hooks() {
    // Both are per-clone (git never clones hooks or merge drivers), so install
    // is the only place they can be set up -- and a silent omission here means
    // every teammate hits merge conflicts on generated files.
    let d = new_repo();
    cli(&["init"], &d);
    cli(&["install", "--all"], &d);

    let driver = String::from_utf8_lossy(
        &Command::new("git")
            .args(["config", "--local", "--get", "merge.jedimem.driver"])
            .current_dir(&d)
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();
    assert!(
        driver.contains("jedimem merge-driver"),
        "driver not registered: {:?}",
        driver
    );

    let ga = std::fs::read_to_string(d.join(".gitattributes")).unwrap_or_default();
    assert!(
        ga.contains("AGENTS.md merge=jedimem"),
        "gitattributes: {:?}",
        ga
    );

    for hook in ["post-merge", "post-checkout"] {
        let p = d.join(".git/hooks").join(hook);
        assert!(p.exists(), "{} hook missing", hook);
        assert!(std::fs::read_to_string(&p)
            .unwrap()
            .contains("jedimem compile"));
    }
}

#[test]
fn uninstall_leaves_memories_and_foreign_config_alone() {
    let d = new_repo();
    std::fs::write(
        d.join("CLAUDE.md"),
        "# R\n- Use httpClient, never axios directly.\n",
    )
    .unwrap();
    cli(&["init"], &d);
    cli(&["install", "--all"], &d);
    cli(&["import", "--stage"], &d);
    let review = cli(&["review"], &d).0;
    let handle = review
        .lines()
        .find(|l| {
            l.len() > 8
                && l.chars()
                    .take(8)
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        })
        .and_then(|l| l.split_whitespace().next())
        .unwrap()
        .to_string();
    cli(&["review", "--approve", &handle], &d);
    let before = std::fs::read_dir(d.join(".jedimem/memories"))
        .unwrap()
        .count();
    assert!(before > 0);

    assert_eq!(cli(&["uninstall"], &d).1, 0);
    let after = std::fs::read_dir(d.join(".jedimem/memories"))
        .unwrap()
        .count();
    assert_eq!(before, after, "uninstall must not delete memories");
    let claude = d.join(".claude/settings.json");
    if claude.exists() {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&claude).unwrap()).unwrap();
        let has_ours = v["hooks"]["SessionStart"]
            .as_array()
            .map(|a| a.iter().any(|e| e.to_string().contains("jedimem")))
            .unwrap_or(false);
        assert!(!has_ours, "our hook should be gone");
    }
}

/// A PATH containing git but none of the agent CLIs, so `is_installed` is false
/// for all of them while jedimem still works.
fn git_only_path() -> std::ffi::OsString {
    let dir = std::env::temp_dir().join(format!("jedimem-gitonly-{}", ulid()));
    std::fs::create_dir_all(&dir).unwrap();
    let git = String::from_utf8_lossy(
        &Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();
    #[cfg(unix)]
    {
        let link = dir.join("git");
        if !link.exists() {
            let _ = std::os::unix::fs::symlink(&git, &link);
        }
    }
    dir.into_os_string()
}

#[test]
fn uninstall_cleans_up_agents_this_machine_never_had() {
    // CI has no agent binaries installed. Detection must gate install only:
    // skipping uninstall for an absent agent leaves our hooks in the repo for
    // every teammate. Found by CI, not by a developer machine that had all three.
    let d = new_repo();
    cli(&["init"], &d);
    cli(&["install", "--all"], &d); // write config for agents regardless of presence
    assert!(d.join(".codex/hooks.json").exists());

    let out = Command::new(bin())
        .args(["uninstall"])
        .current_dir(&d)
        .env("NO_COLOR", "1")
        // A PATH with git but no agent binaries -- jedimem itself needs git.
        .env("PATH", git_only_path())
        .env(
            "XDG_CACHE_HOME",
            std::env::temp_dir().join("jedimem-test-cache"),
        )
        .output()
        .expect("run");
    assert!(out.status.success());
    let left = std::fs::read_to_string(d.join(".codex/hooks.json")).unwrap_or_default();
    assert!(!left.contains("jedimem"), "hooks left behind: {}", left);
}
