//! `jg reset [--soft|--mixed|--hard] [<commit>]` and `jg reset <paths>` (unstage).

use super::{Message, checkout_tree, open_repo, path_matches, repo_paths};
use crate::core::revparse::resolve_commit;
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct ResetOpts {
    /// Commit to reset to (default: HEAD)
    pub target: Option<String>,
    /// Only move the branch; keep index and work tree
    #[arg(long, conflicts_with_all = ["hard", "mixed"])]
    pub soft: bool,
    /// Move the branch and reset the index; keep work-tree changes (default)
    #[arg(long, conflicts_with = "hard")]
    pub mixed: bool,
    /// Move the branch and DISCARD all uncommitted changes to tracked files
    #[arg(long)]
    pub hard: bool,
    /// Only unstage these paths (put them after `--`)
    #[arg(last = true)]
    pub paths: Vec<PathBuf>,
}

pub fn run(opts: ResetOpts) -> Result<Message> {
    let repo = open_repo()?;
    let _lock = repo.lock()?;
    let mut idx = repo.load_index()?;
    let spec = opts.target.clone().unwrap_or_else(|| "HEAD".into());

    if !opts.paths.is_empty() {
        if opts.soft || opts.hard {
            bail!("--soft/--hard cannot be used with paths; use 'jg restore' to discard file changes");
        }
        let filters = repo_paths(&repo, &opts.paths)?;
        let map = match repo.head_oid()? {
            Some(_) => repo.commit_tree_map(&resolve_commit(&repo, &spec)?)?,
            None => Default::default(),
        };
        let mut reset = Vec::new();
        let mut keys: Vec<String> = idx.entries.keys().cloned().chain(map.keys().cloned()).collect();
        keys.sort();
        keys.dedup();
        for p in keys.into_iter().filter(|p| path_matches(p, &filters)) {
            match map.get(&p) {
                Some((m, o)) => idx.set(&p, *m, *o),
                None => {
                    idx.entries.remove(&p);
                }
            }
            reset.push(p);
        }
        repo.save_index(&mut idx)?;
        return Ok(Message::new(format!("Unstaged {} path(s)", reset.len()), json!({ "unstaged": reset })));
    }

    let target = resolve_commit(&repo, &spec)?;
    let old = repo.head_oid()?;
    let target_map = repo.commit_tree_map(&target)?;
    let mode = if opts.hard {
        "hard"
    } else if opts.soft {
        "soft"
    } else {
        "mixed"
    };
    match mode {
        "hard" => {
            let current = idx.as_map();
            checkout_tree(&repo, &mut idx, &current, &target_map, true)?;
            idx.reset_to(&target_map);
            super::merge::clear_state(&repo);
        }
        "mixed" => {
            idx.reset_to(&target_map);
            super::merge::clear_state(&repo);
        }
        _ => {}
    }
    if let Some(o) = old {
        std::fs::write(repo.state_file("ORIG_HEAD"), format!("{o}\n"))?;
    }
    repo.refs.update_head(&target, &repo.who(), &format!("reset: moving to {spec}"))?;
    repo.save_index(&mut idx)?;
    let summary = repo.odb.read_commit(&target)?.summary().to_string();
    Ok(Message::new(
        format!("HEAD is now at {} {summary}", target.short()),
        json!({ "mode": mode, "commit": target, "previous": old }),
    ))
}
