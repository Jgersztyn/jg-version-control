//! `jg git-export` — write jg history into a real git repository, without needing git.
//!
//! Because jg objects are byte-identical to git loose objects, export is a file copy
//! of every reachable object plus writing `refs/heads/*` / `refs/tags/*`. When the
//! target is the work tree's own `.git`, a git index (v2) is written for the checked-out
//! branch so `git status` is clean immediately. Afterwards, `git push` works as usual.

use super::{Message, open_repo};
use crate::core::history::{is_ancestor, objects_to_send};
use crate::core::tree::{self};
use crate::core::{Oid, Repo};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct ExportOpts {
    /// Target git directory (default: `.git` next to `.jg` in the work tree)
    pub path: Option<PathBuf>,
    /// Only export these branches (default: all branches)
    #[arg(short = 'b', long = "branch")]
    pub branches: Vec<String>,
    /// Don't export tags
    #[arg(long = "no-tags")]
    pub no_tags: bool,
    /// Overwrite git branches that have commits jg doesn't know about
    #[arg(short = 'f', long)]
    pub force: bool,
    /// Don't write a git index for the checked-out branch
    #[arg(long = "no-index")]
    pub no_index: bool,
}

pub fn run(opts: ExportOpts) -> Result<Message> {
    let repo = open_repo()?;
    let target = match &opts.path {
        Some(p) => {
            // Accept either a work tree (containing .git) or a git dir.
            if p.join(".git").is_dir() { p.join(".git") } else { p.clone() }
        }
        None => repo
            .work_dir()
            .context("bare repository: give a target path, e.g. jg git-export /path/to/repo.git")?
            .join(".git"),
    };
    let fresh = !target.join("HEAD").exists();
    let in_worktree = repo.work.as_ref().is_some_and(|w| target == w.join(".git"));
    if fresh {
        init_git_dir(&target, opts.path.is_some() && !in_worktree && is_bare_target(&target))?;
    }
    if in_worktree {
        // Keep git from seeing jg's metadata as untracked files.
        let ex = target.join("info").join("exclude");
        let cur = fs::read_to_string(&ex).unwrap_or_default();
        if !cur.lines().any(|l| l.trim() == ".jg/" || l.trim() == ".jg") {
            fs::create_dir_all(ex.parent().unwrap())?;
            fs::write(&ex, format!("{cur}{}.jg/\n", if cur.is_empty() || cur.ends_with('\n') { "" } else { "\n" }))?;
        }
    }

    // Which refs to export.
    let mut refs: BTreeMap<String, Oid> = BTreeMap::new();
    for (r, o) in repo.refs.list("refs/heads/")? {
        let short = r.trim_start_matches("refs/heads/");
        if opts.branches.is_empty() || opts.branches.iter().any(|b| b == short) {
            refs.insert(r, o);
        }
    }
    for b in &opts.branches {
        if !refs.contains_key(&format!("refs/heads/{b}")) {
            bail!("no branch named '{b}'");
        }
    }
    if !opts.no_tags {
        refs.extend(repo.refs.list("refs/tags/")?);
    }
    if refs.is_empty() {
        bail!("nothing to export (no commits yet)");
    }

    // Skip objects reachable from what we exported last time.
    let state_path = repo.dir.join("git-export.json");
    let mut state: BTreeMap<String, Vec<String>> =
        fs::read(&state_path).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
    let key = fs::canonicalize(&target).unwrap_or(target.clone()).to_string_lossy().into_owned();
    let haves: Vec<Oid> =
        state.get(&key).map(|v| v.iter().filter_map(|h| Oid::from_hex(h).ok()).collect()).unwrap_or_default();
    let wants: Vec<Oid> = refs.values().copied().collect();
    let objects = objects_to_send(&repo.odb, &wants, &haves)?;
    let mut copied = 0;
    for o in &objects {
        let hex = o.to_hex();
        let dst = target.join("objects").join(&hex[..2]).join(&hex[2..]);
        if !dst.exists() {
            fs::create_dir_all(dst.parent().unwrap())?;
            fs::copy(repo.odb.file_path(o), &dst)?;
            copied += 1;
        }
    }

    // Update refs, refusing to discard git-only commits unless --force.
    let mut updated = Vec::new();
    let mut skipped = Vec::new();
    for (r, new) in &refs {
        let path = target.join(r);
        if let Ok(cur) = fs::read_to_string(&path)
            && let Ok(cur) = Oid::from_hex(cur.trim())
        {
            if cur == *new {
                continue;
            }
            let safe =
                repo.odb.exists(&cur) && repo.odb.read_commit(&cur).is_ok() && is_ancestor(&repo.odb, &cur, new)?;
            if !safe && !opts.force {
                skipped.push(r.clone());
                continue;
            }
        }
        crate::util::atomic_write(&path, format!("{new}\n").as_bytes())?;
        updated.push(r.clone());
    }
    if fresh {
        let head = match repo.head()? {
            crate::core::refs::Head::Branch(b) if refs.contains_key(&b) => b,
            _ => {
                refs.keys().find(|k| k.starts_with("refs/heads/")).cloned().unwrap_or_else(|| "refs/heads/main".into())
            }
        };
        fs::write(target.join("HEAD"), format!("ref: {head}\n"))?;
    }

    // Git index for the checked-out branch, so `git status` matches.
    let mut wrote_index = false;
    if in_worktree && !opts.no_index {
        let git_head = fs::read_to_string(target.join("HEAD")).unwrap_or_default();
        if let Some(b) = git_head.trim().strip_prefix("ref: ")
            && (fresh || updated.iter().any(|u| u == b))
            && let Some(oid) = refs.get(b)
        {
            write_git_index(&repo, &target.join("index"), oid)?;
            wrote_index = true;
        }
    }

    state.insert(key, wants.iter().map(|o| o.to_hex()).collect());
    fs::write(&state_path, serde_json::to_vec_pretty(&state)?)?;

    let mut text =
        format!("Exported to {} ({} new object(s); {} ref(s) updated)", target.display(), copied, updated.len());
    for s in &skipped {
        text.push_str(&format!("\n ! skipped {s}: git has commits jg doesn't know (use --force to overwrite)"));
    }
    if in_worktree {
        text.push_str("\nYou can now use git here, e.g.: git remote add origin <url> && git push -u origin --all");
    }
    Ok(Message::new(
        text,
        json!({ "target": target, "objects_copied": copied, "updated": updated, "skipped": skipped, "index_written": wrote_index }),
    ))
}

