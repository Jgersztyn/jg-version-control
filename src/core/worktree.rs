//! Work-tree scanning and status computation (HEAD vs index vs files on disk).

use super::ignore::Ignore;
use super::index::Index;
use super::object::Kind;
use super::oid::Oid;
use super::repo::{JG_DIR, Repo};
use super::tree::FlatTree;
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// A file found on disk.
#[derive(Clone, Debug)]
pub struct WorkFile {
    pub size: u64,
    pub mtime_ns: i128,
    pub meta: fs::Metadata,
}

/// Walk the work tree. Returns every non-ignored file, plus tracked files even if ignored.
pub fn scan(repo: &Repo, index: &Index) -> Result<BTreeMap<String, WorkFile>> {
    let root = repo.work_dir()?.to_path_buf();
    let mut ignore = Ignore::default();
    if let Ok(t) = fs::read_to_string(repo.dir.join("info").join("exclude")) {
        ignore.add_rules(&t, "");
    }
    let mut out = BTreeMap::new();
    walk(index, &root, "", &mut ignore, &mut out)?;
    Ok(out)
}

fn has_tracked_under(index: &Index, dir: &str) -> bool {
    let prefix = format!("{dir}/");
    index.entries.range(prefix.clone()..).next().is_some_and(|(p, _)| p.starts_with(&prefix))
}

fn walk(index: &Index, dir: &Path, rel: &str, ignore: &mut Ignore, out: &mut BTreeMap<String, WorkFile>) -> Result<()> {
    ignore.add_dir(dir, rel);
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return Ok(()),
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if rel.is_empty() && (name == JG_DIR || name == ".git") {
            continue;
        }
        if name == JG_DIR || name == ".git" {
            continue;
        }
        let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let meta = match fs::symlink_metadata(entry.path()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            // Skip nested repositories entirely.
            if entry.path().join(JG_DIR).exists() || entry.path().join(".git").exists() {
                continue;
            }
            if ignore.is_ignored(&path, true) && !has_tracked_under(index, &path) {
                continue;
            }
            walk(index, &entry.path(), &path, ignore, out)?;
        } else {
            if ignore.is_ignored(&path, false) && !index.entries.contains_key(&path) {
                continue;
            }
            out.insert(path, WorkFile { size: meta.len(), mtime_ns: mtime_ns(&meta), meta });
        }
    }
    Ok(())
}

pub fn mtime_ns(meta: &fs::Metadata) -> i128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0)
}

/// Hash a work-tree file, using cached stat data from the index when it is safe.
/// Returns (mode, oid) and whether the stat cache should be refreshed.
pub fn hash_work_file(repo: &Repo, index: &Index, path: &str, wf: &WorkFile) -> Result<((u32, Oid), bool)> {
    let entry = index.entries.get(path);
    let mode = repo.work_file_mode(&wf.meta, entry.map(|e| e.mode));
    if let Some(e) = entry {
        // "Racy" protection: only trust the cache if the file was last modified
        // comfortably before the index was written.
        let safe = wf.mtime_ns < index.written_at_ns - 2_000_000_000;
        if e.conflict.is_none() && e.size == wf.size && e.mtime_ns == wf.mtime_ns && safe && e.mode == mode {
            return Ok(((e.mode, e.oid), false));
        }
    }
    let data = repo.read_work_file(path)?;
    let oid = Oid::hash_object(Kind::Blob.as_str(), &data);
    let refresh = entry.is_some_and(|e| e.oid == oid && e.mode == mode);
    Ok(((mode, oid), refresh))
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    /// Only the file mode changed (e.g. executable bit).
    Mode,
}

impl ChangeKind {
    pub fn label(&self) -> &'static str {
        match self {
            ChangeKind::Added => "new file",
            ChangeKind::Modified => "modified",
            ChangeKind::Deleted => "deleted",
            ChangeKind::Mode => "mode changed",
        }
    }
    pub fn letter(&self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Modified => 'M',
            ChangeKind::Deleted => 'D',
            ChangeKind::Mode => 'T',
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
}

/// Compare two flat trees.
pub fn diff_maps(old: &FlatTree, new: &FlatTree) -> Vec<Change> {
    let mut out = Vec::new();
    for (p, (mode, oid)) in old {
        match new.get(p) {
            None => out.push(Change { path: p.clone(), kind: ChangeKind::Deleted }),
            Some((m2, o2)) if o2 != oid => out.push(Change { path: p.clone(), kind: ChangeKind::Modified }),
            Some((m2, _)) if m2 != mode => out.push(Change { path: p.clone(), kind: ChangeKind::Mode }),
            _ => {}
        }
    }
    for p in new.keys() {
        if !old.contains_key(p) {
            out.push(Change { path: p.clone(), kind: ChangeKind::Added });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Status {
    /// HEAD vs index.
    pub staged: Vec<Change>,
    /// Index vs work tree.
    pub unstaged: Vec<Change>,
    pub untracked: Vec<String>,
    pub conflicts: Vec<String>,
    /// Snapshot of the work tree as (mode, oid) for tracked files, reused by callers.
    #[serde(skip)]
    pub work_map: FlatTree,
}

impl Status {
    pub fn is_clean(&self) -> bool {
        self.staged.is_empty() && self.unstaged.is_empty() && self.conflicts.is_empty()
    }
}

/// Compute full status. Refreshes the index's stat cache as a side effect when possible.
pub fn status(repo: &Repo, index: &mut Index) -> Result<Status> {
    let head = repo.head_tree_map()?;
    let files = scan(repo, index)?;
    let mut st = Status::default();
    let mut dirty_cache = false;

    let idx_map = index.as_map();
    st.staged = diff_maps(&head, &idx_map);
    st.staged.retain(|c| index.entries.get(&c.path).is_none_or(|e| e.conflict.is_none()));
    st.conflicts = index.conflicted_paths();

    for (path, entry) in index.entries.clone() {
        if entry.conflict.is_some() {
            continue;
        }
        match files.get(&path) {
            None => st.unstaged.push(Change { path: path.clone(), kind: ChangeKind::Deleted }),
            Some(wf) => {
                let ((mode, oid), refresh) = hash_work_file(repo, index, &path, wf)?;
                st.work_map.insert(path.clone(), (mode, oid));
                if refresh && let Some(e) = index.entries.get_mut(&path) {
                    e.size = wf.size;
                    e.mtime_ns = wf.mtime_ns;
                    dirty_cache = true;
                }
                if oid != entry.oid {
                    st.unstaged.push(Change { path, kind: ChangeKind::Modified });
                } else if mode != entry.mode {
                    st.unstaged.push(Change { path, kind: ChangeKind::Mode });
                }
            }
        }
    }
    for p in files.keys() {
        if !index.entries.contains_key(p) {
            st.untracked.push(p.clone());
        }
    }
    if dirty_cache {
        // Best effort: if another process holds the lock, skip the cache refresh.
        if let Ok(_g) = repo.lock() {
            repo.save_index(index)?;
        }
    }
    Ok(st)
}

/// Flat map of the current work tree for tracked + untracked files (hashing as needed).
pub fn work_tree_map(repo: &Repo, index: &Index, include_untracked: bool) -> Result<FlatTree> {
    let files = scan(repo, index)?;
    let mut out = FlatTree::new();
    for (path, wf) in &files {
        if !include_untracked && !index.entries.contains_key(path) {
            continue;
        }
        let ((mode, oid), _) = hash_work_file(repo, index, path, wf)?;
        out.insert(path.clone(), (mode, oid));
    }
    Ok(out)
}
