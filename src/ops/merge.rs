//! `merge`, `cherry-pick`, `revert` — all built on the same three-way tree merge.
//!
//! While a conflicted operation is in progress these files live in `.jg/`:
//! `OPERATION` (merge|cherry-pick|revert), `ORIG_HEAD`, `MERGE_MSG`, and for
//! merges `MERGE_HEAD` (the second parent of the eventual merge commit).

use super::{Message, checkout_tree, ensure_no_conflicts, make_commit, make_commit_with_author, open_repo};
use crate::core::history;
use crate::core::index::Index;
use crate::core::merge::{file_dir_clash, merge_trees};
use crate::core::revparse::resolve_commit;
use crate::core::tree::{self, FlatTree};
use crate::core::{Oid, Repo};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;

pub fn in_progress(repo: &Repo) -> Option<String> {
    std::fs::read_to_string(repo.state_file("OPERATION")).ok().map(|s| s.trim().to_string())
}

pub fn read_merge_head(repo: &Repo) -> Result<Option<Oid>> {
    match std::fs::read_to_string(repo.state_file("MERGE_HEAD")) {
        Ok(s) => Ok(Some(Oid::from_hex(s.trim())?)),
        Err(_) => Ok(None),
    }
}

pub fn clear_state(repo: &Repo) {
    for f in ["OPERATION", "MERGE_HEAD", "MERGE_MSG"] {
        let _ = std::fs::remove_file(repo.state_file(f));
    }
}

fn save_state(repo: &Repo, op: &str, head: &Oid, merge_head: Option<&Oid>, msg: &str) -> Result<()> {
    std::fs::write(repo.state_file("OPERATION"), op)?;
    std::fs::write(repo.state_file("ORIG_HEAD"), format!("{head}\n"))?;
    std::fs::write(repo.state_file("MERGE_MSG"), msg)?;
    if let Some(m) = merge_head {
        std::fs::write(repo.state_file("MERGE_HEAD"), format!("{m}\n"))?;
    }
    Ok(())
}

/// Merge `theirs` into the current state (HEAD + work tree). Updates work tree and
/// index; returns conflicted paths. Does not commit.
pub fn apply_three_way(
    repo: &Repo,
    idx: &mut Index,
    base: &FlatTree,
    ours: &FlatTree,
    theirs: &FlatTree,
    ours_label: &str,
    theirs_label: &str,
) -> Result<Vec<String>> {
    // Staged-but-uncommitted changes would get mixed into the result; refuse.
    if idx.as_map() != *ours {
        bail!("you have staged changes; commit or stash them before merging (jg stash)");
    }
    let result = merge_trees(&repo.odb, base, ours, theirs, ours_label, theirs_label)?;
    if let Some(p) = file_dir_clash(&result.merged) {
        bail!("cannot merge: '{p}' is a file on one side and a directory on the other. Resolve manually.");
    }
    checkout_tree(repo, idx, ours, &result.merged, false)?;
    let mut paths = Vec::new();
    for (p, c) in result.conflicts {
        if let Some(e) = idx.entries.get_mut(&p) {
            e.conflict = Some(c);
        } else if let Some(side) = c.ours.or(c.theirs) {
            idx.set(&p, side.mode, side.oid);
            idx.entries.get_mut(&p).unwrap().conflict = Some(c);
        }
        paths.push(p);
    }
    Ok(paths)
}

fn conflict_message(op: &str, paths: &[String]) -> String {
    let mut s = format!("CONFLICT: {op} stopped with conflicts in:\n");
    for p in paths {
        s.push_str(&format!("  {p}\n"));
    }
    s.push_str(&format!(
        "Edit those files (look for <<<<<<< markers), then 'jg add <file>' and 'jg {op} --continue'.\nTo give up: 'jg {op} --abort'."
    ));
    s
}

// ---------------------------------------------------------------- merge

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct MergeOpts {
    /// Branch or commit to merge into the current branch
    pub target: Option<String>,
    /// Always create a merge commit, even if a fast-forward is possible
    #[arg(long = "no-ff")]
    pub no_ff: bool,
    /// Refuse to merge unless it is a fast-forward
    #[arg(long = "ff-only")]
    pub ff_only: bool,
    /// Merge commit message
    #[arg(short = 'm', long)]
    pub message: Option<String>,
    /// Allow merging branches with no common history
    #[arg(long = "allow-unrelated-histories")]
    pub allow_unrelated: bool,
    /// Abandon an in-progress merge and restore the pre-merge state
    #[arg(long)]
    pub abort: bool,
    /// Commit a merge after resolving conflicts
    #[arg(long = "continue")]
    pub cont: bool,
}

