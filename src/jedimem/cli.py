"""The jedimem command line."""
from __future__ import annotations

import argparse
import os
import pathlib
import sys

from . import __version__, compiler, config, importers, repo
from .memory import KINDS, LOCAL_ONLY_KINDS, Memory, MemoryError
from .store import Store

BOLD, DIM, RED, GRN, YEL, OFF = "\033[1m", "\033[2m", "\033[31m", "\033[32m", "\033[33m", "\033[0m"
if not sys.stdout.isatty() or os.environ.get("NO_COLOR"):
    BOLD = DIM = RED = GRN = YEL = OFF = ""


def _root(explicit=None) -> pathlib.Path:
    if explicit:
        return pathlib.Path(explicit).resolve()
    try:
        return repo.toplevel()
    except repo.GitError:
        sys.exit(f"{RED}not inside a git repository{OFF}\n"
                 f"jedimem stores memory in git. Run `git init` first, or pass --root.")


def _match_ids(ids, want: str) -> list:
    """Resolve a user-typed handle. Exact id, then suffix, then prefix."""
    want = want.strip().upper()
    if want in ids:
        return [want]
    hits = [i for i in ids if i.endswith(want)]
    return hits or [i for i in ids if i.startswith(want)]


def _ctx(args):
    root = _root(getattr(args, "root", None))
    cfg = config.load(root)
    return root, cfg, Store(root, staging_ref=cfg.get("staging_ref", repo.STAGING_REF))


# ------------------------------------------------------------------ commands

def cmd_init(args):
    root, cfg, store = _ctx(args)
    d = root / ".jedimem"
    if (d / "config.yml").exists() and not args.force:
        print(f"already initialised at {d}  (use --force to overwrite config)")
        return 0
    (d / "memories").mkdir(parents=True, exist_ok=True)
    (d / "local").mkdir(parents=True, exist_ok=True)
    from .memory import ulid
    rid = ulid()
    (d / "config.yml").write_text(f"""# jedimem repo configuration -- committed and shared.
format: 1

# Authoritative repo identity. Written once, never derived: a root-commit hash
# changes under a shallow clone and a remote URL has several spellings.
repo_id: {rid}

budgets:
  always_chars: 6000
  scoped_chars_per_glob: 4000

compile:
  targets: [AGENTS.md, CLAUDE.md]
  marker_begin: "<!-- BEGIN jedimem -->"
  marker_end: "<!-- END jedimem -->"

capture:
  batch_window_minutes: 30
  staging_ref: refs/jedimem/log
""", encoding="utf-8")
    gi = root / ".gitignore"
    cur = gi.read_text(encoding="utf-8") if gi.exists() else ""
    if ".jedimem/local/" not in cur:
        gi.write_text(cur.rstrip("\n") + "\n\n# jedimem per-user, per-machine state\n.jedimem/local/\n",
                      encoding="utf-8")
    print(f"{GRN}initialised{OFF} {d}")
    print(f"  repo_id: {rid}")
    print(f"\nNext: {BOLD}jedimem import --dry-run{OFF} to see what this repo already knows.")
    return 0


