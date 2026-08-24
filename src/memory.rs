//! The memory record: parse, validate, serialise.
//!
//! The on-disk format is the product's contract. Memories are committed to a
//! repo and live in git history forever, so a format change is a migration, not
//! a refactor -- hence the explicit `format:` stamp on every record.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

pub const FORMAT_VERSION: u32 = 1;

/// Delivery tiers. See docs/MEMORY-KINDS.md.
pub const TIERS: [&str; 4] = ["always", "scoped", "on_demand", "pre_action"];

pub const STATUSES: [&str; 6] = [
    "proposed",
    "active",
    "contested",
    "superseded",
    "rejected",
    "expired",
];

/// Kinds, with the tier they default to and whether a human must approve.
/// Kinds encoding *intent or authority* need a human: a transcript cannot
/// establish that a rule is a team rule rather than one reviewer's opinion.
pub const KINDS: [(&str, &str, bool); 18] = [
    ("convention", "scoped", false),
    ("requirement", "always", true),
    ("style", "scoped", false),
    ("workflow", "on_demand", false),
    ("topic", "on_demand", false),
    ("gotcha", "scoped", false),
    ("negative", "on_demand", true),
    ("decision", "on_demand", true),
    ("runbook", "on_demand", false),
    ("constraint", "always", true),
    ("glossary", "on_demand", false),
    ("ownership", "on_demand", false),
    ("flaky", "pre_action", false),
    ("external", "on_demand", false),
    ("perf", "scoped", true),
    ("migration", "always", true),
    ("preference", "always", false),     // local-only
    ("environment", "on_demand", false), // local-only
];

pub const LOCAL_ONLY_KINDS: [&str; 2] = ["preference", "environment"];

pub fn kind_info(kind: &str) -> Option<(&'static str, bool)> {
    KINDS
        .iter()
        .find(|(k, _, _)| *k == kind)
        .map(|(_, t, h)| (*t, *h))
}

pub fn provenance_strength(who: &str) -> u8 {
    match who {
        "verified" => 3,
        "human" => 2,
        _ => 1,
    }
}

#[derive(Debug)]
pub struct MemoryError(pub String);

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for MemoryError {}

fn err<T>(msg: impl Into<String>) -> Result<T, MemoryError> {
    Err(MemoryError(msg.into()))
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Time-ordered, collision-free without coordination.
///
/// Filename collisions are add/add merge conflicts, so IDs must be safe to
/// generate on many machines that share no state.
pub fn ulid() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut rand = [0u8; 10];
    // If the OS RNG fails we must not silently emit predictable ids that could
    // collide across machines, so fall back to nanosecond entropy and say so.
    if getrandom::getrandom(&mut rand).is_err() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        rand[..4].copy_from_slice(&nanos.to_be_bytes());
    }
    let mut n: u128 = (ms & ((1 << 48) - 1)) << 80;
    n |= rand.iter().fold(0u128, |acc, b| (acc << 8) | *b as u128);
    let mut out = [0u8; 26];
    for i in (0..26).rev() {
        out[i] = CROCKFORD[(n & 31) as usize];
        n >>= 5;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Stable identity for *what a memory says*. Two developers whose agents
/// independently learn the same fact produce the same hash -- which is how
/// duplicate detection works with no coordination.
pub fn content_hash(text: &str) -> String {
    let norm = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let digest = Sha256::digest(norm.as_bytes());
    digest
        .iter()
        .take(6)
        .map(|b| format!("{:02x}", b))
        .collect()
}

pub fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    format_utc(secs)
}

/// Civil-from-days (Howard Hinnant's algorithm). Avoids a chrono dependency for
/// the single format we actually emit.
pub fn format_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Memory {
    pub id: String,
    pub format: u32,
    pub kind: String,
    pub scope: String,
    pub status: String,
    pub tier: String,
    pub body: String,
    pub supersedes: Vec<String>,
    pub provenance: BTreeMap<String, String>,
    pub extra: BTreeMap<String, String>,
}

