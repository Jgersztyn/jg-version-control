//! Network-ish commands: `clone`, `fetch`, `pull`, `push`.

use super::{Message, checkout_tree, open_repo};
use crate::core::history::objects_to_send;
use crate::core::tree::FlatTree;
use crate::core::{Oid, Repo};
use crate::transport::{self, Advert, RefUpdate};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

/// The remote-tracking ref a branch merges from, e.g. `refs/remotes/origin/main`.
pub fn upstream_ref(repo: &Repo, branch: &str) -> Option<String> {
    let remote = repo.config.get(&format!("branch.{branch}.remote"))?;
    let merge = repo.config.get(&format!("branch.{branch}.merge"))?;
    let b = merge.strip_prefix("refs/heads/").unwrap_or(&merge);
    Some(format!("refs/remotes/{remote}/{b}"))
}

fn remote_url(repo: &Repo, name: &str) -> Result<String> {
    repo.config
        .get(&format!("remote.{name}.url"))
        .with_context(|| format!("no remote named '{name}' (add one with 'jg remote add {name} <url>')"))
}

/// Default remote: the current branch's upstream remote, else "origin".
fn default_remote(repo: &Repo) -> String {
    repo.current_branch()
        .ok()
        .flatten()
        .and_then(|b| repo.config.get(&format!("branch.{b}.remote")))
        .unwrap_or_else(|| "origin".into())
}

fn local_tips(repo: &Repo) -> Result<Vec<Oid>> {
    let mut v = Vec::new();
    for p in ["refs/heads/", "refs/remotes/", "refs/tags/"] {
        v.extend(repo.refs.list(p)?.into_iter().map(|(_, o)| o));
    }
    v.sort();
    v.dedup();
    Ok(v)
}

/// Fetch everything from a remote and update `refs/remotes/<name>/*` and tags.
fn do_fetch(repo: &Repo, name: &str, prune: bool) -> Result<(Advert, Vec<String>)> {
    let url = remote_url(repo, name)?;
    let mut remote = transport::connect(repo, Some(name), &url)?;
    let advert = remote.advertise()?;
    let wants: Vec<Oid> =
        advert.refs.values().filter_map(|s| Oid::from_hex(s).ok()).filter(|o| !repo.odb.exists(o)).collect();
    if !wants.is_empty() {
        remote.fetch(&wants, &local_tips(repo)?, &repo.odb)?;
    }
    let who = repo.who();
    let mut updated = Vec::new();
    for (r, hex) in &advert.refs {
        let oid = Oid::from_hex(hex)?;
        if let Some(b) = r.strip_prefix("refs/heads/") {
            let local = format!("refs/remotes/{name}/{b}");
            if repo.refs.read(&local)? != Some(oid) {
                repo.refs.write(&local, &oid, &who, &format!("fetch {name}"))?;
                updated.push(format!("{name}/{b}"));
            }
        } else if r.starts_with("refs/tags/") && !repo.refs.exists(r) {
            repo.refs.write(r, &oid, &who, &format!("fetch {name}"))?;
            updated.push(r.trim_start_matches("refs/").to_string());
        }
    }
    if prune {
        for (r, _) in repo.refs.list(&format!("refs/remotes/{name}/"))? {
            let b = r.trim_start_matches(&format!("refs/remotes/{name}/")).to_string();
            if !advert.refs.contains_key(&format!("refs/heads/{b}")) {
                repo.refs.delete(&r)?;
                updated.push(format!("{name}/{b} (pruned)"));
            }
        }
    }
    Ok((advert, updated))
}

// ---------------------------------------------------------------- fetch

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct FetchOpts {
    /// Remote to fetch from (default: upstream remote or "origin")
    pub remote: Option<String>,
    /// Delete remote-tracking branches that no longer exist on the remote
    #[arg(short = 'p', long)]
    pub prune: bool,
}

pub fn fetch(opts: FetchOpts) -> Result<Message> {
    let repo = open_repo()?;
    let name = opts.remote.clone().unwrap_or_else(|| default_remote(&repo));
    let _lock = repo.lock()?;
    let (_, updated) = do_fetch(&repo, &name, opts.prune)?;
    let text = if updated.is_empty() {
        format!("Already up to date with '{name}'.")
    } else {
        updated.iter().map(|u| format!(" * {u}")).collect::<Vec<_>>().join("\n")
    };
    Ok(Message::new(text, json!({ "remote": name, "updated": updated })))
}

// ---------------------------------------------------------------- pull

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct PullOpts {
    /// Remote (default: upstream remote or "origin")
    pub remote: Option<String>,
    /// Remote branch to merge (default: the current branch's upstream / same name)
    pub branch: Option<String>,
    /// Only update if it is a fast-forward
    #[arg(long = "ff-only")]
    pub ff_only: bool,
    /// Always create a merge commit
    #[arg(long = "no-ff")]
    pub no_ff: bool,
}