pub fn merge(opts: MergeOpts) -> Result<Message> {
    let repo = open_repo()?;
    if opts.abort {
        return abort(&repo, "merge");
    }
    if opts.cont {
        return cont(&repo, "merge");
    }
    let target = opts.target.clone().context("what should be merged? (jg merge <branch>)")?;
    let label = target.clone();
    let _lock = repo.lock()?;
    merge_commit_into_head(&repo, &target, &label, &opts)
}

/// Shared by `merge` and `pull`.
pub fn merge_commit_into_head(repo: &Repo, target: &str, label: &str, opts: &MergeOpts) -> Result<Message> {
    let mut idx = repo.load_index()?;
    ensure_no_conflicts(&idx)?;
    if let Some(op) = in_progress(repo) {
        bail!("a {op} is already in progress; finish it ('jg {op} --continue') or abort it ('jg {op} --abort')");
    }
    let theirs = resolve_commit(repo, target)?;
    let branch = repo.current_branch()?.unwrap_or_else(|| "HEAD".into());
    let who = repo.who();
    let Some(head) = repo.head_oid()? else {
        // Unborn branch: just take theirs.
        let to = repo.commit_tree_map(&theirs)?;
        checkout_tree(repo, &mut idx, &FlatTree::new(), &to, false)?;
        repo.save_index(&mut idx)?;
        repo.refs.update_head(&theirs, &who, &format!("merge {label}: initial"))?;
        return Ok(Message::new(
            format!("Fast-forward to {}", theirs.short()),
            json!({ "result": "fast-forward", "commit": theirs }),
        ));
    };
    let base = history::merge_base(&repo.odb, &head, &theirs)?;
    if base == Some(theirs) || head == theirs {
        return Ok(Message::new("Already up to date.", json!({ "result": "up-to-date", "commit": head })));
    }
    if base == Some(head) && !opts.no_ff {
        let from = repo.commit_tree_map(&head)?;
        let to = repo.commit_tree_map(&theirs)?;
        checkout_tree(repo, &mut idx, &from, &to, false)?;
        repo.save_index(&mut idx)?;
        repo.refs.update_head(&theirs, &who, &format!("merge {label}: Fast-forward"))?;
        return Ok(Message::new(
            format!("Updating {}..{}\nFast-forward", head.short(), theirs.short()),
            json!({ "result": "fast-forward", "commit": theirs, "from": head }),
        ));
    }
    if opts.ff_only {
        bail!("not possible to fast-forward; the branches have diverged (merge without --ff-only, or rebase manually)");
    }
    if base.is_none() && !opts.allow_unrelated {
        bail!("refusing to merge unrelated histories (use --allow-unrelated-histories)");
    }
    let base_map = match base {
        Some(b) => repo.commit_tree_map(&b)?,
        None => FlatTree::new(),
    };
    let ours = repo.commit_tree_map(&head)?;
    let theirs_map = repo.commit_tree_map(&theirs)?;
    let conflicts = apply_three_way(repo, &mut idx, &base_map, &ours, &theirs_map, &branch, label)?;
    let msg = opts.message.clone().unwrap_or_else(|| format!("Merge {} into {branch}", describe(label)));
    if !conflicts.is_empty() {
        repo.save_index(&mut idx)?;
        save_state(repo, "merge", &head, Some(&theirs), &msg)?;
        return Ok(Message::new(
            conflict_message("merge", &conflicts),
            json!({ "result": "conflict", "conflicts": conflicts }),
        ));
    }
    let tree_oid = tree::build(&repo.odb, &idx.as_map())?;
    let oid = make_commit(repo, tree_oid, vec![head, theirs], &msg, &format!("merge {label}"))?;
    repo.save_index(&mut idx)?;
    Ok(Message::new(format!("Merge made: {} {msg}", oid.short()), json!({ "result": "merged", "commit": oid })))
}

fn describe(label: &str) -> String {
    if label.contains('/') && !label.starts_with("refs/") {
        format!("remote-tracking branch '{label}'")
    } else {
        format!("branch '{label}'")
    }
}

fn abort(repo: &Repo, op: &str) -> Result<Message> {
    let _lock = repo.lock()?;
    if in_progress(repo).is_none() {
        bail!("no {op} in progress");
    }
    let mut idx = repo.load_index()?;
    let head = repo.head_tree_map()?;
    let current = idx.as_map();
    checkout_tree(repo, &mut idx, &current, &head, true)?;
    idx.reset_to(&head);
    repo.save_index(&mut idx)?;
    clear_state(repo);
    Ok(Message::new(format!("{op} aborted; work tree restored to HEAD"), json!({ "aborted": op })))
}