pub struct NewMemory<'a> {
    pub kind: &'a str,
    pub body: &'a str,
    pub scope: &'a str,
    pub source: &'a str,
    pub evidence: &'a str,
    pub confirmed_by: &'a str,
    pub agent: &'a str,
    pub commit: &'a str,
    pub status: &'a str,
}

impl<'a> Default for NewMemory<'a> {
    fn default() -> Self {
        NewMemory {
            kind: "convention",
            body: "",
            scope: "**",
            source: "agent",
            evidence: "",
            confirmed_by: "agent",
            agent: "",
            commit: "",
            status: "proposed",
        }
    }
}

impl Memory {
    pub fn create(n: NewMemory) -> Result<Memory, MemoryError> {
        let (tier, _) = match kind_info(n.kind) {
            Some(v) => v,
            None => return err(format!("unknown kind {:?}", n.kind)),
        };
        let mut provenance = BTreeMap::new();
        for (k, v) in [
            ("source", n.source),
            ("agent", n.agent),
            ("commit", n.commit),
            ("evidence", n.evidence),
            ("confirmed_by", n.confirmed_by),
        ] {
            if !v.is_empty() {
                provenance.insert(k.to_string(), v.to_string());
            }
        }
        provenance.insert("captured_at".into(), utc_now());
        Ok(Memory {
            id: ulid(),
            format: FORMAT_VERSION,
            kind: n.kind.into(),
            scope: n.scope.into(),
            status: n.status.into(),
            tier: tier.into(),
            body: n.body.trim().to_string(),
            supersedes: vec![],
            provenance,
            extra: BTreeMap::new(),
        })
    }

    /// A short handle for humans.
    ///
    /// Deliberately the TAIL of the ULID: the first 10 characters are the
    /// millisecond timestamp, so memories created in the same batch share them
    /// and a leading prefix cannot disambiguate.
    pub fn short(&self) -> String {
        let n = self.id.chars().count();
        self.id.chars().skip(n.saturating_sub(8)).collect()
    }

    pub fn content_hash(&self) -> String {
        content_hash(&self.body)
    }

    /// First paragraph, unwrapped -- bodies are hard-wrapped.
    pub fn headline(&self) -> String {
        let mut out = Vec::new();
        for line in self.body.lines() {
            if line.trim().is_empty() {
                break;
            }
            out.push(line.trim());
        }
        out.join(" ")
    }

    pub fn needs_human(&self) -> bool {
        kind_info(&self.kind).map(|(_, h)| h).unwrap_or(false)
    }

    /// A newer statement is not automatically a truer one.
    pub fn may_be_superseded_by(&self, other: &Memory) -> bool {
        let mine = provenance_strength(
            self.provenance
                .get("confirmed_by")
                .map(String::as_str)
                .unwrap_or("agent"),
        );
        let theirs = provenance_strength(
            other
                .provenance
                .get("confirmed_by")
                .map(String::as_str)
                .unwrap_or("agent"),
        );
        theirs >= mine
    }

