//! Where memories live: the staging ref, and the committed files.

use crate::memory::{Memory, MemoryError};
use crate::repo;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MEM_DIR: &str = ".jedimem/memories";
pub const STAGE_PREFIX: &str = "pending/";

pub struct Store {
    pub root: PathBuf,
    pub git_ref: String,
}

impl Store {
    pub fn new(root: &Path, git_ref: &str) -> Store {
        Store {
            root: root.to_path_buf(),
            git_ref: git_ref.to_string(),
        }
    }

    pub fn mem_dir(&self) -> PathBuf {
        self.root.join(MEM_DIR)
    }

    // -------------------------------------------------------- committed
    pub fn all(&self, include_inactive: bool) -> Result<Vec<Memory>, MemoryError> {
        let dir = self.mem_dir();
        if !dir.is_dir() {
            return Ok(vec![]);
        }
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| MemoryError(e.to_string()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|x| x == "md").unwrap_or(false))
            .collect();
        paths.sort();
        let mut out = Vec::new();
        for p in paths {
            let m = Memory::load(&p)?;
            if include_inactive || m.status == "active" {
                out.push(m);
            }
        }
        Ok(out)
    }

    pub fn by_id(&self, id: &str) -> Option<Memory> {
        let p = self.mem_dir().join(format!("{}.md", id));
        if p.exists() {
            Memory::load(&p).ok()
        } else {
            None
        }
    }

    pub fn hashes(&self) -> BTreeMap<String, Memory> {
        self.all(true)
            .unwrap_or_default()
            .into_iter()
            .map(|m| (m.content_hash(), m))
            .collect()
    }

    pub fn write(&self, m: &Memory) -> Result<PathBuf, MemoryError> {
        m.validate(None)?;
        let dir = self.mem_dir();
        std::fs::create_dir_all(&dir).map_err(|e| MemoryError(e.to_string()))?;
        let p = dir.join(format!("{}.md", m.id));
        std::fs::write(&p, m.to_text()).map_err(|e| MemoryError(e.to_string()))?;
        Ok(p)
    }

    pub fn set_status(&self, id: &str, status: &str, note: &str) -> Result<Memory, MemoryError> {
        let mut m = self
            .by_id(id)
            .ok_or_else(|| MemoryError(format!("no such memory {}", id)))?;
        m.status = status.to_string();
        if !note.is_empty() {
            m.extra.insert("note".into(), note.to_string());
        }
        self.write(&m)?;
        Ok(m)
    }

    // ---------------------------------------------------------- staging
    pub fn stage(&self, memories: &[Memory], message: &str) -> Result<String, repo::GitError> {
        if memories.is_empty() {
            return Ok(String::new());
        }
        let mut files = BTreeMap::new();
        for m in memories {
            files.insert(format!("{}{}.md", STAGE_PREFIX, m.id), m.to_text());
        }
        repo::stage_files(&files, message, &self.git_ref, Some(&self.root))
    }

    pub fn pending(&self) -> Vec<Memory> {
        let mut out = Vec::new();
        for path in repo::staged_files(&self.git_ref, Some(&self.root), STAGE_PREFIX) {
            if let Ok(text) = repo::staged_content(&path, &self.git_ref, Some(&self.root)) {
                if let Ok(m) = Memory::from_text(&format!("{}\n", text)) {
                    out.push(m);
                }
            }
        }
        out
    }

    pub fn clear_pending(&self, ids: &[String], message: &str) -> Result<(), repo::GitError> {
        if ids.is_empty() {
            return Ok(());
        }
        let paths: Vec<String> = ids
            .iter()
            .map(|i| format!("{}{}.md", STAGE_PREFIX, i))
            .collect();
        repo::drop_staged(&paths, &self.git_ref, Some(&self.root), message)?;
        Ok(())
    }

    /// Move staged candidates into committed files (a working-tree write, and
    /// therefore only ever on an explicit human action).
    pub fn promote(&self, ids: &[String], status: &str) -> Result<Vec<Memory>, MemoryError> {
        let pending: BTreeMap<String, Memory> = self
            .pending()
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect();
        let mut written = Vec::new();
        for id in ids {
            if let Some(m) = pending.get(id) {
                let mut m = m.clone();
                m.status = status.to_string();
                self.write(&m)?;
                written.push(m);
            }
        }
        let msg = format!("jedimem: promote {} memory(ies)", written.len());
        self.clear_pending(ids, &msg)
            .map_err(|e| MemoryError(e.to_string()))?;
        Ok(written)
    }
}