pub fn pull(opts: PullOpts) -> Result<Message> {
    let repo = open_repo()?;
    let name = opts.remote.clone().unwrap_or_else(|| default_remote(&repo));
    let current = repo.current_branch()?.context("cannot pull with a detached HEAD; check out a branch first")?;
    let _lock = repo.lock()?;
    do_fetch(&repo, &name, false)?;
    let tracking = match &opts.branch {
        Some(b) => format!("refs/remotes/{name}/{b}"),
        None => upstream_ref(&repo, &current)
            .filter(|u| u.starts_with(&format!("refs/remotes/{name}/")))
            .unwrap_or_else(|| format!("refs/remotes/{name}/{current}")),
    };
    if repo.refs.read(&tracking)?.is_none() {
        bail!("remote '{name}' has no branch '{}'", tracking.rsplit('/').next().unwrap_or(""));
    }
    let label = tracking.trim_start_matches("refs/remotes/").to_string();
    let mopts = super::merge::MergeOpts { ff_only: opts.ff_only, no_ff: opts.no_ff, ..Default::default() };
    super::merge::merge_commit_into_head(&repo, &tracking, &label, &mopts)
}

// ---------------------------------------------------------------- push

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct PushOpts {
    /// Remote (default: upstream remote or "origin")
    pub remote: Option<String>,
    /// Branches to push, as `name` or `local:remote` (default: current branch)
    pub refspecs: Vec<String>,
    /// Remember this remote as the branch's upstream
    #[arg(short = 'u', long = "set-upstream")]
    pub set_upstream: bool,
    /// Overwrite the remote branch even if it is not a fast-forward
    #[arg(short = 'f', long)]
    pub force: bool,
    /// Also push all tags
    #[arg(long)]
    pub tags: bool,
    /// Delete the named branches on the remote
    #[arg(short = 'd', long)]
    pub delete: bool,
}

pub fn push(opts: PushOpts) -> Result<Message> {
    let repo = open_repo()?;
    let name = opts.remote.clone().unwrap_or_else(|| default_remote(&repo));
    let url = remote_url(&repo, &name)?;
    let mut remote = transport::connect(&repo, Some(&name), &url)?;
    let advert = remote.advertise()?;

    let mut specs = opts.refspecs.clone();
    if specs.is_empty() && !opts.tags {
        specs.push(repo.current_branch()?.context("detached HEAD: say which branch to push")?);
    }
    let mut updates = Vec::new();
    let mut wants = Vec::new();
    let mut branches = Vec::new();
    for spec in &specs {
        let (src, dst) = spec.split_once(':').unwrap_or((spec, spec));
        let dst_ref = if dst.starts_with("refs/") { dst.to_string() } else { format!("refs/heads/{dst}") };
        let old = advert.get(&dst_ref).unwrap_or(Oid::ZERO);
        if opts.delete {
            updates.push(RefUpdate { name: dst_ref, old: old.to_hex(), new: Oid::ZERO.to_hex(), force: true });
            continue;
        }
        let new = crate::core::revparse::resolve_commit(&repo, src)?;
        if !old.is_zero() && !opts.force {
            if !repo.odb.exists(&old) {
                bail!(
                    "'{name}/{dst}' has commits you don't have yet; run 'jg pull' first (or push --force to overwrite)"
                );
            }
            if !crate::core::history::is_ancestor(&repo.odb, &old, &new)? {
                bail!(
                    "rejected: '{dst}' on '{name}' is not an ancestor of your branch (non-fast-forward).\nRun 'jg pull' to merge first, or use --force to overwrite."
                );
            }
        }
        if old == new {
            continue;
        }
        wants.push(new);
        branches.push((src.to_string(), dst.to_string(), new));
        updates.push(RefUpdate { name: dst_ref, old: old.to_hex(), new: new.to_hex(), force: opts.force });
    }
    if opts.tags {
        for (r, oid) in repo.refs.list("refs/tags/")? {
            if advert.get(&r).is_none() {
                wants.push(oid);
                updates.push(RefUpdate { name: r, old: Oid::ZERO.to_hex(), new: oid.to_hex(), force: false });
            }
        }
    }
    if updates.is_empty() {
        return Ok(Message::new("Everything up-to-date", json!({ "remote": name, "updated": [] })));
    }
    let haves: Vec<Oid> = advert.refs.values().filter_map(|s| Oid::from_hex(s).ok()).collect();
    let objects = objects_to_send(&repo.odb, &wants, &haves)?;
    let results = remote.push(&objects, &repo.odb, &updates)?;
    drop(remote);

    let _lock = repo.lock()?;
    let who = repo.who();
    let mut lines = vec![format!("To {url}")];
    let mut failed = false;
    for r in &results {
        let short = r.name.trim_start_matches("refs/heads/");
        if r.ok {
            lines.push(format!("   {short}: {}", r.message));
            if let Some(b) = r.name.strip_prefix("refs/heads/") {
                let track = format!("refs/remotes/{name}/{b}");
                if opts.delete {
                    repo.refs.delete(&track)?;
                } else if let Some(u) = updates.iter().find(|u| u.name == r.name) {
                    repo.refs.write(&track, &Oid::from_hex(&u.new)?, &who, "push")?;
                }
            }
        } else {
            failed = true;
            lines.push(format!(" ! {short}: rejected ({})", r.message));
        }
    }
    if opts.set_upstream && !failed {
        let mut cfg = repo.repo_config_file();
        for (src, dst, _) in &branches {
            cfg.set(&format!("branch.{src}.remote"), &name)?;
            cfg.set(&format!("branch.{src}.merge"), &format!("refs/heads/{dst}"))?;
            lines.push(format!("branch '{src}' set up to track '{name}/{dst}'"));
        }
        cfg.save()?;
    }
    let report = json!({ "remote": name, "objects": objects.len(), "results": results });
    if failed {
        bail!("{}\nsome refs failed to push", lines.join("\n"));
    }
    Ok(Message::new(lines.join("\n"), report))
}

