//! Staging commands: `add`, `rm`, `mv`, `restore`.

use super::{Message, open_repo, path_matches, repo_paths};
use crate::core::worktree;
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct AddOpts {
    /// Files or directories to stage ("." for everything)
    pub paths: Vec<PathBuf>,
    /// Stage all changes in the repository (new, modified and deleted files)
    #[arg(short = 'A', long)]
    pub all: bool,
    /// Only stage changes to already-tracked files (no new files)
    #[arg(short = 'u', long)]
    pub update: bool,
    /// Add files even if they are ignored
    #[arg(short = 'f', long)]
    pub force: bool,
    /// Show what would be staged without doing it
    #[arg(short = 'n', long = "dry-run")]
    pub dry_run: bool,
}

pub fn add(opts: AddOpts) -> Result<Message> {
    let repo = open_repo()?;
    let _lock = repo.lock()?;
    let mut idx = repo.load_index()?;
    let mut filters = repo_paths(&repo, &opts.paths)?;
    if opts.all || (opts.update && filters.is_empty()) {
        filters = vec![String::new()];
    }
    if filters.is_empty() {
        bail!("nothing specified, nothing added.\nhint: try 'jg add .' or 'jg add <file>'");
    }
    let files = worktree::scan(&repo, &idx)?;
    let mut added = Vec::new();
    let mut removed = Vec::new();

    // Explicitly named files that are ignored need --force.
    for (f, orig) in filters.iter().zip(opts.paths.iter()) {
        if f.is_empty() || files.contains_key(f) || idx.entries.contains_key(f) {
            continue;
        }
        let abs = repo.work_path(f)?;
        if abs.is_file() {
            if opts.force {
                let data = repo.read_work_file(f)?;
                let meta = std::fs::symlink_metadata(&abs)?;
                let mode = repo.work_file_mode(&meta, None);
                if !opts.dry_run {
                    let oid = repo.odb.write_blob(&data)?;
                    idx.set(f, mode, oid);
                }
                added.push(f.clone());
            } else {
                bail!("'{}' is ignored (see .jgignore/.gitignore); use -f to add it anyway", orig.display());
            }
        } else if !abs.exists() && !idx.entries.keys().any(|p| path_matches(p, std::slice::from_ref(f))) {
            bail!("pathspec '{}' did not match any files", orig.display());
        }
    }

    for (path, wf) in &files {
        if !path_matches(path, &filters) {
            continue;
        }
        let tracked = idx.entries.get(path);
        if opts.update && tracked.is_none() {
            continue;
        }
        let ((mode, oid), _) = worktree::hash_work_file(&repo, &idx, path, wf)?;
        let unchanged = tracked.is_some_and(|e| e.oid == oid && e.mode == mode && e.conflict.is_none());
        if unchanged {
            continue;
        }
        if !opts.dry_run {
            let data = repo.read_work_file(path)?;
            repo.odb.write_blob(&data)?;
            idx.set(path, mode, oid);
            if let Some(e) = idx.entries.get_mut(path) {
                e.size = wf.size;
                e.mtime_ns = wf.mtime_ns;
            }
        }
        added.push(path.clone());
    }
    // Tracked files deleted from disk are staged as deletions.
    let tracked: Vec<String> = idx.entries.keys().cloned().collect();
    for path in tracked {
        if path_matches(&path, &filters) && !files.contains_key(&path) {
            if !opts.dry_run {
                idx.entries.remove(&path);
            }
            removed.push(path);
        }
    }
    if !opts.dry_run {
        repo.save_index(&mut idx)?;
    }
    let mut text = String::new();
    for p in &added {
        text.push_str(&format!("add '{p}'\n"));
    }
    for p in &removed {
        text.push_str(&format!("remove '{p}'\n"));
    }
    if text.is_empty() {
        text = "nothing to add".into();
    }
    Ok(Message::new(text.trim_end(), json!({ "added": added, "removed": removed, "dry_run": opts.dry_run })))
}

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct RmOpts {
    /// Files to stop tracking
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,
    /// Only remove from the index; keep the file on disk
    #[arg(long)]
    pub cached: bool,
    /// Allow recursive removal of directories
    #[arg(short = 'r')]
    pub recursive: bool,
    /// Remove even if the file has uncommitted changes
    #[arg(short = 'f', long)]
    pub force: bool,
}