fn is_bare_target(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "git") || p.file_name().is_some_and(|n| n != ".git")
}

fn init_git_dir(dir: &Path, bare: bool) -> Result<()> {
    for d in ["objects/info", "objects/pack", "refs/heads", "refs/tags", "info"] {
        fs::create_dir_all(dir.join(d))?;
    }
    fs::write(dir.join("HEAD"), "ref: refs/heads/main\n")?;
    fs::write(
        dir.join("config"),
        format!(
            "[core]\n\trepositoryformatversion = 0\n\tfilemode = {}\n\tbare = {bare}\n",
            if cfg!(windows) { "false" } else { "true" }
        ),
    )?;
    fs::write(dir.join("description"), "Exported from jg.\n")?;
    Ok(())
}

/// Write a git index (version 2) listing every file of `commit`'s tree.
/// Stat fields are zero, so git re-hashes files once and then caches them.
fn write_git_index(repo: &Repo, path: &Path, commit: &Oid) -> Result<()> {
    let c = repo.odb.read_commit(commit)?;
    let files = tree::flatten(&repo.odb, &c.tree)?;
    let mut entries: Vec<(&String, &(u32, Oid))> = files.iter().collect();
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut out = Vec::new();
    out.extend_from_slice(b"DIRC");
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (p, (mode, oid)) in entries {
        let start = out.len();
        out.extend_from_slice(&[0u8; 24]); // ctime, mtime, dev, ino
        out.extend_from_slice(&mode.to_be_bytes());
        out.extend_from_slice(&[0u8; 12]); // uid, gid, size
        out.extend_from_slice(&oid.0);
        let flags = p.len().min(0xFFF) as u16;
        out.extend_from_slice(&flags.to_be_bytes());
        out.extend_from_slice(p.as_bytes());
        let len = out.len() - start;
        let pad = 8 - (len % 8);
        out.extend(std::iter::repeat_n(0u8, pad));
    }
    let checksum = Oid::hash(&out);
    out.extend_from_slice(&checksum.0);
    crate::util::atomic_write(path, &out)
}
