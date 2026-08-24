"""Bootstrapping memory for an existing ("brownfield") repository.

A mature repo is not a blank slate. It already encodes years of convention in
places nobody thinks of as memory: a hand-written CLAUDE.md, Cursor rules, ADRs,
a CODEOWNERS file, and a git history full of reverts that each mark something the
team tried and abandoned.

Importing that is what makes jedimem useful on day one instead of day ninety.
Waiting to accumulate memories from live sessions means the tool is worthless for
exactly as long as it takes people to uninstall it.

Design rules, all load-bearing:

  * **Deterministic and offline.** No importer here calls an LLM. A team must be
    able to run `jedimem import` on a private repo, read every proposed memory,
    and diff it -- with no model in the loop and nothing leaving the machine.
  * **Everything lands as `proposed`.** Import is a suggestion engine, never an
    author. A hand-written rule was written by a human, but it was not reviewed
    *as a memory*, and the two are different acts.
  * **Idempotent.** Re-running imports nothing new; dedup is by content hash
    against both the committed store and the current batch.
  * **Traceable.** Every imported memory records the file and line it came from,
    so `jedimem why` answers honestly.
"""
from __future__ import annotations

import pathlib
import re
import subprocess

from .memory import Memory, content_hash
from .redact import redact

# Files that already hold agent instructions, in rough order of authority.
INSTRUCTION_FILES = [
    "CLAUDE.md", "AGENTS.md", "GEMINI.md", "CONVENTIONS.md",
    ".windsurfrules", ".clinerules", ".rules",
    ".github/copilot-instructions.md",
]
INSTRUCTION_GLOBS = [
    ".cursor/rules/*.mdc", ".cursor/rules/**/*.mdc",
    ".github/instructions/*.instructions.md",
    ".clinerules/*.md",
    # Rule directories. Teams outgrow a single CLAUDE.md and split it up; a
    # brownfield import that misses these misses most of the actual content.
    ".claude/rules/*.md", ".claude/rules/**/*.md",
    ".agents/rules/*.md", ".codex/rules/*.md",
    "docs/conventions/*.md",
]
ADR_GLOBS = ["docs/adr/*.md", "docs/adr/**/*.md", "docs/decisions/*.md",
             "doc/adr/*.md", "adr/*.md", "docs/architecture/decisions/*.md"]

# Conservative wording heuristics. Deliberately biased toward `convention`,
# the least dangerous kind to get wrong: a wrong convention costs one
# non-idiomatic patch, whereas a wrong `requirement` gets treated as law.
_KIND_RULES = [
    ("requirement", re.compile(r"\b(must|shall|is required|are required|mandatory)\b", re.I)),
    ("constraint",  re.compile(r"\b(never|forbidden|do not ever|under no circumstances|"
                               r"security|compliance|gdpr|pci|hipaa)\b", re.I)),
    ("workflow",    re.compile(r"\b(run|runs|command|commands|before committing|"
                               r"to deploy|to test|npm|yarn|pnpm|make|pytest|cargo)\b", re.I)),
    ("style",       re.compile(r"\b(format|indent|quote|naming|lint|prettier|eslint|"
                               r"line length|semicolon)\b", re.I)),
    ("gotcha",      re.compile(r"\b(gotcha|careful|beware|note that|watch out|"
                               r"common mistake|footgun)\b", re.I)),
    ("negative",    re.compile(r"\b(we tried|don't use|do not use|avoid|deprecated|"
                               r"instead of|rather than)\b", re.I)),
]

_MIN_LEN = 25          # shorter than this is a fragment, not a rule
_MAX_LEN = 600         # longer than this is a document, not a memory


_TOPIC_RE = re.compile(r"^`?[\w@/.-]+/`?\s*[-—:]")          # "`packages/common/` — ..."


def _infer_kind(text: str) -> str:
    # A repo-map entry describes where code lives; it is not an instruction.
    # Check first, because such lines often contain incidental verbs.
    if _TOPIC_RE.match(text):
        return "topic"
    for kind, pat in _KIND_RULES:
        if pat.search(text):
            return kind
    return "convention"


def _clean(text: str) -> str:
    t = re.sub(r"\s+", " ", text.strip())
    t = re.sub(r"^[-*+]\s+", "", t)
    t = re.sub(r"^\d+[.)]\s+", "", t)
    return t.strip()


