//! `jg git-import` — bring a git repository's history into jg (no git needed).
//!
//! Every git object (loose or packed) is copied into jg's object store. Ids stay the
//! same because the formats are identical. Then branches and tags are created:
//! - a git branch with no jg counterpart is created under the same name;
//! - if the jg branch already contains it, nothing changes;
//! - if the jg branch is behind it (and isn't checked out), it is fast-forwarded;
//! - otherwise (diverged or unrelated history) it is imported as `git/<name>`.
//!
//! The work tree is never modified. Running it again only imports what's new.

use super::{Message, open_repo};
use crate::core::Repo;
use crate::core::gitread::GitRepo;
use crate::core::history::is_ancestor;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct ImportOpts {
    /// Git repository to import (work tree, `.git` dir, or bare repo). Default: the current folder
    pub path: Option<PathBuf>,
    /// Import all git branches under this prefix (e.g. `git` -> `git/main`)
    #[arg(long)]
    pub prefix: Option<String>,
    /// Don't import tags
    #[arg(long = "no-tags")]
    pub no_tags: bool,
}

/// `core.autocrlf` as git would see it: repo config, then global, then system.
fn git_autocrlf(git: &GitRepo) -> Option<String> {
    use crate::core::config::ConfigFile;
    if let Some(v) = git.config_value("core", "autocrlf") {
        return Some(v);
    }
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).ok().map(PathBuf::from);
    let mut files: Vec<PathBuf> = Vec::new();
    if let Some(h) = &home {
        files.push(h.join(".gitconfig"));
        files.push(h.join(".config").join("git").join("config"));
    }
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramW6432"] {
            if let Ok(p) = std::env::var(var) {
                files.push(PathBuf::from(p).join("Git").join("etc").join("gitconfig"));
            }
        }
    } else {
        files.push(PathBuf::from("/etc/gitconfig"));
    }
    files.iter().find_map(|f| ConfigFile::load(f).get("core.autocrlf"))
}