def cmd_import(args):
    root, cfg, store = _ctx(args)
    if args.list_sources:
        print("import sources:")
        for name, fn in sorted(importers.SOURCES.items()):
            doc = ((fn.__doc__ or "").strip().splitlines() or [""])[0]
            print(f"  {name:14} {doc}")
        return 0

    sources = args.source or list(importers.SOURCES)
    try:
        mems, stats = importers.run(root, sources=sources,
                                    existing_hashes=store.hashes(), limit=args.limit)
    except ValueError as e:
        sys.exit(f"{RED}{e}{OFF}")

    print(f"{BOLD}Scanned{OFF} {root}")
    for name in sources:
        s = stats.get(name, {})
        if not s:
            continue
        extra = f", {s['redacted']} redacted" if s.get("redacted") else ""
        print(f"  {name:14} {s['found']:4} found  "
              f"{GRN}{s['new']:4} new{OFF}  {DIM}{s['duplicate']} dup{extra}{OFF}")
    if not mems:
        print(f"\n{YEL}Nothing new to import.{OFF} "
              f"Either this repo has no instruction files/ADRs/reverts, "
              f"or they are already imported.")
        return 0

    by_kind = {}
    for m in mems:
        by_kind.setdefault(m.kind, []).append(m)

    # --- two warnings that matter more than the listing itself --------------

    # 1. Importing from a file we also COMPILE INTO would duplicate content:
    #    the agent already reads that file, and compile would write it back.
    targets = {t.lower() for t in cfg.targets}
    overlap = sorted({m.provenance.get("evidence", "").split(":")[0].lower()
                      for m in mems} & targets)
    if overlap:
        verb = "are" if len(overlap) > 1 else "is"
        print(f"\n{YEL}Note:{OFF} {', '.join(overlap)} {verb} both an import source "
              f"and a compile target.")
        print(f"  Your agents already read it, so importing it adds structure "
              f"(scope, kind,\n  provenance, review) but NOT reach. After approving, "
              f"delete the original\n  hand-written lines or you will carry the same "
              f"rule twice.")

    # 2. Volume. Unbounded memory measurably LOWERS agent accuracy, so a
    #    wholesale import is the junk-accumulation failure mode with extra steps.
    if len(mems) > 40:
        print(f"\n{YEL}That is a lot of memories.{OFF} More memory is not better: "
              f"in a controlled\n  study, unbounded growth lowered accuracy for 4 of 4 "
              f"agents (one 16.75% -> 13.05%).")
        print(f"  Import a slice you can actually review, e.g.:")
        print(f"    {BOLD}jedimem import --stage --from instructions --limit 25{OFF}")
        print(f"  or start with the kinds that carry the most value per line:")
        print(f"    {BOLD}jedimem import --stage --from adr --from git{OFF}   "
              f"{DIM}# decisions + reverts{OFF}")

    print(f"\n{BOLD}{len(mems)} candidate memories{OFF}")
    for kind in sorted(by_kind):
        needs_human = KINDS[kind][1]
        flag = f" {YEL}(needs human approval){OFF}" if needs_human else ""
        print(f"\n{BOLD}{kind}{OFF} × {len(by_kind[kind])}{flag}")
        for m in by_kind[kind][: args.show]:
            src = m.provenance.get("evidence", "")
            print(f"  • {m.headline[:110]}")
            print(f"    {DIM}{src}  scope={m.scope}{OFF}")
        if len(by_kind[kind]) > args.show:
            print(f"  {DIM}… {len(by_kind[kind]) - args.show} more{OFF}")

    if args.dry_run:
        print(f"\n{DIM}Dry run: nothing written.{OFF}")
        print(f"Stage them for review with: {BOLD}jedimem import --stage "
              f"{' '.join('--from ' + s for s in sources)}{OFF}")
        return 0

    if args.commit:
        for m in mems:
            m.status = "active"
            store.write(m)
        print(f"\n{GRN}Wrote {len(mems)} memories{OFF} to {store.mem_dir}")
        print(f"{YEL}Note:{OFF} --commit skips review. Check `git diff` before committing.")
        changed = compiler.compile_repo(root, store.all(), cfg)
        if changed:
            print(f"  recompiled: {', '.join(changed)}")
        return 0

    store.stage(mems, message=f"jedimem: import {len(mems)} candidates")
    print(f"\n{GRN}Staged {len(mems)} candidates{OFF} on {cfg.get('staging_ref')}")
    print(f"  Nothing was written to your working tree and your git status is unchanged.")
    print(f"  Review with: {BOLD}jedimem review{OFF}")
    return 0


def cmd_compile(args):
    root, cfg, store = _ctx(args)
    mems = store.all()
    changed = compiler.compile_repo(root, mems, cfg, check=args.check)
    if args.check:
        if changed:
            print(f"{RED}STALE{OFF}: {', '.join(changed)}  (run `jedimem compile`)")
            return 1
        print(f"up to date ({len(mems)} active memories)")
        return 0
    for c in changed:
        print(f"compiled -> {c}")
    if not changed:
        print(f"up to date ({len(mems)} active memories)")
    return 0


def cmd_status(args):
    root, cfg, store = _ctx(args)
    active = store.all()
    allm = store.all(include_inactive=True)
    pending = store.pending()
    print(f"{BOLD}jedimem{OFF} {__version__}   repo {root}")
    print(f"  repo_id     {cfg.get('repo_id') or DIM + 'unset (run jedimem init)' + OFF}")
    print(f"  memories    {len(active)} active, {len(allm)} total")
    counts = {}
    for m in allm:
        counts[m.status] = counts.get(m.status, 0) + 1
    if counts:
        print(f"              {DIM}" + ", ".join(f"{v} {k}" for k, v in sorted(counts.items())) + OFF)
    print(f"  pending     {len(pending)} awaiting review"
          + (f"  {YEL}-> jedimem review{OFF}" if pending else ""))
    used = sum(len(m.headline) + 20 for m in active if m.tier == "always")
    budget = int(cfg.get("always_chars", 6000))
    bar = "over budget" if used > budget else f"{budget - used} chars headroom"
    print(f"  always tier {used}/{budget} chars ({bar})")
    stale = compiler.compile_repo(root, active, cfg, check=True)
    print(f"  compiled    " + (f"{RED}stale: {', '.join(stale)}{OFF}" if stale
                               else f"{GRN}up to date{OFF}"))
    if cfg.get("paused"):
        print(f"  capture     {YEL}PAUSED{OFF}")
    return 0


