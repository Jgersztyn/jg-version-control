//! `jg stash` — shelve uncommitted changes and restore them later.
//!
//! Each stash is a git-style commit `W` whose tree is the work-tree state, with
//! parents `[HEAD, I]` where `I` is a commit of the index state. The stack is
//! stored newest-first in `.jg/stash` (one id per line).

use super::{Message, Report, Style, checkout_tree, ensure_no_conflicts, open_repo};
use crate::core::object::{Commit, normalize_message};
use crate::core::tree::{self, FlatTree};
use crate::core::{Oid, Repo, worktree};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(clap::Subcommand, Deserialize, Debug)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum StashCmd {
    /// Save local changes and reset to HEAD (default)
    Push(PushOpts),
    /// List stashes
    List,
    /// Show the changes in a stash
    Show { index: Option<usize> },
    /// Apply a stash and remove it from the list
    Pop { index: Option<usize> },
    /// Apply a stash but keep it in the list
    Apply { index: Option<usize> },
    /// Delete a stash
    Drop { index: Option<usize> },
    /// Delete all stashes
    Clear,
}

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct PushOpts {
    /// Description for the stash
    #[arg(short = 'm', long)]
    pub message: Option<String>,
    /// Also stash (and remove) untracked files
    #[arg(short = 'u', long = "include-untracked")]
    pub include_untracked: bool,
}

fn stack_path(repo: &Repo) -> std::path::PathBuf {
    repo.dir.join("stash")
}

pub fn load_stack(repo: &Repo) -> Result<Vec<Oid>> {
    let s = std::fs::read_to_string(stack_path(repo)).unwrap_or_default();
    s.lines().filter(|l| !l.trim().is_empty()).map(Oid::from_hex).collect()
}

fn save_stack(repo: &Repo, stack: &[Oid]) -> Result<()> {
    let s: String = stack.iter().map(|o| format!("{o}\n")).collect();
    crate::util::atomic_write(&stack_path(repo), s.as_bytes())
}

#[derive(Serialize)]
pub struct StashEntry {
    pub index: usize,
    pub commit: Oid,
    pub message: String,
}

#[derive(Serialize)]
pub struct StashList {
    pub stashes: Vec<StashEntry>,
}

