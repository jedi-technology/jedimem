//! Layered configuration.
//!
//! Precedence, weakest first: repo -> user-global -> user-per-repo -> env.
//!
//! One rule makes the layering safe: a *committed* layer may never narrow a
//! user's privacy. Config can enable a memory kind; it can never disable pause,
//! enable telemetry (there is none), or force a runtime that requires a secret.
//! A repo you clone must not be able to change what your machine sends.

use std::collections::BTreeMap;
use std::path::Path;

/// Keys a committed repo config is NOT allowed to set. Enforced, not documented.
pub const REPO_FORBIDDEN: [&str; 5] = ["paused", "runtime", "model", "api_key", "telemetry"];

#[derive(Debug, Clone)]
pub struct Config {
    map: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        let mut map = BTreeMap::new();
        for (k, v) in [
            ("repo_id", ""),
            ("always_chars", "6000"),
            ("scoped_chars_per_glob", "4000"),
            ("batch_window_minutes", "30"),
            ("staging_ref", crate::repo::STAGING_REF),
            ("compile_targets", "AGENTS.md,CLAUDE.md"),
            ("marker_begin", "<!-- BEGIN jedimem -->"),
            ("marker_end", "<!-- END jedimem -->"),
            ("runtime", "auto"),
            ("paused", "false"),
        ] {
            map.insert(k.to_string(), v.to_string());
        }
        Config { map }
    }
}

impl Config {
    pub fn get(&self, k: &str) -> &str {
        self.map.get(k).map(String::as_str).unwrap_or("")
    }
    pub fn num(&self, k: &str, default: usize) -> usize {
        self.get(k).parse().unwrap_or(default)
    }
    pub fn flag(&self, k: &str) -> bool {
        matches!(self.get(k), "true" | "yes" | "1")
    }
    pub fn targets(&self) -> Vec<String> {
        self.get("compile_targets")
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
    fn merge(&mut self, other: BTreeMap<String, String>) {
        self.map.extend(other);
    }
}

/// A deliberately tiny YAML subset: scalars, one nesting level, inline lists.
/// Depending on a YAML crate would enlarge the supply-chain surface for a file
/// format we fully control.
fn parse_yaml_ish(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut section = String::new();
    for raw in text.lines() {
        let line = match raw.trim_start().starts_with('#') {
            true => "",
            false => raw.split('#').next().unwrap_or(""),
        };
        if line.trim().is_empty() {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        let (key, val) = match line.trim().split_once(':') {
            Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
            None => continue,
        };
        let clean = |v: &str| v.trim().trim_matches('"').trim_matches('\'').to_string();
        if !indented {
            section.clear();
            if val.is_empty() {
                section = key;
            } else if val.starts_with('[') {
                let items: Vec<String> = val
                    .trim_matches(|c| c == '[' || c == ']')
                    .split(',')
                    .map(clean)
                    .filter(|s| !s.is_empty())
                    .collect();
                out.insert(flatten_key(&key), items.join(","));
            } else {
                out.insert(flatten_key(&key), clean(&val));
            }
        } else if !section.is_empty() {
            if val.starts_with('[') {
                let items: Vec<String> = val
                    .trim_matches(|c| c == '[' || c == ']')
                    .split(',')
                    .map(clean)
                    .filter(|s| !s.is_empty())
                    .collect();
                out.insert(flatten_nested(&section, &key), items.join(","));
            } else if !val.is_empty() {
                out.insert(flatten_nested(&section, &key), clean(&val));
            }
        }
    }
    out
}

fn flatten_key(k: &str) -> String {
    k.to_string()
}

/// Map the nested on-disk shape onto the flat keys the code uses.
fn flatten_nested(section: &str, key: &str) -> String {
    match (section, key) {
        ("compile", "targets") => "compile_targets".into(),
        ("budgets", k) => k.into(),
        ("capture", k) => k.into(),
        ("compile", k) => k.into(),
        (_, k) => k.into(),
    }
}

fn read(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .map(|t| parse_yaml_ish(&t))
        .unwrap_or_default()
}

pub fn load(repo_root: &Path) -> Config {
    let mut cfg = Config::default();

    let mut repo_layer = read(&repo_root.join(".jedimem").join("config.yml"));
    for k in REPO_FORBIDDEN {
        repo_layer.remove(k); // a cloned repo cannot set these on your machine
    }
    cfg.merge(repo_layer);

    let home = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| Path::new(&h).join(".config"))
                .unwrap_or_default()
        })
        .join("jedimem");
    cfg.merge(read(&home.join("config.yml")));
    cfg.merge(read(
        &repo_root.join(".jedimem").join("local").join("config.yml"),
    ));

    let keys: Vec<String> = cfg
        .map
        .keys()
        .cloned()
        .chain(["api_key".to_string()])
        .collect();
    for key in keys {
        if let Ok(v) = std::env::var(format!("JEDIMEM_{}", key.to_uppercase())) {
            cfg.map.insert(key, v);
        }
    }
    cfg
}