def cmd_review(args):
    root, cfg, store = _ctx(args)
    pending = store.pending()
    if not pending:
        print("nothing pending")
        return 0
    if args.approve or args.reject:
        ids = set(args.approve or []) | set(args.reject or [])
        known = {m.id for m in pending}
        # allow unique prefixes
        resolved_ok, resolved_no = [], []
        for want, bucket in [(args.approve or [], resolved_ok), (args.reject or [], resolved_no)]:
            for w in want:
                hits = _match_ids(known, w)
                if len(hits) != 1:
                    sys.exit(f"{RED}{'no' if not hits else 'ambiguous'} pending memory "
                             f"{w!r}{OFF}" + (f" -> {', '.join(h[-8:] for h in hits)}"
                                              if hits else ""))
                bucket.append(hits[0])
        if resolved_ok:
            store.promote(resolved_ok)
            print(f"{GRN}approved {len(resolved_ok)}{OFF} -> {store.mem_dir}")
        if resolved_no:
            store.clear_pending(resolved_no, message="jedimem: reject")
            print(f"rejected {len(resolved_no)}")
        changed = compiler.compile_repo(root, store.all(), cfg)
        for c in changed:
            print(f"  recompiled -> {c}")
        return 0
    if args.approve_all:
        ids = [m.id for m in pending]
        store.promote(ids)
        print(f"{GRN}approved all {len(ids)}{OFF}")
        for c in compiler.compile_repo(root, store.all(), cfg):
            print(f"  recompiled -> {c}")
        return 0

    print(f"{BOLD}{len(pending)} pending{OFF}  "
          f"{DIM}(approve with `jedimem review --approve <id-prefix>`){OFF}\n")
    for m in pending:
        human = f" {YEL}[needs human]{OFF}" if KINDS.get(m.kind, ("", False))[1] else ""
        print(f"{BOLD}{m.short}{OFF}  {m.kind}/{m.tier}  scope={m.scope}{human}")
        print(f"  {m.headline[:140]}")
        print(f"  {DIM}from {m.provenance.get('evidence','?')} "
              f"({m.provenance.get('source','?')}){OFF}\n")
    return 0


def cmd_list(args):
    root, cfg, store = _ctx(args)
    mems = store.all(include_inactive=args.all)
    if args.kind:
        mems = [m for m in mems if m.kind == args.kind]
    for m in mems:
        print(f"{m.short}  {m.status:10} {m.kind:11} {m.tier:10} {m.headline[:80]}")
    print(f"{DIM}{len(mems)} memories{OFF}")
    return 0


def cmd_why(args):
    root, cfg, store = _ctx(args)
    q = args.query.lower()
    hits = [m for m in store.all(include_inactive=True)
            if q in m.id.lower() or q in m.body.lower()]
    if not hits:
        print(f"no memory matches {args.query!r}")
        return 1
    for m in hits[:5]:
        print(f"{BOLD}{m.id}{OFF}  {m.kind}/{m.tier}  status={m.status}")
        print(f"  {m.headline}")
        print(f"  {BOLD}provenance{OFF}")
        for k, v in m.provenance.items():
            print(f"    {k:12} {v}")
        if m.supersedes:
            print(f"    supersedes   {', '.join(m.supersedes)}")
        print()
    return 0


def cmd_contest(args):
    root, cfg, store = _ctx(args)
    ids = {m.id for m in store.all(include_inactive=True)}
    match = _match_ids(ids, args.id)
    hits = [m for m in store.all(include_inactive=True) if m.id in match]
    if len(hits) != 1:
        sys.exit(f"{RED}{'no' if not hits else 'ambiguous'} memory {args.id!r}{OFF}")
    m = store.set_status(hits[0].id, "contested", note=args.reason)
    print(f"{YEL}contested{OFF} {m.id}: {args.reason}")
    print("  It stops being delivered but is NOT deleted -- history is kept.")
    for c in compiler.compile_repo(root, store.all(), cfg):
        print(f"  recompiled -> {c}")
    return 0