/// Finish a conflicted operation once all conflicts are resolved and staged.
fn cont(repo: &Repo, op: &str) -> Result<Message> {
    let _lock = repo.lock()?;
    let current = in_progress(repo).with_context(|| format!("no {op} in progress"))?;
    let mut idx = repo.load_index()?;
    ensure_no_conflicts(&idx)?;
    let head = repo.head_oid()?.context("no HEAD commit")?;
    let mut parents = vec![head];
    if let Some(m) = read_merge_head(repo)? {
        parents.push(m);
    }
    let msg = std::fs::read_to_string(repo.state_file("MERGE_MSG")).unwrap_or_else(|_| current.to_string());
    let tree_oid = tree::build(&repo.odb, &idx.as_map())?;
    let oid = make_commit(repo, tree_oid, parents, &msg, &format!("{current} (continue)"))?;
    repo.save_index(&mut idx)?;
    clear_state(repo);
    Ok(Message::new(super::commit_line(repo, &oid, &msg), json!({ "commit": oid })))
}

// ---------------------------------------------------------------- cherry-pick / revert

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct PickOpts {
    /// Commit to apply
    pub commit: Option<String>,
    /// Apply the changes to the work tree/index without committing
    #[arg(short = 'n', long = "no-commit")]
    pub no_commit: bool,
    /// Abandon the in-progress operation
    #[arg(long)]
    pub abort: bool,
    /// Commit after resolving conflicts
    #[arg(long = "continue")]
    pub cont: bool,
}

pub fn cherry_pick(opts: PickOpts) -> Result<Message> {
    pick(opts, false)
}

pub fn revert(opts: PickOpts) -> Result<Message> {
    pick(opts, true)
}

fn pick(opts: PickOpts, reverse: bool) -> Result<Message> {
    let op = if reverse { "revert" } else { "cherry-pick" };
    let repo = open_repo()?;
    if opts.abort {
        return abort(&repo, op);
    }
    if opts.cont {
        return cont(&repo, op);
    }
    let _lock = repo.lock()?;
    let mut idx = repo.load_index()?;
    ensure_no_conflicts(&idx)?;
    if let Some(cur) = in_progress(&repo) {
        bail!("a {cur} is already in progress; finish or abort it first");
    }
    let spec = opts.commit.clone().context("which commit? (jg cherry-pick <commit>)")?;
    let target = resolve_commit(&repo, &spec)?;
    let c = repo.odb.read_commit(&target)?;
    if c.parents.len() > 1 {
        bail!("{} is a merge commit; picking merges is not supported", target.short());
    }
    let head = repo.head_oid()?.context("cannot cherry-pick/revert onto an empty branch")?;
    let parent_map = match c.parents.first() {
        Some(p) => repo.commit_tree_map(p)?,
        None => FlatTree::new(),
    };
    let commit_map = tree::flatten(&repo.odb, &c.tree)?;
    let ours = repo.commit_tree_map(&head)?;
    let label = format!("{} ({})", target.short(), c.summary());
    let (base, theirs, msg) = if reverse {
        (commit_map, parent_map, format!("Revert \"{}\"\n\nThis reverts commit {target}.\n", c.summary()))
    } else {
        (parent_map, commit_map, c.message.clone())
    };
    let conflicts = apply_three_way(&repo, &mut idx, &base, &ours, &theirs, "HEAD", &label)?;
    if !conflicts.is_empty() {
        repo.save_index(&mut idx)?;
        save_state(&repo, op, &head, None, &msg)?;
        return Ok(Message::new(
            conflict_message(op, &conflicts),
            json!({ "result": "conflict", "conflicts": conflicts }),
        ));
    }
    if opts.no_commit {
        repo.save_index(&mut idx)?;
        return Ok(Message::new(
            format!("Applied {} to the index (not committed)", target.short()),
            json!({ "result": "applied" }),
        ));
    }
    let tree_oid = tree::build(&repo.odb, &idx.as_map())?;
    if tree_oid == repo.odb.read_commit(&head)?.tree {
        repo.save_index(&mut idx)?;
        bail!("{op} of {} results in no changes", target.short());
    }
    let oid = if reverse {
        make_commit(&repo, tree_oid, vec![head], &msg, op)?
    } else {
        // Cherry-picks keep the original author.
        make_commit_with_author(&repo, tree_oid, vec![head], &msg, c.author.clone(), op)?
    };
    repo.save_index(&mut idx)?;
    Ok(Message::new(
        super::commit_line(&repo, &oid, &msg),
        json!({ "result": "committed", "commit": oid, "source": target }),
    ))
}
