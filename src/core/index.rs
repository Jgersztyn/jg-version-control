//! The index ("staging area"): what the next commit's tree will contain.
//!
//! Stored as JSON at `.jg/index` (not git's binary format) — it is purely local
//! state, so readability wins. Git's binary index is only written on export.

use super::oid::Oid;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// One side of a conflicted path: (mode, oid).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Side {
    pub mode: u32,
    #[serde(with = "oid_hex")]
    pub oid: Oid,
}

/// Recorded when a merge could not resolve a path automatically.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub base: Option<Side>,
    pub ours: Option<Side>,
    pub theirs: Option<Side>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    #[serde(with = "oid_hex")]
    pub oid: Oid,
    pub mode: u32,
    /// Cached file size and mtime, used to skip re-hashing unchanged files.
    pub size: u64,
    pub mtime_ns: i128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<Conflict>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    /// When this index was last written; files modified in the same instant are re-hashed.
    pub written_at_ns: i128,
    pub entries: BTreeMap<String, IndexEntry>,
}

impl Index {
    pub fn load(path: &Path) -> Result<Index> {
        match std::fs::read(path) {
            Ok(data) => serde_json::from_slice(&data).context("index is corrupt"),
            Err(_) => Ok(Index { version: 1, ..Default::default() }),
        }
    }

    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.version = 1;
        self.written_at_ns = now_ns();
        let data = serde_json::to_vec(self)?;
        crate::util::atomic_write(path, &data)
    }

    pub fn has_conflicts(&self) -> bool {
        self.entries.values().any(|e| e.conflict.is_some())
    }

    pub fn conflicted_paths(&self) -> Vec<String> {
        self.entries.iter().filter(|(_, e)| e.conflict.is_some()).map(|(p, _)| p.clone()).collect()
    }

    /// Simple (path -> (mode, oid)) view, as used by tree building and diffs.
    pub fn as_map(&self) -> BTreeMap<String, (u32, Oid)> {
        self.entries.iter().map(|(p, e)| (p.clone(), (e.mode, e.oid))).collect()
    }

    /// Set an entry without cached stat info (forces a re-hash on next status).
    pub fn set(&mut self, path: &str, mode: u32, oid: Oid) {
        self.entries.insert(path.to_string(), IndexEntry { oid, mode, size: u64::MAX, mtime_ns: 0, conflict: None });
    }

    /// Replace the whole index with the contents of a flattened tree.
    pub fn reset_to(&mut self, map: &BTreeMap<String, (u32, Oid)>) {
        let old = std::mem::take(&mut self.entries);
        for (p, (mode, oid)) in map {
            // Keep cached stat data when the content is unchanged.
            match old.get(p) {
                Some(e) if e.oid == *oid && e.mode == *mode && e.conflict.is_none() => {
                    self.entries.insert(p.clone(), e.clone());
                }
                _ => self.set(p, *mode, *oid),
            }
        }
    }
}

pub fn now_ns() -> i128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i128).unwrap_or(0)
}

mod oid_hex {
    use super::Oid;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(o: &Oid, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&o.to_hex())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Oid, D::Error> {
        let s = String::deserialize(d)?;
        Oid::from_hex(&s).map_err(serde::de::Error::custom)
    }
}