def _usable(text: str) -> bool:
    if not (_MIN_LEN <= len(text) <= _MAX_LEN):
        return False
    # Skip headings-as-bullets, links-only lines, code, and TODO scaffolding.
    if text.startswith(("#", "```", "|", "<!--")):
        return False
    if re.fullmatch(r"[\[\]\(\)\w\s./:-]*https?://\S+[\s\S]*", text) and len(text) < 80:
        return False
    if re.match(r"(?i)^(todo|tbd|wip|see also|table of contents)\b", text):
        return False
    return True


def _frontmatter(text: str) -> tuple:
    """Return (dict, body) for files that use YAML-ish frontmatter."""
    if not text.startswith("---"):
        return {}, text
    parts = text.split("\n---", 2)
    if len(parts) < 2:
        return {}, text
    fm = {}
    for line in parts[0].lstrip("-\n").splitlines():
        k, sep, v = line.partition(":")
        if sep:
            fm[k.strip()] = v.strip().strip('"').strip("'")
    return fm, parts[1].lstrip("\n") if len(parts) > 1 else text


def _scope_from_frontmatter(fm: dict) -> str:
    """Cursor `.mdc` uses `globs:`; Copilot instructions use `applyTo:`."""
    for key in ("globs", "applyTo", "apply_to"):
        v = fm.get(key)
        if v:
            first = v.split(",")[0].strip().strip("[]'\"")
            if first and first != "**/*":
                return first
    return "**"


# ------------------------------------------------------------------ sources

def from_instructions(root: pathlib.Path) -> list:
    """Existing agent-instruction files -- the highest-value import by far.

    A team adopting jedimem usually has one of these already, hand-curated. It is
    the closest thing to a ground-truth memory set that exists anywhere.
    """
    root = pathlib.Path(root)
    found = []
    paths = [root / f for f in INSTRUCTION_FILES]
    for g in INSTRUCTION_GLOBS:
        paths.extend(sorted(root.glob(g)))

    for p in paths:
        if not p.is_file():
            continue
        rel = p.relative_to(root).as_posix()
        text = p.read_text(encoding="utf-8", errors="replace")

        # Never re-import our own generated output: that is the file-level
        # version of the self-amplification loop.
        text = re.sub(r"<!-- BEGIN jedimem -->[\s\S]*?<!-- END jedimem -->", "", text)

        fm, body = _frontmatter(text)
        default_scope = _scope_from_frontmatter(fm)

        section = ""
        for lineno, raw in enumerate(body.splitlines(), start=1):
            line = raw.rstrip()
            if line.startswith("#"):
                section = line.lstrip("#").strip()
                continue
            if not re.match(r"^\s*([-*+]|\d+[.)])\s+", line):
                continue
            text_ = _clean(line)
            if not _usable(text_):
                continue
            scope = default_scope
            kind = _infer_kind(f"{section} {text_}")
            body_md = text_
            if section and section.lower() not in text_.lower():
                body_md = f"{text_}\n\n**Context:** {section} (imported from `{rel}`)"
            found.append(Memory.create(
                kind=kind, body=body_md, scope=scope, source="import",
                agent="", evidence=f"{rel}:{lineno}", confirmed_by="human"))
    return found


def from_adrs(root: pathlib.Path) -> list:
    """Architecture Decision Records: the pre-AI form of exactly this idea."""
    root = pathlib.Path(root)
    out = []
    seen = set()
    for g in ADR_GLOBS:
        for p in sorted(root.glob(g)):
            if not p.is_file() or p in seen:
                continue
            seen.add(p)
            rel = p.relative_to(root).as_posix()
            text = p.read_text(encoding="utf-8", errors="replace")
            fm, body = _frontmatter(text)

            title = ""
            for line in body.splitlines():
                if line.startswith("#"):
                    title = line.lstrip("#").strip()
                    break
            status = ""
            ms = re.search(r"(?im)^\s*(?:##\s*)?status\s*:?\s*\n?\s*(\w+)", body)
            if ms:
                status = ms.group(1).lower()
            # Superseded/rejected ADRs are still knowledge -- as `negative`.
            kind = "negative" if status in ("superseded", "rejected", "deprecated") else "decision"

            decision = ""
            md = re.search(r"(?is)##\s*decision\s*\n(.+?)(?=\n##|\Z)", body)
            if md:
                decision = _clean(md.group(1))[:_MAX_LEN]
            if not decision:
                decision = _clean(re.sub(r"^#.*", "", body, count=1))[:_MAX_LEN]
            if len(decision) < _MIN_LEN:
                continue

            b = f"{title}: {decision}" if title else decision
            if status:
                b += f"\n\n**Status:** {status} (ADR `{rel}`)"
            out.append(Memory.create(kind=kind, body=b, source="import",
                                     evidence=rel, confirmed_by="human"))
    return out


