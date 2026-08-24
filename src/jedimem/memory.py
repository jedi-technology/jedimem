"""The memory record: parse, validate, serialise.

The on-disk format is the product's contract. Memories are committed to a repo
and live in git history forever, so a format change is a migration, not a
refactor -- hence the explicit `format:` stamp on every record.
"""
from __future__ import annotations

import hashlib
import os
import re
import time
from dataclasses import dataclass, field

FORMAT_VERSION = 1

# Delivery tiers. See docs/MEMORY-KINDS.md.
TIERS = ("always", "scoped", "on_demand", "pre_action")

# Kinds, mapped to the tier they default to and whether a human must approve.
# Kinds that encode *intent or authority* need a human; a transcript cannot
# establish that a rule is a team rule rather than one reviewer's opinion.
KINDS = {
    "convention":  ("scoped",    False),
    "requirement": ("always",    True),
    "style":       ("scoped",    False),
    "workflow":    ("on_demand", False),
    "topic":       ("on_demand", False),
    "gotcha":      ("scoped",    False),
    "negative":    ("on_demand", True),
    "decision":    ("on_demand", True),
    "runbook":     ("on_demand", False),
    "constraint":  ("always",    True),
    "glossary":    ("on_demand", False),
    "ownership":   ("on_demand", False),
    "flaky":       ("pre_action", False),
    "external":    ("on_demand", False),
    "perf":        ("scoped",    True),
    "migration":   ("always",    True),
    "preference":  ("always",    False),   # local-only
    "environment": ("on_demand", False),   # local-only
}
LOCAL_ONLY_KINDS = ("preference", "environment")

STATUSES = ("proposed", "active", "contested", "superseded", "rejected", "expired")

# Provenance tiers, strongest first. A newer claim may not supersede an older
# one of stronger provenance: a newer statement is not automatically truer.
PROVENANCE_STRENGTH = {"verified": 3, "human": 2, "agent": 1, "imported": 1}

_ULID_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"   # Crockford base32


def ulid(when: float | None = None, entropy: bytes | None = None) -> str:
    """Time-ordered, collision-free without coordination.

    Filename collisions are add/add merge conflicts, so IDs must be safe to
    generate on many machines with no shared state.
    """
    ms = int((time.time() if when is None else when) * 1000)
    rand = os.urandom(10) if entropy is None else entropy
    n = (ms << 80) | int.from_bytes(rand, "big")
    out = []
    for _ in range(26):
        out.append(_ULID_ALPHABET[n & 31])
        n >>= 5
    return "".join(reversed(out))


def content_hash(text: str) -> str:
    """Stable identity for *what a memory says*.

    Two developers whose agents independently learn the same fact produce the
    same hash, which is how duplicate detection works with no coordination.
    """
    norm = re.sub(r"\s+", " ", text.strip().lower())
    return hashlib.sha256(norm.encode("utf-8")).hexdigest()[:12]


class MemoryError(ValueError):
    pass


