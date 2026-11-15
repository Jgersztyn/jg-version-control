//! `jg init`

use super::Message;
use crate::core::Repo;
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct InitOpts {
    /// Directory to create the repository in (default: current directory)
    pub path: Option<PathBuf>,
    /// Create a bare repository (no work tree) — use this for a server/backup copy
    #[arg(long)]
    pub bare: bool,
    /// Name of the initial branch (default: init.defaultBranch config or "main")
    #[arg(short = 'b', long = "initial-branch")]
    pub initial_branch: Option<String>,
}

pub fn run(opts: InitOpts) -> Result<Message> {
    let path = match &opts.path {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };
    std::fs::create_dir_all(&path)?;
    let repo = Repo::init(&path, opts.bare, opts.initial_branch.as_deref())?;
    let branch = repo.current_branch()?.unwrap_or_default();
    // If this folder is also a git repo, keep git from listing .jg/ as untracked.
    let git_dir = path.join(".git");
    let git_excluded = !opts.bare && git_dir.is_dir() && crate::util::exclude_jg_from_git(&git_dir)?;
    let mut text = format!(
        "Initialized empty {}jg repository in {} (branch '{branch}')",
        if opts.bare { "bare " } else { "" },
        repo.dir.display()
    );
    if git_excluded {
        text.push_str("\nAdded .jg/ to .git/info/exclude so git ignores jg's data.");
        text.push_str("\nTo bring over the git history too, run: jg git-import");
    }
    Ok(Message::new(
        text,
        json!({ "path": repo.dir, "bare": opts.bare, "branch": branch, "git_excluded": git_excluded }),
    ))
}
