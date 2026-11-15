//! `branch`, `checkout` / `switch`, and `tag`.

use super::{Message, Report, Style, checkout_tree, open_repo, path_matches, repo_paths};
use crate::core::Oid;
use crate::core::history;
use crate::core::object::{Kind, Signature, Tag};
use crate::core::refs::{Head, check_name};
use crate::core::revparse::{peel_to_commit, resolve_commit};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

// ---------------------------------------------------------------- branch

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct BranchOpts {
    /// Name of a branch to create (omit to list branches)
    pub name: Option<String>,
    /// Start point for the new branch (default: HEAD)
    pub start: Option<String>,
    /// Delete a branch (must be merged into HEAD)
    #[arg(short = 'd', long)]
    pub delete: bool,
    /// Delete a branch even if it is not merged
    #[arg(short = 'D')]
    pub force_delete: bool,
    /// Rename a branch: `jg branch -m <old> <new>` or `jg branch -m <new>`
    #[arg(short = 'm', long = "move")]
    pub rename: bool,
    /// Also list remote-tracking branches
    #[arg(short = 'a', long)]
    pub all: bool,
    /// Overwrite an existing branch when creating
    #[arg(short = 'f', long)]
    pub force: bool,
}

#[derive(Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub commit: Oid,
    pub current: bool,
    pub remote: bool,
    pub summary: String,
}

#[derive(Serialize)]
pub struct BranchList {
    pub branches: Vec<BranchInfo>,
}