// ---------------------------------------------------------------- clone

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct CloneOpts {
    /// Source: a local path, `user@host:path`, or `ssh://user@host[:port]/path`
    pub url: String,
    /// Directory to clone into (default: derived from the URL)
    pub dir: Option<PathBuf>,
    /// Make a bare clone (e.g. to create a server/backup copy)
    #[arg(long)]
    pub bare: bool,
    /// Branch to check out (default: the remote's HEAD branch)
    #[arg(short = 'b', long)]
    pub branch: Option<String>,
}

pub fn clone(opts: CloneOpts) -> Result<Message> {
    let dir = match &opts.dir {
        Some(d) => d.clone(),
        None => {
            let base = opts.url.trim_end_matches(['/', '\\']).rsplit(['/', '\\', ':']).next().unwrap_or("repo");
            let base = base.trim_end_matches(".jg").trim_end_matches(".git");
            PathBuf::from(if base.is_empty() { "repo" } else { base })
        }
    };
    if dir.exists() && std::fs::read_dir(&dir)?.next().is_some() {
        bail!("destination '{}' already exists and is not empty", dir.display());
    }
    std::fs::create_dir_all(&dir)?;
    // A relative local source path must be resolved before we change context.
    let url = match transport::parse_url(&opts.url) {
        transport::Url::Local(p) => std::fs::canonicalize(&p)
            .with_context(|| format!("'{}' does not exist", opts.url))?
            .to_string_lossy()
            .into_owned(),
        _ => opts.url.clone(),
    };
    let mut repo = Repo::init(&dir, opts.bare, Some("main"))?;
    let mut cfg = repo.repo_config_file();
    cfg.set("remote.origin.url", &url)?;
    cfg.save()?;
    repo.reload_config();

    let mut remote = transport::connect(&repo, Some("origin"), &url)?;
    let advert = remote.advertise()?;
    let wants: Vec<Oid> = advert.refs.values().filter_map(|s| Oid::from_hex(s).ok()).collect();
    let count = if wants.is_empty() { 0 } else { remote.fetch(&wants, &[], &repo.odb)? };
    drop(remote);
    let who = repo.who();
    for (r, hex) in &advert.refs {
        let oid = Oid::from_hex(hex)?;
        if opts.bare || r.starts_with("refs/tags/") {
            repo.refs.write(r, &oid, &who, "clone")?;
        } else if let Some(b) = r.strip_prefix("refs/heads/") {
            repo.refs.write(&format!("refs/remotes/origin/{b}"), &oid, &who, "clone")?;
        }
    }
    let branch = opts
        .branch
        .clone()
        .or_else(|| advert.head.as_ref().map(|h| h.trim_start_matches("refs/heads/").to_string()))
        .unwrap_or_else(|| "main".into());
    repo.refs.set_head_branch(&format!("refs/heads/{branch}"))?;
    if !opts.bare {
        if let Some(oid) = advert.get(&format!("refs/heads/{branch}")) {
            repo.refs.write(&format!("refs/heads/{branch}"), &oid, &who, &format!("clone: from {url}"))?;
            let mut cfg = repo.repo_config_file();
            cfg.set(&format!("branch.{branch}.remote"), "origin")?;
            cfg.set(&format!("branch.{branch}.merge"), &format!("refs/heads/{branch}"))?;
            cfg.save()?;
            let mut idx = repo.load_index()?;
            let to = repo.commit_tree_map(&oid)?;
            checkout_tree(&repo, &mut idx, &FlatTree::new(), &to, true)?;
            repo.save_index(&mut idx)?;
        } else if opts.branch.is_some() {
            bail!("remote has no branch '{branch}'");
        }
    }
    Ok(Message::new(
        format!("Cloned into '{}' ({count} objects, branch '{branch}')", dir.display()),
        json!({ "path": dir, "objects": count, "branch": branch, "bare": opts.bare }),
    ))
}