def cmd_lint(args):
    root, cfg, store = _ctx(args)
    bad = 0
    seen = {}
    for p in sorted((root / ".jedimem" / "memories").glob("*.md")):
        try:
            m = Memory.load(p)
        except MemoryError as e:
            print(f"{RED}FAIL{OFF} {p.name}: {e}")
            bad += 1
            continue
        if m.content_hash in seen:
            print(f"{YEL}WARN{OFF} {p.name}: duplicate content of {seen[m.content_hash]}")
        seen[m.content_hash] = p.name
        if m.kind in LOCAL_ONLY_KINDS:
            print(f"{RED}FAIL{OFF} {p.name}: kind {m.kind!r} must stay in .jedimem/local/, "
                  f"never committed")
            bad += 1
    print(f"{len(seen)} memories checked, {bad} invalid")
    return 1 if bad else 0


def cmd_pause(args):
    root, cfg, store = _ctx(args)
    local = root / ".jedimem" / "local"
    local.mkdir(parents=True, exist_ok=True)
    p = local / "config.yml"
    cur = p.read_text(encoding="utf-8") if p.exists() else ""
    val = "true" if args.state == "pause" else "false"
    lines = [l for l in cur.splitlines() if not l.startswith("paused:")]
    lines.append(f"paused: {val}")
    p.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"capture {'PAUSED' if val == 'true' else 'resumed'} (local only, not committed)")
    return 0


def main(argv=None):
    ap = argparse.ArgumentParser(
        prog="jedimem",
        description="Team memory for coding agents, stored as files in your repo.")
    ap.add_argument("--version", action="version", version=f"jedimem {__version__}")
    ap.add_argument("--root", help="repo root (default: discover from cwd)")
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("init", help="create .jedimem/ in this repo")
    p.add_argument("--force", action="store_true")
    p.set_defaults(fn=cmd_init)

    p = sub.add_parser("import", help="bootstrap memory from what this repo already knows")
    p.add_argument("--from", dest="source", action="append",
                   choices=sorted(importers.SOURCES),
                   help="import source (repeatable; default: all)")
    p.add_argument("--dry-run", action="store_true", default=True,
                   help="show what would be imported, write nothing (default)")
    p.add_argument("--stage", dest="dry_run", action="store_false",
                   help="stage candidates for review instead of printing")
    p.add_argument("--commit", action="store_true",
                   help="write straight to .jedimem/memories, skipping review")
    p.add_argument("--limit", type=int, default=0)
    p.add_argument("--show", type=int, default=5, help="examples to print per kind")
    p.add_argument("--list-sources", action="store_true")
    p.set_defaults(fn=cmd_import)

    p = sub.add_parser("compile", help="regenerate AGENTS.md / CLAUDE.md sections")
    p.add_argument("--check", action="store_true", help="exit 1 if stale (for CI)")
    p.set_defaults(fn=cmd_compile)

    p = sub.add_parser("status", help="what is captured, pending, and compiled")
    p.set_defaults(fn=cmd_status)

    p = sub.add_parser("review", help="approve or reject pending candidates")
    p.add_argument("--approve", action="append")
    p.add_argument("--reject", action="append")
    p.add_argument("--approve-all", action="store_true")
    p.set_defaults(fn=cmd_review)

    p = sub.add_parser("list", help="list memories")
    p.add_argument("--all", action="store_true", help="include inactive")
    p.add_argument("--kind", choices=sorted(KINDS))
    p.set_defaults(fn=cmd_list)

    p = sub.add_parser("why", help="where did this memory come from?")
    p.add_argument("query")
    p.set_defaults(fn=cmd_why)

    p = sub.add_parser("contest", help="mark a memory disputed (never deletes)")
    p.add_argument("id")
    p.add_argument("reason")
    p.set_defaults(fn=cmd_contest)

    p = sub.add_parser("lint", help="validate memory files (for CI)")
    p.set_defaults(fn=cmd_lint)

    p = sub.add_parser("pause", help="stop capturing in this repo")
    p.set_defaults(fn=cmd_pause, state="pause")
    p = sub.add_parser("resume", help="resume capturing")
    p.set_defaults(fn=cmd_pause, state="resume")

    args = ap.parse_args(argv)
    return args.fn(args)
