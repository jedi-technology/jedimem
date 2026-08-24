"""Git access.

Every write path here is deliberately conservative, because the measurements in
docs/research/04-git-format.md showed the obvious approaches lose data:

  * `git add` + `git commit` from a background process lost 13 of 20 memories to
    .git/index.lock contention, silently.
  * A lock-free commit onto the checked-out branch was then silently reverted by
    the user's next ordinary `git commit` (their index was stale).

So: we never touch the index, never advance refs/heads/*, and never write to the
working tree except when a human explicitly asks (promote/compile).
"""
from __future__ import annotations

import os
import pathlib
import subprocess
import tempfile

STAGING_REF = "refs/jedimem/log"


class GitError(RuntimeError):
    pass


def _run(args, cwd=None, check=True, stdin=None):
    p = subprocess.run(["git"] + args, cwd=cwd, capture_output=True, text=True,
                       input=stdin)
    if check and p.returncode != 0:
        raise GitError(f"git {' '.join(args)}: {p.stderr.strip()}")
    return p.stdout.strip(), p.returncode


def is_repo(cwd=None) -> bool:
    _, rc = _run(["rev-parse", "--git-dir"], cwd=cwd, check=False)
    return rc == 0


def toplevel(cwd=None) -> pathlib.Path:
    out, rc = _run(["rev-parse", "--show-toplevel"], cwd=cwd, check=False)
    if rc != 0:
        raise GitError("not inside a git repository")
    return pathlib.Path(out)


def git_dir(cwd=None) -> pathlib.Path:
    """Per-worktree. The right home for per-checkout state."""
    out, _ = _run(["rev-parse", "--absolute-git-dir"], cwd=cwd)
    return pathlib.Path(out)