pub fn rm(opts: RmOpts) -> Result<Message> {
    let repo = open_repo()?;
    let _lock = repo.lock()?;
    let mut idx = repo.load_index()?;
    let head = repo.head_tree_map()?;
    let filters = repo_paths(&repo, &opts.paths)?;
    let mut removed = Vec::new();
    for (f, orig) in filters.iter().zip(&opts.paths) {
        let matches: Vec<String> = idx
            .entries
            .keys()
            .filter(|p| *p == f || (opts.recursive && path_matches(p, std::slice::from_ref(f))))
            .cloned()
            .collect();
        if matches.is_empty() {
            if idx.entries.keys().any(|p| path_matches(p, std::slice::from_ref(f))) {
                bail!("not removing '{}' recursively without -r", orig.display());
            }
            bail!("pathspec '{}' did not match any tracked files", orig.display());
        }
        for p in matches {
            if !opts.force && !opts.cached {
                let e = &idx.entries[&p];
                let staged = head.get(&p) != Some(&(e.mode, e.oid));
                let modified = repo.work_path(&p)?.exists()
                    && crate::core::Oid::hash_object("blob", &repo.read_work_file(&p)?) != e.oid;
                if staged || modified {
                    bail!("'{p}' has uncommitted changes; use -f to remove anyway or --cached to keep the file");
                }
            }
            idx.entries.remove(&p);
            if !opts.cached {
                repo.remove_work_file(&p)?;
            }
            removed.push(p);
        }
    }
    repo.save_index(&mut idx)?;
    let text = removed.iter().map(|p| format!("rm '{p}'")).collect::<Vec<_>>().join("\n");
    Ok(Message::new(text, json!({ "removed": removed, "cached": opts.cached })))
}

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct MvOpts {
    /// Existing tracked file
    pub source: PathBuf,
    /// New path
    pub destination: PathBuf,
}

pub fn mv(opts: MvOpts) -> Result<Message> {
    let repo = open_repo()?;
    let _lock = repo.lock()?;
    let mut idx = repo.load_index()?;
    let src = repo.to_repo_path(&opts.source)?;
    let mut dst = repo.to_repo_path(&opts.destination)?;
    if repo.work_path(&dst)?.is_dir() {
        let name = src.rsplit('/').next().unwrap_or(&src);
        dst = if dst.is_empty() { name.to_string() } else { format!("{dst}/{name}") };
    }
    let moving: Vec<String> =
        idx.entries.keys().filter(|p| path_matches(p, std::slice::from_ref(&src))).cloned().collect();
    if moving.is_empty() {
        bail!("'{}' is not tracked", opts.source.display());
    }
    if repo.work_path(&dst)?.exists() {
        bail!("destination '{}' already exists", opts.destination.display());
    }
    let dst_abs = repo.work_path(&dst)?;
    if let Some(parent) = dst_abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(repo.work_path(&src)?, &dst_abs)?;
    let mut moved = Vec::new();
    for p in moving {
        let e = idx.entries.remove(&p).unwrap();
        let newp = format!("{dst}{}", &p[src.len()..]);
        idx.entries.insert(newp.clone(), e);
        moved.push(json!({ "from": p, "to": newp }));
    }
    repo.save_index(&mut idx)?;
    Ok(Message::new(format!("renamed '{src}' -> '{dst}'"), json!({ "moved": moved })))
}

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct RestoreOpts {
    /// Files to restore
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,
    /// Unstage: restore the index from HEAD (or --source), keep work-tree changes
    #[arg(short = 'S', long)]
    pub staged: bool,
    /// Also restore the work tree (default when --staged is not given)
    #[arg(short = 'W', long)]
    pub worktree: bool,
    /// Restore from this commit instead of the index/HEAD
    #[arg(short = 's', long)]
    pub source: Option<String>,
}

/// `restore <paths>` discards work-tree edits; `restore --staged <paths>` unstages.
pub fn restore(opts: RestoreOpts) -> Result<Message> {
    let repo = open_repo()?;
    let _lock = repo.lock()?;
    let mut idx = repo.load_index()?;
    let filters = repo_paths(&repo, &opts.paths)?;
    let worktree_too = opts.worktree || !opts.staged;
    let source_map = match &opts.source {
        Some(s) => Some(repo.commit_tree_map(&crate::core::revparse::resolve_commit(&repo, s)?)?),
        None if opts.staged => Some(repo.head_tree_map()?),
        None => None,
    };
    let mut restored = Vec::new();
    // Candidate paths: anything in the index or the source tree under the filters.
    let mut candidates: Vec<String> = idx.entries.keys().cloned().collect();
    if let Some(m) = &source_map {
        candidates.extend(m.keys().cloned());
    }
    candidates.sort();
    candidates.dedup();
    let mut matched_any = vec![false; filters.len()];
    for p in candidates {
        let Some(fi) = filters.iter().position(|f| path_matches(&p, std::slice::from_ref(f))) else { continue };
        matched_any[fi] = true;
        let target = match &source_map {
            Some(m) => m.get(&p).copied(),
            None => idx.entries.get(&p).map(|e| (e.mode, e.oid)),
        };
        if opts.staged {
            match target {
                Some((m, o)) => idx.set(&p, m, o),
                None => {
                    idx.entries.remove(&p);
                }
            }
        }
        if worktree_too {
            match target {
                Some((m, o)) => repo.write_work_file(&p, m, &o)?,
                None if opts.source.is_some() || opts.staged => repo.remove_work_file(&p)?,
                None => {}
            }
        }
        restored.push(p);
    }
    for (i, hit) in matched_any.iter().enumerate() {
        if !hit {
            bail!("pathspec '{}' did not match any known files", opts.paths[i].display());
        }
    }
    repo.save_index(&mut idx)?;
    let what = if opts.staged && !opts.worktree { "unstaged" } else { "restored" };
    let text = restored.iter().map(|p| format!("{what} '{p}'")).collect::<Vec<_>>().join("\n");
    Ok(Message::new(text, json!({ "restored": restored, "staged": opts.staged, "worktree": worktree_too })))
}
