"""Layered configuration.

Precedence, weakest first: repo -> user-global -> user-per-repo -> environment.

One rule makes the layering safe: a *committed* layer may never narrow a user's
privacy. Config can enable a memory kind; it can never disable pause, enable
telemetry (there is none), or force a runtime that requires a secret. A repo you
clone must not be able to change what your machine sends.
"""
from __future__ import annotations

import os
import pathlib

DEFAULTS = {
    "repo_id": "",
    "always_chars": 6000,
    "scoped_chars_per_glob": 4000,
    "batch_window_minutes": 30,
    "staging_ref": "refs/jedimem/log",
    "compile_targets": ["AGENTS.md", "CLAUDE.md"],
    "marker_begin": "<!-- BEGIN jedimem -->",
    "marker_end": "<!-- END jedimem -->",
    "runtime": "auto",          # auto | claude | codex | pi | api | none
    "model": "claude-haiku-4-5-20251001",
    "paused": False,
}

# Keys a committed repo config is NOT allowed to set. Enforced, not documented.
REPO_FORBIDDEN = {"paused", "runtime", "model", "api_key", "telemetry"}


def _parse_scalar(v: str):
    s = v.strip().strip('"').strip("'")
    if s.lower() in ("true", "yes"):
        return True
    if s.lower() in ("false", "no"):
        return False
    if s.isdigit():
        return int(s)
    return s


def _load_yaml_ish(path: pathlib.Path) -> dict:
    """A deliberately tiny YAML subset: scalars, one nesting level, inline lists.

    Depending on PyYAML would mean depending on pip at install time, and the
    install path must work with nothing but a shell and git.
    """
    out, section = {}, None
    if not path.exists():
        return out
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0] if not raw.strip().startswith("#") else ""
        if not line.strip():
            continue
        indent = len(line) - len(line.lstrip())
        key, sep, val = line.strip().partition(":")
        if not sep:
            continue
        key = key.strip()
        if indent == 0:
            section = None
            if not val.strip():
                section = key
                out.setdefault(key, {})
            elif val.strip().startswith("["):
                out[key] = [x.strip().strip('"').strip("'")
                            for x in val.strip().strip("[]").split(",") if x.strip()]
            else:
                out[key] = _parse_scalar(val)
        elif section is not None:
            if val.strip().startswith("["):
                out[section][key] = [x.strip().strip('"').strip("'")
                                     for x in val.strip().strip("[]").split(",") if x.strip()]
            elif val.strip():
                out[section][key] = _parse_scalar(val)
    return out


def _flatten(raw: dict) -> dict:
    """Map the nested on-disk shape onto flat keys."""
    flat = {}
    for k, v in raw.items():
        if k == "budgets" and isinstance(v, dict):
            flat.update({kk: vv for kk, vv in v.items()})
        elif k == "compile" and isinstance(v, dict):
            if "targets" in v:
                flat["compile_targets"] = v["targets"]
            for kk in ("marker_begin", "marker_end"):
                if kk in v:
                    flat[kk] = v[kk]
        elif k == "capture" and isinstance(v, dict):
            flat.update(v)
        elif k == "kinds" and isinstance(v, dict):
            flat["kinds"] = v
        elif not isinstance(v, dict):
            flat[k] = v
    return flat


class Config(dict):
    @property
    def targets(self):
        return self.get("compile_targets", DEFAULTS["compile_targets"])


def load(repo_root: pathlib.Path) -> Config:
    cfg = Config(DEFAULTS)

    repo_layer = _flatten(_load_yaml_ish(repo_root / ".jedimem" / "config.yml"))
    for k in REPO_FORBIDDEN:
        repo_layer.pop(k, None)          # a cloned repo cannot set these
    cfg.update(repo_layer)

    home = pathlib.Path(os.environ.get("XDG_CONFIG_HOME",
                                       pathlib.Path.home() / ".config")) / "jedimem"
    cfg.update(_flatten(_load_yaml_ish(home / "config.yml")))
    cfg.update(_flatten(_load_yaml_ish(repo_root / ".jedimem" / "local" / "config.yml")))

    for key in list(DEFAULTS) + ["api_key"]:
        env = os.environ.get("JEDIMEM_" + key.upper())
        if env is not None:
            cfg[key] = _parse_scalar(env)
    return cfg
