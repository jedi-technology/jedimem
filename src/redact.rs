//! Redaction, applied before ANY write -- including to the staging ref.
//!
//! Read docs/research/05-distribution-security.md §6.4 before trusting this.
//! Existing scanners are structurally unable to protect a memory file:
//! gitleaks' entropy default discards human-chosen passwords, trufflehog needs
//! a provider API to verify, and GitHub push protection skips test/mock/spec
//! paths. None was built for the prose, hostnames and business context a memory
//! actually contains.
//!
//! So this is a *reducer*, not a control. The documented remedy for a leaked
//! credential is rotation, never history rewrite.

use regex::Regex;
use std::sync::OnceLock;

struct Rule {
    re: Regex,
    replacement: &'static str,
    label: &'static str,
}

fn rules() -> &'static Vec<Rule> {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let mk = |p: &str, r: &'static str, l: &'static str| Rule {
            re: Regex::new(p).expect("static redaction pattern must compile"),
            replacement: r,
            label: l,
        };
        vec![
            // Longest/most specific first: sk-ant- would otherwise be reported
            // as a generic sk- key.
            mk(r"\bsk-ant-[A-Za-z0-9_\-]{20,}", "[REDACTED:anthropic-key]", "anthropic-key"),
            mk(r"\bsk-[A-Za-z0-9_\-]{20,}", "[REDACTED:openai-key]", "openai-key"),
            mk(r"\bgh[pousr]_[A-Za-z0-9]{20,}", "[REDACTED:github-token]", "github-token"),
            mk(r"\bAKIA[0-9A-Z]{16}\b", "[REDACTED:aws-key-id]", "aws-key-id"),
            mk(r"\bxox[baprs]-[A-Za-z0-9\-]{10,}", "[REDACTED:slack-token]", "slack-token"),
            mk(
                r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
                "[REDACTED:private-key]",
                "private-key",
            ),
            mk(
                r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
                "[REDACTED:jwt]",
                "jwt",
            ),
            // credentials embedded in a URL
            mk(r"://[^/\s:@]+:[^/\s@]+@", "://[REDACTED:userinfo]@", "userinfo"),
            // key = value assignments
            mk(
                r#"(?i)\b((?:api|secret|access|private|auth)[_-]?(?:key|token|secret)|password|passwd|bearer)(\s*[:=]\s*)(['"]?)([^\s'"]{8,})"#,
                "$1$2$3[REDACTED]",
                "credential",
            ),
        ]
    })
}

/// Returns (clean_text, labels_found).
pub fn redact(text: &str) -> (String, Vec<String>) {
    let mut out = text.to_string();
    let mut found = Vec::new();
    for rule in rules() {
        if rule.re.is_match(&out) {
            out = rule.re.replace_all(&out, rule.replacement).into_owned();
            if !found.iter().any(|f| f == rule.label) {
                found.push(rule.label.to_string());
            }
        }
    }
    (out, found)
}

pub fn has_secret(text: &str) -> bool {
    !redact(text).1.is_empty()
}
