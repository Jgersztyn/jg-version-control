//! Commands. Each command module exposes:
//! - an options struct deriving `clap::Args` (CLI) **and** `serde::Deserialize`
//!   (so a future MCP server / API can build the same options from JSON), and
//! - a `run(...) -> Result<Report>` function returning a `Serialize` report.
//!
//! The binary renders reports either as human text or as JSON (`--json`).

pub mod agent;
pub mod branch;
pub mod commit;
pub mod config;
pub mod diff;
pub mod gitexport;
pub mod gitimport;
pub mod init;
pub mod log;
pub mod merge;
pub mod reauthor;
pub mod remote;
pub mod reset;
pub mod stage;
pub mod stash;
pub mod status;
pub mod sync;
pub mod update;

use crate::core::index::Index;
use crate::core::object::{Commit, normalize_message};
use crate::core::tree::FlatTree;
use crate::core::worktree;
use crate::core::{Oid, Repo};
use anyhow::{Result, bail};
use serde::Serialize;

/// Output settings passed to `Report::human`.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub color: bool,
}

impl Style {
    fn wrap(&self, code: &str, s: &str) -> String {
        if self.color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
    }
    pub fn green(&self, s: &str) -> String {
        self.wrap("32", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.wrap("31", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.wrap("33", s)
    }
    pub fn cyan(&self, s: &str) -> String {
        self.wrap("36", s)
    }
    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
}

/// Every command result implements this: JSON comes from `Serialize`,
/// human output from `human()`.
pub trait Report: Serialize {
    fn human(&self, style: Style) -> String;
}

/// Generic report: a message for humans plus arbitrary JSON data.
#[derive(Serialize)]
pub struct Message {
    #[serde(skip)]
    pub text: String,
    #[serde(flatten)]
    pub data: serde_json::Value,
}

impl Message {
    pub fn new(text: impl Into<String>, data: serde_json::Value) -> Message {
        Message { text: text.into(), data }
    }
}

impl Report for Message {
    fn human(&self, _: Style) -> String {
        self.text.clone()
    }
}

/// Open the repository containing the current directory.
pub fn open_repo() -> Result<Repo> {
    let cwd = std::env::current_dir()?;
    Repo::discover(&cwd)
}

/// Write a commit object and move HEAD (or the current branch) to it.
pub fn make_commit(repo: &Repo, tree: Oid, parents: Vec<Oid>, message: &str, reflog: &str) -> Result<Oid> {
    let sig = repo.signature()?;
    make_commit_with_author(repo, tree, parents, message, sig, reflog)
}

pub fn make_commit_with_author(
    repo: &Repo,
    tree: Oid,
    parents: Vec<Oid>,
    message: &str,
    author: crate::core::object::Signature,
    reflog: &str,
) -> Result<Oid> {
    let message = normalize_message(message);
    if message.is_empty() {
        bail!("aborting commit due to empty commit message");
    }
    let committer = repo.signature()?;
    let c = Commit { tree, parents, author, committer, extra_headers: vec![], message: message.clone() };
    let oid = repo.odb.write_commit(&c)?;
    repo.refs.update_head(&oid, &repo.who(), &format!("{reflog}: {}", c.summary()))?;
    Ok(oid)
}

/// Move the work tree and index from tree `from` to tree `to`.
///
/// Without `force`, refuses (before touching anything) if a local change would be
/// lost: staged or unstaged edits to a path that differs between the trees, or an
/// untracked file in the way. Local changes to other paths are carried along.
/// With `force`, the index and tracked files are reset to `to` exactly (`reset --hard`).
pub fn checkout_tree(repo: &Repo, idx: &mut Index, from: &FlatTree, to: &FlatTree, force: bool) -> Result<()> {
    let files = worktree::scan(repo, idx)?;
    let mut paths: Vec<&String> = from.keys().chain(to.keys()).chain(idx.entries.keys()).collect();
    paths.sort();
    paths.dedup();
    let paths: Vec<String> = paths.into_iter().cloned().collect();

    let mut to_write: Vec<(String, u32, Oid)> = Vec::new();
    let mut to_remove: Vec<String> = Vec::new();
    let mut problems: Vec<String> = Vec::new();

    for p in &paths {
        let f = from.get(p);
        let t = to.get(p);
        let in_index = idx.entries.get(p).map(|e| (e.mode, e.oid));
        let work = match files.get(p) {
            Some(wf) => Some(worktree::hash_work_file(repo, idx, p, wf)?.0),
            None => None,
        };
        if force {
            if work.as_ref() != t || in_index.as_ref() != t {
                match t {
                    Some((m, o)) => to_write.push((p.clone(), *m, *o)),
                    None if in_index.is_some() || f.is_some() => to_remove.push(p.clone()),
                    None => {}
                }
            }
            continue;
        }
        if f == t {
            continue; // unaffected by the switch; local changes are kept
        }
        let conflicted = idx.entries.get(p).is_some_and(|e| e.conflict.is_some());
        let index_clean = in_index.as_ref() == f || in_index.as_ref() == t;
        let work_clean = work.as_ref() == in_index.as_ref() || work.as_ref() == t;
        if f.is_none() && in_index.is_none() {
            // Untracked file in the way?
            if work.is_some() && work.as_ref() != t {
                problems.push(format!("untracked file would be overwritten: {p}"));
                continue;
            }
        } else if conflicted || !index_clean || !work_clean {
            problems.push(format!("local changes would be overwritten: {p}"));
            continue;
        }
        match t {
            Some((m, o)) => to_write.push((p.clone(), *m, *o)),
            None => to_remove.push(p.clone()),
        }
    }
    if !problems.is_empty() {
        bail!(
            "{}\nCommit or stash your changes first (jg stash), or use --force to discard them.",
            problems.join("\n")
        );
    }
    for p in &to_remove {
        repo.remove_work_file(p)?;
        idx.entries.remove(p);
    }
    // Attribute files first, so line-ending rules apply to the files written after them.
    to_write.sort_by_key(|(p, _, _)| !p.ends_with(".gitattributes"));
    for (p, m, o) in &to_write {
        repo.write_work_file(p, *m, o)?;
        idx.set(p, *m, *o);
        // Cache stat data so the next status is fast.
        if let Ok(meta) = std::fs::symlink_metadata(repo.work_path(p)?)
            && let Some(e) = idx.entries.get_mut(p)
        {
            e.size = meta.len();
            e.mtime_ns = worktree::mtime_ns(&meta);
        }
    }
    Ok(())
}

/// Refuse to run if the index has unresolved conflicts.
pub fn ensure_no_conflicts(idx: &Index) -> Result<()> {
    if idx.has_conflicts() {
        bail!(
            "you have unresolved conflicts in: {}\nFix them, then 'jg add <file>' and 'jg commit' (or 'jg merge --abort').",
            idx.conflicted_paths().join(", ")
        );
    }
    Ok(())
}

/// Path filter: does `path` match any of the given repo-relative prefixes?
pub fn path_matches(path: &str, filters: &[String]) -> bool {
    filters.is_empty()
        || filters
            .iter()
            .any(|f| f.is_empty() || path == f || path.starts_with(&format!("{}/", f.trim_end_matches('/'))))
}

/// Convert user path arguments to repo paths.
pub fn repo_paths(repo: &Repo, paths: &[std::path::PathBuf]) -> Result<Vec<String>> {
    paths.iter().map(|p| repo.to_repo_path(p)).collect()
}

/// Human summary line for a new commit: `[branch abc1234] summary`.
pub fn commit_line(repo: &Repo, oid: &Oid, message: &str) -> String {
    let branch = repo.current_branch().ok().flatten().unwrap_or_else(|| "detached HEAD".into());
    format!("[{branch} {}] {}", oid.short(), message.lines().next().unwrap_or(""))
}