    pub fn validate(&self, filename: Option<&str>) -> Result<(), MemoryError> {
        if self.id.is_empty() {
            return err("missing id");
        }
        if let Some(f) = filename {
            if f != self.id {
                return err(format!("id {:?} does not match filename {:?}", self.id, f));
            }
        }
        if self.format != FORMAT_VERSION {
            return err(format!(
                "unsupported format {} (expected {})",
                self.format, FORMAT_VERSION
            ));
        }
        if kind_info(&self.kind).is_none() {
            return err(format!("unknown kind {:?}", self.kind));
        }
        if !STATUSES.contains(&self.status.as_str()) {
            return err(format!("unknown status {:?}", self.status));
        }
        if !TIERS.contains(&self.tier.as_str()) {
            return err(format!("unknown tier {:?}", self.tier));
        }
        if self.body.trim().is_empty() {
            return err("empty body");
        }
        for req in ["source", "captured_at", "confirmed_by"] {
            if !self.provenance.contains_key(req) {
                return err(format!("provenance missing {:?}", req));
            }
        }
        // A memory must never be *about* a person. Authorship belongs in
        // provenance for traceability; it is never the subject.
        if self.extra.contains_key("subject_person") {
            return err("a memory may not have a person as its subject");
        }
        // A memory is data. It can never grant capability.
        for banned in ["allowed_tools", "permissions", "hooks", "exec", "command"] {
            if self.extra.contains_key(banned) {
                return err(format!(
                    "memory may not carry {:?}: memories cannot grant capability",
                    banned
                ));
            }
        }
        Ok(())
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("---\n");
        s.push_str(&format!("id: {}\n", self.id));
        s.push_str(&format!("format: {}\n", self.format));
        s.push_str(&format!("kind: {}\n", self.kind));
        s.push_str(&format!("scope: \"{}\"\n", self.scope));
        s.push_str(&format!("status: {}\n", self.status));
        s.push_str(&format!("tier: {}\n", self.tier));
        s.push_str(&format!("content_hash: {}\n", self.content_hash()));
        s.push_str("provenance:\n");
        for k in [
            "source",
            "agent",
            "session",
            "turn",
            "commit",
            "captured_at",
            "confirmed_by",
            "evidence",
        ] {
            if let Some(v) = self.provenance.get(k) {
                if !v.is_empty() {
                    s.push_str(&format!("  {}: {}\n", k, v));
                }
            }
        }
        s.push_str(&format!("supersedes: [{}]\n", self.supersedes.join(", ")));
        for (k, v) in &self.extra {
            s.push_str(&format!("{}: {}\n", k, v));
        }
        s.push_str("---\n\n");
        s.push_str(self.body.trim());
        s.push('\n');
        s
    }

    pub fn from_text(text: &str) -> Result<Memory, MemoryError> {
        let rest = match text.strip_prefix("---\n") {
            Some(r) => r,
            None => return err("missing frontmatter"),
        };
        let (fm_raw, body) = match rest.split_once("\n---\n") {
            Some(v) => v,
            None => return err("unterminated frontmatter"),
        };
        let mut m = Memory {
            format: 0,
            scope: "**".into(),
            body: body.trim().to_string(),
            ..Default::default()
        };
        let mut in_provenance = false;
        for raw in fm_raw.lines() {
            if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
                continue;
            }
            let indented = raw.starts_with(' ') || raw.starts_with('\t');
            let (key, val) = match raw.trim().split_once(':') {
                Some((k, v)) => (k.trim(), v.trim().trim_matches('"').trim_matches('\'')),
                None => continue,
            };
            if indented && in_provenance {
                m.provenance.insert(key.to_string(), val.to_string());
                continue;
            }
            in_provenance = false;
            match key {
                "provenance" => in_provenance = true,
                "id" => m.id = val.to_string(),
                "format" => m.format = val.parse().unwrap_or(0),
                "kind" => m.kind = val.to_string(),
                "scope" => m.scope = val.to_string(),
                "status" => m.status = val.to_string(),
                "tier" => m.tier = val.to_string(),
                "content_hash" => {} // derived; recomputed on write
                "supersedes" => {
                    m.supersedes = val
                        .trim_matches(|c| c == '[' || c == ']')
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                }
                other => {
                    m.extra.insert(other.to_string(), val.to_string());
                }
            }
        }
        Ok(m)
    }

    pub fn load(path: &std::path::Path) -> Result<Memory, MemoryError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| MemoryError(format!("{}: {}", path.display(), e)))?;
        let m = Memory::from_text(&text)?;
        let stem = path.file_stem().and_then(|s| s.to_str());
        m.validate(stem)?;
        Ok(m)
    }
}