impl Report for BranchList {
    fn human(&self, s: Style) -> String {
        self.branches
            .iter()
            .map(|b| {
                if b.current {
                    format!("* {} {} {}", s.green(&b.name), s.dim(&b.commit.short()), b.summary)
                } else if b.remote {
                    format!("  {} {} {}", s.red(&format!("remotes/{}", b.name)), s.dim(&b.commit.short()), b.summary)
                } else {
                    format!("  {} {} {}", b.name, s.dim(&b.commit.short()), b.summary)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub enum BranchResult {
    List(BranchList),
    Done(Message),
}

pub fn run(opts: BranchOpts) -> Result<BranchResult> {
    let repo = open_repo()?;
    let current = repo.current_branch()?;
    let Some(name) = opts.name.clone() else {
        let mut branches = Vec::new();
        for (r, oid) in repo.refs.list("refs/heads/")? {
            let n = r.trim_start_matches("refs/heads/").to_string();
            let summary = repo.odb.read_commit(&oid).map(|c| c.summary().to_string()).unwrap_or_default();
            branches.push(BranchInfo {
                current: current.as_deref() == Some(&n),
                name: n,
                commit: oid,
                remote: false,
                summary,
            });
        }
        if opts.all {
            for (r, oid) in repo.refs.list("refs/remotes/")? {
                let summary = repo.odb.read_commit(&oid).map(|c| c.summary().to_string()).unwrap_or_default();
                branches.push(BranchInfo {
                    name: r.trim_start_matches("refs/remotes/").to_string(),
                    commit: oid,
                    current: false,
                    remote: true,
                    summary,
                });
            }
        }
        return Ok(BranchResult::List(BranchList { branches }));
    };
    let _lock = repo.lock()?;
    let who = repo.who();

    if opts.rename {
        let (from, to) = match &opts.start {
            Some(new) => (name.clone(), new.clone()),
            None => (current.clone().context("not on a branch; use 'jg branch -m <old> <new>'")?, name.clone()),
        };
        check_name(&to)?;
        let (fr, tr) = (format!("refs/heads/{from}"), format!("refs/heads/{to}"));
        if !repo.refs.exists(&fr) {
            bail!("branch '{from}' not found");
        }
        if repo.refs.exists(&tr) && !opts.force {
            bail!("branch '{to}' already exists");
        }
        repo.refs.rename(&fr, &tr)?;
        if current.as_deref() == Some(&from) {
            repo.refs.set_head_branch(&tr)?;
        }
        // Carry upstream config over.
        let mut cfg = repo.repo_config_file();
        for k in ["remote", "merge"] {
            if let Some(v) = cfg.get(&format!("branch.{from}.{k}")) {
                cfg.set(&format!("branch.{to}.{k}"), &v)?;
            }
        }
        cfg.remove_section("branch", Some(&from));
        cfg.save()?;
        return Ok(BranchResult::Done(Message::new(
            format!("Renamed branch '{from}' to '{to}'"),
            json!({ "renamed": { "from": from, "to": to } }),
        )));
    }

    if opts.delete || opts.force_delete {
        let r = format!("refs/heads/{name}");
        let oid = repo.refs.read(&r)?.with_context(|| format!("branch '{name}' not found"))?;
        if current.as_deref() == Some(&name) {
            bail!("cannot delete the branch you are on ('{name}'); switch to another branch first");
        }
        if !opts.force_delete {
            let head = repo.head_oid()?;
            let upstream = super::sync::upstream_ref(&repo, &name).and_then(|u| repo.refs.read(&u).ok().flatten());
            let merged = match head {
                Some(h) => history::is_ancestor(&repo.odb, &oid, &h)?,
                None => false,
            } || upstream.is_some_and(|u| history::is_ancestor(&repo.odb, &oid, &u).unwrap_or(false));
            if !merged {
                bail!("branch '{name}' is not fully merged.\nIf you are sure, delete it with 'jg branch -D {name}'.");
            }
        }
        repo.refs.delete(&r)?;
        let mut cfg = repo.repo_config_file();
        if cfg.remove_section("branch", Some(&name)) {
            cfg.save()?;
        }
        return Ok(BranchResult::Done(Message::new(
            format!("Deleted branch {name} (was {}).", oid.short()),
            json!({ "deleted": name, "commit": oid }),
        )));
    }

    check_name(&name)?;
    let r = format!("refs/heads/{name}");
    if repo.refs.exists(&r) && !opts.force {
        bail!("a branch named '{name}' already exists");
    }
    let start = resolve_commit(&repo, opts.start.as_deref().unwrap_or("HEAD"))?;
    repo.refs.write(&r, &start, &who, &format!("branch: Created from {}", opts.start.as_deref().unwrap_or("HEAD")))?;
    Ok(BranchResult::Done(Message::new(
        format!("Created branch '{name}' at {}", start.short()),
        json!({ "created": name, "commit": start }),
    )))
}

// ---------------------------------------------------------------- checkout / switch

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct CheckoutOpts {
    /// Branch, tag or commit to switch to
    pub target: Option<String>,
    /// Create a new branch (at `target`, or HEAD) and switch to it
    #[arg(short = 'b', long = "create")]
    pub new_branch: Option<String>,
    /// Discard local changes that would be overwritten
    #[arg(short = 'f', long)]
    pub force: bool,
    /// Detach HEAD at the commit even if `target` is a branch
    #[arg(long)]
    pub detach: bool,
    /// Restore these files from `target` (or the index) instead of switching branches
    #[arg(last = true)]
    pub paths: Vec<PathBuf>,
}

pub fn checkout(opts: CheckoutOpts) -> Result<Message> {
    let repo = open_repo()?;
    if !opts.paths.is_empty() {
        return checkout_paths(&repo, &opts);
    }
    let _lock = repo.lock()?;
    let mut idx = repo.load_index()?;
    let who = repo.who();
    let from_label =
        repo.current_branch()?.or_else(|| repo.head_oid().ok().flatten().map(|o| o.short())).unwrap_or_default();

    // Resolve what we are switching to.
    let (target_commit, branch_ref) = if let Some(nb) = &opts.new_branch {
        check_name(nb)?;
        let r = format!("refs/heads/{nb}");
        if repo.refs.exists(&r) {
            bail!("a branch named '{nb}' already exists");
        }
        let start = match &opts.target {
            Some(t) => Some(resolve_commit(&repo, t)?),
            None => repo.head_oid()?,
        };
        (start, Some(r))
    } else {
        let t = opts.target.clone().context("what do you want to switch to? (jg checkout <branch>)")?;
        let r = format!("refs/heads/{t}");
        if !opts.detach && repo.refs.exists(&r) {
            (repo.refs.read(&r)?, Some(r))
        } else if !opts.detach && !repo.refs.exists(&r) && remote_branch_unique(&repo, &t)?.is_some() {
            // `jg checkout feature` when only origin/feature exists: create a tracking branch.
            let (remote, rref, oid) = remote_branch_unique(&repo, &t)?.unwrap();
            repo.refs.write(&r, &oid, &who, &format!("branch: Created from {rref}"))?;
            let mut cfg = repo.repo_config_file();
            cfg.set(&format!("branch.{t}.remote"), &remote)?;
            cfg.set(&format!("branch.{t}.merge"), &format!("refs/heads/{t}"))?;
            cfg.save()?;
            (Some(oid), Some(r))
        } else {
            (Some(resolve_commit(&repo, &t)?), None)
        }
    };

    if !idx.has_conflicts() || opts.force {
        let from = repo.head_tree_map()?;
        let to = match target_commit {
            Some(c) => repo.commit_tree_map(&c)?,
            None => from.clone(),
        };
        checkout_tree(&repo, &mut idx, &from, &to, opts.force)?;
        repo.save_index(&mut idx)?;
    } else {
        super::ensure_no_conflicts(&idx)?;
    }

    let to_label;
    match (&branch_ref, target_commit) {
        (Some(r), c) => {
            if let Some(nb) = &opts.new_branch {
                if let Some(c) = c {
                    repo.refs.write(r, &c, &who, &format!("branch: Created from {from_label}"))?;
                }
                let _ = nb;
            }
            let old = repo.head_oid()?.unwrap_or(Oid::ZERO);
            repo.refs.set_head_branch(r)?;
            to_label = r.trim_start_matches("refs/heads/").to_string();
            log_head_move(&repo, &old, c, &who, &from_label, &to_label);
        }
        (None, Some(c)) => {
            let old = repo.head_oid()?.unwrap_or(Oid::ZERO);
            repo.refs.set_head_detached(&c)?;
            to_label = c.short();
            log_head_move(&repo, &old, Some(c), &who, &from_label, &to_label);
        }
        (None, None) => bail!("nothing to check out"),
    }
    super::merge::clear_state(&repo);
    let detached = branch_ref.is_none();
    let text = if opts.new_branch.is_some() {
        format!("Switched to a new branch '{to_label}'")
    } else if detached {
        format!("HEAD is now at {to_label} (detached). Create a branch with 'jg checkout -b <name>' to keep commits.")
    } else {
        format!("Switched to branch '{to_label}'")
    };
    Ok(Message::new(
        text,
        json!({ "branch": if detached { None } else { Some(&to_label) }, "commit": target_commit, "detached": detached }),
    ))
}

fn log_head_move(repo: &crate::core::Repo, old: &Oid, new: Option<Oid>, who: &str, from: &str, to: &str) {
    if let Some(n) = new {
        let _ = std::fs::create_dir_all(repo.dir.join("logs"));
        use std::io::Write;
        if let Ok(mut f) =
            std::fs::OpenOptions::new().create(true).append(true).open(repo.dir.join("logs").join("HEAD"))
        {
            let _ = writeln!(f, "{old} {n} {who}\tcheckout: moving from {from} to {to}");
        }
    }
}

/// Find exactly one remote-tracking branch named `name` (e.g. `origin/name`).
fn remote_branch_unique(repo: &crate::core::Repo, name: &str) -> Result<Option<(String, String, Oid)>> {
    let mut hits = Vec::new();
    for (r, oid) in repo.refs.list("refs/remotes/")? {
        let rest = r.trim_start_matches("refs/remotes/");
        if let Some((remote, b)) = rest.split_once('/')
            && b == name
        {
            hits.push((remote.to_string(), r.clone(), oid));
        }
    }
    Ok(if hits.len() == 1 { hits.pop() } else { None })
}

/// `checkout [rev] -- <paths>`: overwrite work-tree files from a commit or the index.
fn checkout_paths(repo: &crate::core::Repo, opts: &CheckoutOpts) -> Result<Message> {
    let _lock = repo.lock()?;
    let mut idx = repo.load_index()?;
    let filters = repo_paths(repo, &opts.paths)?;
    let source: Vec<(String, u32, Oid)> = match &opts.target {
        Some(t) => {
            let map = repo.commit_tree_map(&resolve_commit(repo, t)?)?;
            map.into_iter().filter(|(p, _)| path_matches(p, &filters)).map(|(p, (m, o))| (p, m, o)).collect()
        }
        None => idx
            .entries
            .iter()
            .filter(|(p, _)| path_matches(p, &filters))
            .map(|(p, e)| (p.clone(), e.mode, e.oid))
            .collect(),
    };
    if source.is_empty() {
        bail!("pathspec did not match any known files");
    }
    for (p, m, o) in &source {
        repo.write_work_file(p, *m, o)?;
        if opts.target.is_some() {
            idx.set(p, *m, *o);
        }
    }
    repo.save_index(&mut idx)?;
    let paths: Vec<&String> = source.iter().map(|x| &x.0).collect();
    Ok(Message::new(format!("Updated {} path(s)", paths.len()), json!({ "restored": paths })))
}

// ---------------------------------------------------------------- tag

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct TagOpts {
    /// Tag name (omit to list tags)
    pub name: Option<String>,
    /// Commit to tag (default: HEAD)
    pub target: Option<String>,
    /// Create an annotated tag with this message
    #[arg(short = 'm', long = "message")]
    pub message: Option<String>,
    /// Create an annotated tag (requires -m)
    #[arg(short = 'a', long)]
    pub annotate: bool,
    /// Delete the tag
    #[arg(short = 'd', long)]
    pub delete: bool,
    /// Replace an existing tag
    #[arg(short = 'f', long)]
    pub force: bool,
}

#[derive(Serialize)]
pub struct TagInfo {
    pub name: String,
    pub target: Oid,
    pub annotated: bool,
}

#[derive(Serialize)]
pub struct TagList {
    pub tags: Vec<TagInfo>,
}

impl Report for TagList {
    fn human(&self, _: Style) -> String {
        self.tags.iter().map(|t| t.name.clone()).collect::<Vec<_>>().join("\n")
    }
}

pub enum TagResult {
    List(TagList),
    Done(Message),
}

pub fn tag(opts: TagOpts) -> Result<TagResult> {
    let repo = open_repo()?;
    let Some(name) = opts.name.clone() else {
        let mut tags = Vec::new();
        for (r, oid) in repo.refs.list("refs/tags/")? {
            let annotated = matches!(repo.odb.read(&oid), Ok((Kind::Tag, _)));
            tags.push(TagInfo {
                name: r.trim_start_matches("refs/tags/").into(),
                target: peel_to_commit(&repo, oid).unwrap_or(oid),
                annotated,
            });
        }
        return Ok(TagResult::List(TagList { tags }));
    };
    let _lock = repo.lock()?;
    let r = format!("refs/tags/{name}");
    if opts.delete {
        let oid = repo.refs.read(&r)?.with_context(|| format!("tag '{name}' not found"))?;
        repo.refs.delete(&r)?;
        return Ok(TagResult::Done(Message::new(
            format!("Deleted tag '{name}' (was {})", oid.short()),
            json!({ "deleted": name }),
        )));
    }
    check_name(&name)?;
    if repo.refs.exists(&r) && !opts.force {
        bail!("tag '{name}' already exists");
    }
    let target = resolve_commit(&repo, opts.target.as_deref().unwrap_or("HEAD"))?;
    let oid = if opts.annotate || opts.message.is_some() {
        let msg = opts.message.clone().context("annotated tags need a message: -m \"...\"")?;
        let tagger: Signature = repo.signature()?;
        let t = Tag {
            object: target,
            kind: Kind::Commit,
            name: name.clone(),
            tagger: Some(tagger),
            message: crate::core::object::normalize_message(&msg),
        };
        repo.odb.write(Kind::Tag, &t.serialize())?
    } else {
        target
    };
    repo.refs.write(&r, &oid, &repo.who(), "tag")?;
    Ok(TagResult::Done(Message::new(
        format!("Tagged {} as '{name}'", target.short()),
        json!({ "tag": name, "target": target, "annotated": oid != target }),
    )))
}

/// Used by status/etc: the full ref HEAD points at, if any.
pub fn head_ref(repo: &crate::core::Repo) -> Result<Option<String>> {
    Ok(match repo.head()? {
        Head::Branch(r) => Some(r),
        Head::Detached(_) => None,
    })
}