@dataclass
class Memory:
    id: str = ""
    format: int = FORMAT_VERSION
    kind: str = "convention"
    scope: str = "**"
    status: str = "proposed"
    tier: str = ""
    body: str = ""
    supersedes: list = field(default_factory=list)
    provenance: dict = field(default_factory=dict)
    extra: dict = field(default_factory=dict)

    # -- construction -----------------------------------------------------
    @classmethod
    def create(cls, kind, body, scope="**", source="agent", agent="", session="",
               commit="", turn="", confirmed_by="agent", evidence="", status="proposed"):
        if kind not in KINDS:
            raise MemoryError(f"unknown kind {kind!r}")
        tier, _ = KINDS[kind]
        return cls(
            id=ulid(), kind=kind, scope=scope, status=status, tier=tier,
            body=body.strip(),
            provenance={k: v for k, v in dict(
                source=source, agent=agent, session=session, commit=commit,
                turn=str(turn) if turn != "" else "", evidence=evidence,
                captured_at=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                confirmed_by=confirmed_by).items() if v},
        )

    # -- validation -------------------------------------------------------
    def validate(self, filename: str | None = None) -> None:
        if not self.id:
            raise MemoryError("missing id")
        if filename and filename != self.id:
            raise MemoryError(f"id {self.id!r} does not match filename {filename!r}")
        if self.format != FORMAT_VERSION:
            raise MemoryError(f"unsupported format {self.format} (expected {FORMAT_VERSION})")
        if self.kind not in KINDS:
            raise MemoryError(f"unknown kind {self.kind!r}")
        if self.status not in STATUSES:
            raise MemoryError(f"unknown status {self.status!r}")
        if self.tier not in TIERS:
            raise MemoryError(f"unknown tier {self.tier!r}")
        if not self.body.strip():
            raise MemoryError("empty body")
        for req in ("source", "captured_at", "confirmed_by"):
            if req not in self.provenance:
                raise MemoryError(f"provenance missing {req!r}")
        # A memory must never be *about* a person. Authorship belongs in
        # provenance for traceability; it is never the subject.
        if self.extra.get("subject_person"):
            raise MemoryError("a memory may not have a person as its subject")
        # A memory is data. It can never grant capability.
        for banned in ("allowed_tools", "permissions", "hooks", "exec", "command"):
            if banned in self.extra:
                raise MemoryError(f"memory may not carry {banned!r}: memories cannot grant capability")

    @property
    def short(self) -> str:
        """A short handle for humans.

        Deliberately the TAIL of the ULID: the first 10 characters are the
        millisecond timestamp, so memories created in the same batch share them
        and a leading prefix is useless for disambiguation.
        """
        return self.id[-8:]

    @property
    def content_hash(self) -> str:
        return content_hash(self.body)

    @property
    def headline(self) -> str:
        """First paragraph, unwrapped -- bodies are hard-wrapped."""
        out = []
        for ln in self.body.splitlines():
            if not ln.strip():
                break
            out.append(ln.strip())
        return " ".join(out)

    def may_be_superseded_by(self, other: "Memory") -> bool:
        mine = PROVENANCE_STRENGTH.get(self.provenance.get("confirmed_by", "agent"), 1)
        theirs = PROVENANCE_STRENGTH.get(other.provenance.get("confirmed_by", "agent"), 1)
        return theirs >= mine

    # -- serialisation ----------------------------------------------------
    def to_text(self) -> str:
        lines = ["---",
                 f"id: {self.id}",
                 f"format: {self.format}",
                 f"kind: {self.kind}",
                 f"scope: \"{self.scope}\"",
                 f"status: {self.status}",
                 f"tier: {self.tier}",
                 f"content_hash: {self.content_hash}",
                 "provenance:"]
        for k in ("source", "agent", "session", "turn", "commit", "captured_at",
                  "confirmed_by", "evidence"):
            if self.provenance.get(k):
                lines.append(f"  {k}: {self.provenance[k]}")
        if self.supersedes:
            lines.append("supersedes: [" + ", ".join(self.supersedes) + "]")
        else:
            lines.append("supersedes: []")
        for k, v in sorted(self.extra.items()):
            lines.append(f"{k}: {v}")
        lines += ["---", "", self.body.strip(), ""]
        return "\n".join(lines)

    @classmethod
    def from_text(cls, text: str) -> "Memory":
        if not text.startswith("---\n"):
            raise MemoryError("missing frontmatter")
        fm_raw, sep, body = text[4:].partition("\n---\n")
        if not sep:
            raise MemoryError("unterminated frontmatter")
        m = cls(body=body.strip())
        cur_map = None
        for raw in fm_raw.splitlines():
            if not raw.strip() or raw.lstrip().startswith("#"):
                continue
            indent = len(raw) - len(raw.lstrip())
            key, _, val = raw.strip().partition(":")
            val = val.strip().strip('"').strip("'")
            if indent > 0 and cur_map is not None:
                cur_map[key] = val
                continue
            cur_map = None
            if key == "provenance":
                cur_map = m.provenance
            elif key == "id":
                m.id = val
            elif key == "format":
                m.format = int(val or 0)
            elif key == "kind":
                m.kind = val
            elif key == "scope":
                m.scope = val
            elif key == "status":
                m.status = val
            elif key == "tier":
                m.tier = val
            elif key == "supersedes":
                m.supersedes = [x.strip() for x in val.strip("[]").split(",") if x.strip()]
            elif key == "content_hash":
                pass  # derived; recomputed on write
            else:
                m.extra[key] = val
        return m

    @classmethod
    def load(cls, path) -> "Memory":
        import pathlib
        p = pathlib.Path(path)
        m = cls.from_text(p.read_text(encoding="utf-8"))
        m.validate(filename=p.stem)
        return m