impl Report for StashList {
    fn human(&self, s: Style) -> String {
        if self.stashes.is_empty() {
            return "no stashes".into();
        }
        self.stashes
            .iter()
            .map(|e| format!("{}: {}", s.yellow(&format!("stash@{{{}}}", e.index)), e.message))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub enum StashResult {
    List(StashList),
    Show(super::diff::DiffReport),
    Done(Message),
}

pub fn run(cmd: Option<StashCmd>) -> Result<StashResult> {
    let repo = open_repo()?;
    match cmd.unwrap_or(StashCmd::Push(PushOpts::default())) {
        StashCmd::Push(o) => push(&repo, o).map(StashResult::Done),
        StashCmd::List => {
            let stack = load_stack(&repo)?;
            let mut stashes = Vec::new();
            for (i, oid) in stack.iter().enumerate() {
                let c = repo.odb.read_commit(oid)?;
                stashes.push(StashEntry { index: i, commit: *oid, message: c.summary().to_string() });
            }
            Ok(StashResult::List(StashList { stashes }))
        }
        StashCmd::Show { index } => {
            let w = pick(&repo, index)?;
            let c = repo.odb.read_commit(&w)?;
            let base = repo.commit_tree_map(&c.parents[0])?;
            let new = tree::flatten(&repo.odb, &c.tree)?;
            let files = super::diff::diff_sources(&repo, &base, &super::diff::Source::Tree(&new), &[], 3)?;
            Ok(StashResult::Show(super::diff::report(files, 1)))
        }
        StashCmd::Pop { index } => apply(&repo, index, true).map(StashResult::Done),
        StashCmd::Apply { index } => apply(&repo, index, false).map(StashResult::Done),
        StashCmd::Drop { index } => {
            let _lock = repo.lock()?;
            let mut stack = load_stack(&repo)?;
            let i = index.unwrap_or(0);
            if i >= stack.len() {
                bail!("stash@{{{i}}} does not exist");
            }
            let oid = stack.remove(i);
            save_stack(&repo, &stack)?;
            Ok(StashResult::Done(Message::new(
                format!("Dropped stash@{{{i}}} ({})", oid.short()),
                json!({ "dropped": oid }),
            )))
        }
        StashCmd::Clear => {
            let _lock = repo.lock()?;
            let n = load_stack(&repo)?.len();
            save_stack(&repo, &[])?;
            Ok(StashResult::Done(Message::new(format!("Cleared {n} stash(es)"), json!({ "cleared": n }))))
        }
    }
}

fn pick(repo: &Repo, index: Option<usize>) -> Result<Oid> {
    let stack = load_stack(repo)?;
    let i = index.unwrap_or(0);
    stack.get(i).copied().with_context(|| {
        if stack.is_empty() { "no stashes".to_string() } else { format!("stash@{{{i}}} does not exist") }
    })
}

fn push(repo: &Repo, opts: PushOpts) -> Result<Message> {
    let _lock = repo.lock()?;
    let mut idx = repo.load_index()?;
    ensure_no_conflicts(&idx)?;
    let head = repo.head_oid()?.context("cannot stash before the first commit")?;
    let head_map = repo.commit_tree_map(&head)?;
    let idx_map = idx.as_map();
    let work_map = worktree::work_tree_map(repo, &idx, opts.include_untracked)?;
    // Work-tree snapshot = tracked files as on disk (+ untracked if requested).
    let mut w_map: FlatTree = FlatTree::new();
    for (p, v) in &work_map {
        if opts.include_untracked || idx.entries.contains_key(p) {
            w_map.insert(p.clone(), *v);
        }
    }
    if idx_map == head_map && w_map == head_map {
        return Ok(Message::new("No local changes to save", json!({ "stashed": null })));
    }
    // Make sure every work-tree blob is stored.
    for (p, (_, oid)) in &w_map {
        if !repo.odb.exists(oid) {
            repo.odb.write_blob(&repo.read_work_file(p)?)?;
        }
    }
    let sig = repo.signature()?;
    let branch = repo.current_branch()?.unwrap_or_else(|| "(no branch)".into());
    let head_c = repo.odb.read_commit(&head)?;
    let desc = match &opts.message {
        Some(m) => format!("On {branch}: {m}"),
        None => format!("WIP on {branch}: {} {}", head.short(), head_c.summary()),
    };
    let i_commit = Commit {
        tree: tree::build(&repo.odb, &idx_map)?,
        parents: vec![head],
        author: sig.clone(),
        committer: sig.clone(),
        extra_headers: vec![],
        message: normalize_message(&format!("index on {branch}: {} {}", head.short(), head_c.summary())),
    };
    let i_oid = repo.odb.write_commit(&i_commit)?;
    let w_commit = Commit {
        tree: tree::build(&repo.odb, &w_map)?,
        parents: vec![head, i_oid],
        author: sig.clone(),
        committer: sig,
        extra_headers: vec![],
        message: normalize_message(&desc),
    };
    let w_oid = repo.odb.write_commit(&w_commit)?;
    let mut stack = load_stack(repo)?;
    stack.insert(0, w_oid);
    save_stack(repo, &stack)?;

    // Reset tracked files to HEAD, then remove stashed untracked files.
    checkout_tree(repo, &mut idx, &idx_map, &head_map, true)?;
    idx.reset_to(&head_map);
    if opts.include_untracked {
        for p in w_map.keys() {
            if !head_map.contains_key(p) {
                repo.remove_work_file(p)?;
            }
        }
    }
    repo.save_index(&mut idx)?;
    Ok(Message::new(
        format!("Saved working directory and index state {desc}"),
        json!({ "stashed": w_oid, "message": desc }),
    ))
}

fn apply(repo: &Repo, index: Option<usize>, drop: bool) -> Result<Message> {
    let _lock = repo.lock()?;
    let mut idx = repo.load_index()?;
    ensure_no_conflicts(&idx)?;
    let i = index.unwrap_or(0);
    let w = pick(repo, index)?;
    let wc = repo.odb.read_commit(&w)?;
    let base = repo.commit_tree_map(&wc.parents[0])?;
    let stashed = tree::flatten(&repo.odb, &wc.tree)?;
    let head_map = repo.head_tree_map()?;
    if idx.as_map() != head_map {
        bail!("you have staged changes; commit or unstage them before applying a stash");
    }
    // Untracked files that the stash would recreate must not already exist.
    for p in stashed.keys() {
        if !base.contains_key(p) && !idx.entries.contains_key(p) && repo.work_path(p)?.exists() {
            bail!("'{p}' already exists in the work tree and would be overwritten by the stash");
        }
    }
    let conflicts = super::merge::apply_three_way(
        repo,
        &mut idx,
        &base,
        &head_map,
        &stashed,
        "Updated upstream",
        "Stashed changes",
    )?;
    // Like git: changes come back as unstaged edits, but new files stay tracked.
    for (p, e) in idx.entries.clone() {
        if e.conflict.is_some() {
            continue;
        }
        match head_map.get(&p) {
            Some((m, o)) => idx.set(&p, *m, *o),
            None if stashed.contains_key(&p) && !base.contains_key(&p) => {
                // Was untracked when stashed (and not in the index) -> leave untracked.
                let was_staged = repo.commit_tree_map(&wc.parents[1]).map(|m| m.contains_key(&p)).unwrap_or(false);
                if !was_staged {
                    idx.entries.remove(&p);
                }
            }
            None => {}
        }
    }
    repo.save_index(&mut idx)?;
    if !conflicts.is_empty() {
        return Ok(Message::new(
            format!(
                "CONFLICT applying stash in: {}\nResolve the files, then 'jg add' them. The stash was kept (drop it with 'jg stash drop').",
                conflicts.join(", ")
            ),
            json!({ "result": "conflict", "conflicts": conflicts }),
        ));
    }
    if drop {
        let mut stack = load_stack(repo)?;
        stack.retain(|o| *o != w);
        save_stack(repo, &stack)?;
    }
    Ok(Message::new(
        format!("Applied stash@{{{i}}}{}", if drop { " and dropped it" } else { "" }),
        json!({ "result": "applied", "stash": w, "dropped": drop }),
    ))
}