def common_dir(cwd=None) -> pathlib.Path:
    """Shared by all worktrees of a repo. The local scope key."""
    out, _ = _run(["rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=cwd)
    return pathlib.Path(out)


def worktrees(cwd=None) -> list:
    out, _ = _run(["worktree", "list", "--porcelain"], cwd=cwd)
    trees, cur = [], {}
    for line in out.splitlines():
        if not line.strip():
            if cur:
                trees.append(cur); cur = {}
            continue
        k, _, v = line.partition(" ")
        cur[k] = v
    if cur:
        trees.append(cur)
    return trees


def head_commit(cwd=None) -> str:
    out, rc = _run(["rev-parse", "HEAD"], cwd=cwd, check=False)
    return out if rc == 0 else ""


def current_branch(cwd=None) -> str:
    out, rc = _run(["rev-parse", "--abbrev-ref", "HEAD"], cwd=cwd, check=False)
    return out if rc == 0 else ""


def normalized_remote(cwd=None) -> str:
    """Strip the incidental differences between spellings of the same remote."""
    url, rc = _run(["remote", "get-url", "origin"], cwd=cwd, check=False)
    if rc != 0 or not url:
        return ""
    u = url.strip()
    for pre in ("git+ssh://", "ssh://", "https://", "http://", "git://"):
        if u.startswith(pre):
            u = u[len(pre):]
    if "@" in u.split("/")[0]:
        u = u.split("@", 1)[1]
    u = u.replace(":", "/", 1) if "/" not in u.split(":")[0] else u
    if u.endswith(".git"):
        u = u[:-4]
    return u.rstrip("/").lower()


def root_commit(cwd=None) -> str:
    """NOT a stable identity: a --depth 1 clone yields a different root."""
    out, rc = _run(["rev-list", "--max-parents=0", "HEAD"], cwd=cwd, check=False)
    return out.splitlines()[0] if rc == 0 and out else ""


# ---------------------------------------------------------------- staging ref

def _write_blob(content: str, cwd=None) -> str:
    out, _ = _run(["hash-object", "-w", "--stdin"], cwd=cwd, stdin=content)
    return out


def ref_exists(ref=STAGING_REF, cwd=None) -> str:
    out, rc = _run(["rev-parse", "--verify", "-q", ref], cwd=cwd, check=False)
    return out if rc == 0 else ""


def stage_files(files: dict, message: str, ref=STAGING_REF, cwd=None,
                retries: int = 100) -> str:
    """Commit {path: content} onto a side ref without touching index/HEAD/worktree.

    Compare-and-swap on the ref, retrying on contention. The parent MUST be read
    before the tree is built: doing it the other way round passes the CAS while
    committing a stale tree, which silently drops other writers' work.
    """
    blobs = {p: _write_blob(c, cwd=cwd) for p, c in files.items()}
    for _ in range(retries):
        parent = ref_exists(ref, cwd=cwd)
        # mktemp NAME ONLY: git refuses an existing zero-byte index file.
        fd, idx = tempfile.mkstemp(prefix="jedimem-idx-")
        os.close(fd); os.unlink(idx)
        env = dict(os.environ, GIT_INDEX_FILE=idx)
        try:
            if parent:
                subprocess.run(["git", "read-tree", parent], cwd=cwd, env=env,
                               capture_output=True, check=True)
            for path, blob in blobs.items():
                subprocess.run(["git", "update-index", "--add", "--cacheinfo",
                                f"100644,{blob},{path}"], cwd=cwd, env=env,
                               capture_output=True, check=True)
            tree = subprocess.run(["git", "write-tree"], cwd=cwd, env=env,
                                  capture_output=True, text=True, check=True).stdout.strip()
        finally:
            if os.path.exists(idx):
                os.unlink(idx)

        args = ["commit-tree", tree, "-m", message] + (["-p", parent] if parent else [])
        commit, _ = _run(args, cwd=cwd)
        _, rc = _run(["update-ref", ref, commit, parent or ""], cwd=cwd, check=False)
        if rc == 0:
            return commit
    raise GitError(f"could not update {ref} after {retries} attempts")


def staged_files(ref=STAGING_REF, cwd=None, prefix="") -> list:
    if not ref_exists(ref, cwd=cwd):
        return []
    out, _ = _run(["ls-tree", "-r", "--name-only", ref], cwd=cwd)
    return [p for p in out.splitlines() if p.startswith(prefix)]


def staged_content(path: str, ref=STAGING_REF, cwd=None) -> str:
    out, _ = _run(["show", f"{ref}:{path}"], cwd=cwd)
    return out


def drop_staged(paths, ref=STAGING_REF, cwd=None, message="jedimem: clear staged") -> str:
    """Rewrite the staging ref without `paths` (after promote/reject)."""
    keep = {}
    for p in staged_files(ref=ref, cwd=cwd):
        if p not in paths:
            keep[p] = staged_content(p, ref=ref, cwd=cwd) + "\n"
    parent = ref_exists(ref, cwd=cwd)
    fd, idx = tempfile.mkstemp(prefix="jedimem-idx-"); os.close(fd); os.unlink(idx)
    env = dict(os.environ, GIT_INDEX_FILE=idx)
    try:
        for path, content in keep.items():
            blob = _write_blob(content, cwd=cwd)
            subprocess.run(["git", "update-index", "--add", "--cacheinfo",
                            f"100644,{blob},{path}"], cwd=cwd, env=env,
                           capture_output=True, check=True)
        tree = subprocess.run(["git", "write-tree"], cwd=cwd, env=env,
                              capture_output=True, text=True, check=True).stdout.strip() \
            if keep else subprocess.run(["git", "hash-object", "-w", "-t", "tree", "/dev/null"],
                                        cwd=cwd, capture_output=True, text=True,
                                        check=True).stdout.strip()
    finally:
        if os.path.exists(idx):
            os.unlink(idx)
    args = ["commit-tree", tree, "-m", message] + (["-p", parent] if parent else [])
    commit, _ = _run(args, cwd=cwd)
    _run(["update-ref", ref, commit, parent or ""], cwd=cwd)
    return commit