def from_codeowners(root: pathlib.Path) -> list:
    """CODEOWNERS: who to ask about a path, already machine-readable."""
    root = pathlib.Path(root)
    for cand in ("CODEOWNERS", ".github/CODEOWNERS", "docs/CODEOWNERS"):
        p = root / cand
        if not p.is_file():
            continue
        out = []
        for lineno, raw in enumerate(p.read_text(encoding="utf-8", errors="replace")
                                     .splitlines(), start=1):
            line = raw.split("#")[0].strip()
            if not line:
                continue
            parts = line.split()
            if len(parts) < 2:
                continue
            pattern, owners = parts[0], parts[1:]
            out.append(Memory.create(
                kind="ownership",
                body=f"Changes to `{pattern}` are owned by {', '.join(owners)}. "
                     f"Ask them for review or context.",
                scope=pattern.lstrip("/") or "**", source="import",
                evidence=f"{cand}:{lineno}", confirmed_by="human"))
        return out
    return []


def from_git_history(root: pathlib.Path, limit=2000) -> list:
    """Reverts are the cheapest negative knowledge a repo contains.

    A revert is a durable, machine-readable record that the team tried something
    and backed it out. Nobody writes those down, and every team relitigates them.
    """
    root = pathlib.Path(root)
    try:
        out = subprocess.run(
            ["git", "log", f"-{limit}", "--no-merges", "--format=%H%x00%s%x00%b%x00%an%x1e"],
            cwd=root, capture_output=True, text=True, check=True).stdout
    except (subprocess.CalledProcessError, FileNotFoundError):
        return []

    mems = []
    for rec in out.split("\x1e"):
        rec = rec.strip("\n")
        if not rec:
            continue
        parts = rec.split("\x00")
        if len(parts) < 3:
            continue
        sha, subject, body = parts[0], parts[1], parts[2]
        if not re.match(r'(?i)^revert\b|^revert "', subject):
            continue
        reverted = re.sub(r'(?i)^revert\s*"?', "", subject).rstrip('"')
        reason = ""
        for line in body.splitlines():
            line = line.strip()
            if line and not line.lower().startswith(("this reverts", "co-authored", "signed-off")):
                reason = line
                break
        b = f"Reverted: {reverted}."
        if reason:
            b += f"\n\n**Reason given:** {reason}"
        b += f"\n\n**Why this is here:** a revert records something the team tried " \
             f"and backed out. Confirm it still applies before relying on it " \
             f"(commit `{sha[:8]}`)."
        if len(b) > _MAX_LEN * 2:
            continue
        mems.append(Memory.create(kind="negative", body=b, source="import",
                                  commit=sha[:12], evidence=f"commit {sha[:8]}",
                                  confirmed_by="agent"))
    return mems


SOURCES = {
    "instructions": from_instructions,
    "adr": from_adrs,
    "codeowners": from_codeowners,
    "git": from_git_history,
}


def run(root, sources=None, existing_hashes=None, limit=0):
    """Import from the named sources, deduped and redacted.

    Returns (memories, stats).
    """
    root = pathlib.Path(root)
    sources = sources or list(SOURCES)
    existing = dict(existing_hashes or {})
    seen = set(existing)
    out, stats = [], {}

    for name in sources:
        fn = SOURCES.get(name)
        if fn is None:
            raise ValueError(f"unknown import source {name!r}; "
                             f"choose from {', '.join(sorted(SOURCES))}")
        got = fn(root)
        kept = 0
        dupes = 0
        secrets = 0
        for m in got:
            clean, found = redact(m.body)
            if found:
                secrets += 1
                m.body = clean
                m.extra["redacted"] = ",".join(sorted(set(found)))
            h = content_hash(m.body)
            if h in seen:
                dupes += 1
                continue
            seen.add(h)
            out.append(m)
            kept += 1
            if limit and len(out) >= limit:
                break
        stats[name] = {"found": len(got), "new": kept, "duplicate": dupes,
                       "redacted": secrets}
        if limit and len(out) >= limit:
            break
    return out, stats
