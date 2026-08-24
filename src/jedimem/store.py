"""Where memories live: the staging ref, and the committed files."""
from __future__ import annotations

import pathlib

from . import repo
from .memory import Memory, MemoryError

MEM_DIR = ".jedimem/memories"
STAGE_PREFIX = "pending/"


class Store:
    def __init__(self, root: pathlib.Path, staging_ref=repo.STAGING_REF):
        self.root = pathlib.Path(root)
        self.ref = staging_ref

    # ------------------------------------------------------------ committed
    @property
    def mem_dir(self) -> pathlib.Path:
        return self.root / MEM_DIR

    def all(self, include_inactive=False) -> list:
        out = []
        if not self.mem_dir.is_dir():
            return out
        for p in sorted(self.mem_dir.glob("*.md")):
            m = Memory.load(p)
            if include_inactive or m.status == "active":
                out.append(m)
        return out

    def by_id(self, mid: str):
        p = self.mem_dir / f"{mid}.md"
        return Memory.load(p) if p.exists() else None

    def hashes(self) -> dict:
        return {m.content_hash: m for m in self.all(include_inactive=True)}

    def write(self, m: Memory) -> pathlib.Path:
        m.validate()
        self.mem_dir.mkdir(parents=True, exist_ok=True)
        p = self.mem_dir / f"{m.id}.md"
        p.write_text(m.to_text(), encoding="utf-8")
        return p

    def set_status(self, mid: str, status: str, note: str = "") -> Memory:
        m = self.by_id(mid)
        if m is None:
            raise MemoryError(f"no such memory {mid}")
        m.status = status
        if note:
            m.extra["note"] = note
        self.write(m)
        return m

    # -------------------------------------------------------------- staging
    def stage(self, memories, message="jedimem: capture") -> str:
        files = {f"{STAGE_PREFIX}{m.id}.md": m.to_text() for m in memories}
        if not files:
            return ""
        return repo.stage_files(files, message, ref=self.ref, cwd=self.root)

    def pending(self) -> list:
        out = []
        for path in repo.staged_files(ref=self.ref, cwd=self.root, prefix=STAGE_PREFIX):
            try:
                out.append(Memory.from_text(
                    repo.staged_content(path, ref=self.ref, cwd=self.root) + "\n"))
            except MemoryError:
                continue
        return out

    def clear_pending(self, ids, message="jedimem: resolve pending"):
        paths = [f"{STAGE_PREFIX}{i}.md" for i in ids]
        if paths:
            repo.drop_staged(paths, ref=self.ref, cwd=self.root, message=message)

    def promote(self, ids, status="active") -> list:
        """Move staged candidates into committed files (working tree write)."""
        pend = {m.id: m for m in self.pending()}
        written = []
        for i in ids:
            m = pend.get(i)
            if m is None:
                continue
            m.status = status
            self.write(m)
            written.append(m)
        self.clear_pending(ids, message=f"jedimem: promote {len(written)} memory(ies)")
        return written
