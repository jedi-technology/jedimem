"""Redaction, applied before ANY write -- including to the staging ref.

Read docs/research/05-distribution-security.md §6.4 before trusting this.
Existing scanners are structurally unable to protect a memory file: gitleaks'
entropy default discards human-chosen passwords, trufflehog needs a provider API
to verify, and GitHub push protection skips test/mock/spec paths. None was built
for the prose, hostnames and business context a memory actually contains.

So this is a *reducer*, not a control. The documented remedy for a leaked
credential is rotation, never history rewrite.
"""
from __future__ import annotations

import re

PATTERNS = [
    (re.compile(r"\bsk-[A-Za-z0-9_\-]{20,}"), "[REDACTED:openai-key]"),
    (re.compile(r"\bsk-ant-[A-Za-z0-9_\-]{20,}"), "[REDACTED:anthropic-key]"),
    (re.compile(r"\bgh[pousr]_[A-Za-z0-9]{20,}"), "[REDACTED:github-token]"),
    (re.compile(r"\bAKIA[0-9A-Z]{16}\b"), "[REDACTED:aws-key-id]"),
    (re.compile(r"\bxox[baprs]-[A-Za-z0-9\-]{10,}"), "[REDACTED:slack-token]"),
    (re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----"),
     "[REDACTED:private-key]"),
    (re.compile(r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}"),
     "[REDACTED:jwt]"),
    # credentials embedded in a URL
    (re.compile(r"://[^/\s:@]+:[^/\s@]+@"), "://[REDACTED:userinfo]@"),
    # key = value assignments
    (re.compile(r"(?i)\b((?:api|secret|access|private|auth)[_-]?(?:key|token|secret)"
                r"|password|passwd|bearer)\b(\s*[:=]\s*)(['\"]?)([^\s'\"]{8,})\3"),
     r"\1\2\3[REDACTED]\3"),
]


def redact(text: str) -> tuple:
    """Returns (clean_text, list_of_kinds_found)."""
    found = []
    out = text
    for pat, repl in PATTERNS:
        out, n = pat.subn(repl, out)
        if n:
            m = re.search(r"REDACTED:?([a-z\-]*)", repl)
            found.append((m.group(1) or "credential") if m else "credential")
    return out, found


def has_secret(text: str) -> bool:
    return bool(redact(text)[1])