pub fn run(opts: ImportOpts) -> Result<Message> {
    let cwd = std::env::current_dir()?;
    let src = opts.path.clone().unwrap_or_else(|| cwd.clone());
    let mut git = GitRepo::open(&src)?;
    let git_refs = git.refs()?;
    if git_refs.is_empty() {
        bail!("the git repository has no branches or tags to import");
    }

    // Use the jg repo we're in, or create one in the git work tree.
    let (repo, created) = match open_repo() {
        Ok(r) => (r, false),
        Err(_) => {
            let git_dir = std::fs::canonicalize(&git.dir)?;
            if git_dir.file_name().is_none_or(|n| n != ".git") {
                bail!(
                    "not inside a jg repository. For a bare git repo, create one first: jg init --bare <dir> (then run git-import there)"
                );
            }
            let work = git_dir.parent().context("bad git path")?.to_path_buf();
            let branch = git.head_branch().and_then(|b| b.strip_prefix("refs/heads/").map(String::from));
            let r = Repo::init(&work, false, branch.as_deref())?;
            crate::util::exclude_jg_from_git(&git_dir)?;
            (r, true)
        }
    };
    let _lock = repo.lock()?;
    let had_commits = repo.head_oid()?.is_some();

    // 1. Objects.
    let mut imported = 0usize;
    for oid in git.all_oids()? {
        if repo.odb.exists(&oid) {
            continue;
        }
        let (kind, data) = git.read(&oid)?;
        let written = repo.odb.write(kind, &data)?;
        if written != oid {
            bail!("git object {oid} is corrupt (content hashes to {written})");
        }
        imported += 1;
    }

    // 2. Branches.
    let who = repo.who();
    let current_ref = super::branch::head_ref(&repo)?;
    let mut lines: Vec<String> = Vec::new();
    let mut created_refs: Vec<String> = Vec::new();
    let mut side_branches: Vec<(String, String)> = Vec::new(); // (git name, jg name)
    for (name, oid) in git_refs.iter().filter(|(n, _)| n.starts_with("refs/heads/")) {
        let short = name.trim_start_matches("refs/heads/");
        let target = match &opts.prefix {
            Some(p) => format!("refs/heads/{}/{short}", p.trim_end_matches('/')),
            None => name.clone(),
        };
        let side = format!("refs/heads/git/{short}");
        let set = |r: &str, msg: &str, lines: &mut Vec<String>| -> Result<()> {
            repo.refs.write(r, oid, &who, "git-import")?;
            lines.push(format!("  {}: {msg}", r.trim_start_matches("refs/heads/")));
            Ok(())
        };
        match repo.refs.read(&target)? {
            None => {
                set(&target, &format!("created at {}", oid.short()), &mut lines)?;
                created_refs.push(target.clone());
            }
            Some(cur) if cur == *oid => lines.push(format!("  {short}: already up to date")),
            Some(cur) if is_ancestor(&repo.odb, oid, &cur)? => {
                lines.push(format!("  {short}: already contained in jg"))
            }
            Some(cur) if is_ancestor(&repo.odb, &cur, oid)? && current_ref.as_deref() != Some(target.as_str()) => {
                set(&target, &format!("fast-forwarded to {}", oid.short()), &mut lines)?;
            }
            Some(_) => {
                set(&side, &format!("imported here (jg's '{short}' has different history)"), &mut lines)?;
                side_branches.push((short.to_string(), side.trim_start_matches("refs/heads/").to_string()));
            }
        }
    }

    // 3. Tags.
    let mut tags = 0usize;
    if !opts.no_tags {
        for (name, oid) in git_refs.iter().filter(|(n, _)| n.starts_with("refs/tags/")) {
            match repo.refs.read(name)? {
                None => {
                    repo.refs.write(name, oid, &who, "git-import")?;
                    tags += 1;
                }
                Some(cur) if cur == *oid => {}
                Some(_) => {
                    lines.push(format!("  ! tag {} differs in jg; kept jg's", name.trim_start_matches("refs/tags/")))
                }
            }
        }
    }

    // 4. Line-ending setting, so files look the same to jg as they do to git.
    let mut notes: Vec<String> = Vec::new();
    if repo.repo_config_file().get("core.autocrlf").is_none()
        && let Some(v) = git_autocrlf(&git)
        && v != "false"
    {
        let mut cfg = repo.repo_config_file();
        cfg.set("core.autocrlf", &v)?;
        cfg.save()?;
        notes.push(format!("Copied git's line-ending setting: core.autocrlf = {v}"));
    }

    // 5. A brand-new jg branch gets an index matching its commit (work tree untouched).
    if !had_commits {
        let mut repo2 = Repo::open(repo.work_dir()?)?;
        repo2.reload_config();
        if let Some(head) = repo2.head_oid()? {
            let map = repo2.commit_tree_map(&head)?;
            let mut idx = repo2.load_index()?;
            if idx.entries.is_empty() {
                idx.reset_to(&map);
                repo2.save_index(&mut idx)?;
            }
        }
    }

    let mut text = format!(
        "Imported {imported} object(s) from {}{}.\nBranches:\n{}",
        git.dir.display(),
        if created { " into a new jg repository" } else { "" },
        if lines.is_empty() { "  (none)".to_string() } else { lines.join("\n") }
    );
    if tags > 0 {
        text.push_str(&format!("\nTags: {tags} imported"));
    }
    for n in &notes {
        text.push_str(&format!("\n{n}"));
    }
    for (g, j) in &side_branches {
        text.push_str(&format!(
            "\n\njg's '{g}' and git's '{g}' (now '{j}') have separate histories. To put your jg work on top of git's:\n  \
             jg checkout {g}\n  jg reset {j}      # branch now sits on git's history; files on disk stay as they are\n  \
             jg status         # shows only real differences from git's last commit\n  \
             jg add -A\n  jg commit -m \"Continue from git history\"\n\
             Your earlier jg commits stay reachable via 'jg reflog'. Or keep both histories: jg merge {j} --allow-unrelated-histories"
        ));
    }
    if !had_commits && repo.head_oid()?.is_some() {
        text.push_str("\nRun 'jg status' to check the work tree against the imported history.");
    }
    Ok(Message::new(
        text,
        json!({
            "objects_imported": imported,
            "created_repository": created,
            "branches": lines,
            "created": created_refs,
            "side_branches": side_branches.iter().map(|(g, j)| json!({ "git": g, "jg": j })).collect::<Vec<_>>(),
            "tags_imported": tags,
            "notes": notes,
        }),
    ))
}
