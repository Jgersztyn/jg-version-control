//! `jg status`

use super::{Report, Style, open_repo};
use crate::core::Oid;
use crate::core::history;
use crate::core::worktree::{self, Change};
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct StatusOpts {
    /// Short one-line-per-file format
    #[arg(short = 's', long)]
    pub short: bool,
}

#[derive(Serialize)]
pub struct Tracking {
    pub upstream: String,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Serialize)]
pub struct StatusReport {
    pub branch: Option<String>,
    pub head: Option<Oid>,
    pub detached: bool,
    pub tracking: Option<Tracking>,
    /// "merge", "cherry-pick", "revert" when an operation is in progress.
    pub in_progress: Option<String>,
    pub staged: Vec<Change>,
    pub unstaged: Vec<Change>,
    pub untracked: Vec<String>,
    pub conflicts: Vec<String>,
    pub clean: bool,
    #[serde(skip)]
    short: bool,
}

pub fn run(opts: StatusOpts) -> Result<StatusReport> {
    let repo = open_repo()?;
    let mut idx = repo.load_index()?;
    let st = worktree::status(&repo, &mut idx)?;
    let branch = repo.current_branch()?;
    let head = repo.head_oid()?;
    let tracking = match (&branch, head) {
        (Some(b), Some(h)) => tracking_info(&repo, b, h)?,
        _ => None,
    };
    Ok(StatusReport {
        detached: branch.is_none(),
        branch,
        head,
        tracking,
        in_progress: super::merge::in_progress(&repo),
        clean: st.is_clean() && st.untracked.is_empty(),
        staged: st.staged,
        unstaged: st.unstaged,
        untracked: st.untracked,
        conflicts: st.conflicts,
        short: opts.short,
    })
}

/// Ahead/behind counts against the branch's upstream, if configured.
pub fn tracking_info(repo: &crate::core::Repo, branch: &str, head: Oid) -> Result<Option<Tracking>> {
    let Some(upstream) = super::sync::upstream_ref(repo, branch) else { return Ok(None) };
    let Some(up) = repo.refs.read(&upstream)? else { return Ok(None) };
    let mine = history::ancestors(&repo.odb, &[head])?;
    let theirs = history::ancestors(&repo.odb, &[up])?;
    Ok(Some(Tracking {
        upstream: upstream.trim_start_matches("refs/remotes/").to_string(),
        ahead: mine.difference(&theirs).count(),
        behind: theirs.difference(&mine).count(),
    }))
}

impl Report for StatusReport {
    fn human(&self, s: Style) -> String {
        let mut o = String::new();
        if self.short {
            for c in &self.conflicts {
                o.push_str(&format!("{} {c}\n", s.red("UU")));
            }
            let mut paths: Vec<&String> =
                self.staged.iter().map(|c| &c.path).chain(self.unstaged.iter().map(|c| &c.path)).collect();
            paths.sort();
            paths.dedup();
            for p in paths {
                let x = self.staged.iter().find(|c| &c.path == p).map(|c| c.kind.letter()).unwrap_or(' ');
                let y = self.unstaged.iter().find(|c| &c.path == p).map(|c| c.kind.letter()).unwrap_or(' ');
                o.push_str(&format!("{}{} {p}\n", s.green(&x.to_string()), s.red(&y.to_string())));
            }
            for u in &self.untracked {
                o.push_str(&format!("{} {u}\n", s.red("??")));
            }
            return o.trim_end().to_string();
        }
        match (&self.branch, self.head) {
            (Some(b), _) => o.push_str(&format!("On branch {}\n", s.bold(b))),
            (None, Some(h)) => o.push_str(&format!("{} at {}\n", s.red("HEAD detached"), h.short())),
            _ => {}
        }
        if self.head.is_none() {
            o.push_str("\nNo commits yet\n");
        }
        if let Some(t) = &self.tracking {
            match (t.ahead, t.behind) {
                (0, 0) => o.push_str(&format!("Your branch is up to date with '{}'.\n", t.upstream)),
                (a, 0) => o.push_str(&format!("Your branch is ahead of '{}' by {a} commit(s).\n", t.upstream)),
                (0, b) => o.push_str(&format!("Your branch is behind '{}' by {b} commit(s).\n", t.upstream)),
                (a, b) => o.push_str(&format!(
                    "Your branch and '{}' have diverged ({a} and {b} different commits).\n",
                    t.upstream
                )),
            }
        }
        if let Some(op) = &self.in_progress {
            o.push_str(&format!(
                "\n{}\n  (fix conflicts, 'jg add' them, then 'jg commit'; or 'jg {op} --abort')\n",
                s.yellow(&format!("You are in the middle of a {op}."))
            ));
        }
        if !self.conflicts.is_empty() {
            o.push_str("\nUnmerged paths:\n");
            for c in &self.conflicts {
                o.push_str(&format!("\t{}\n", s.red(&format!("both modified:   {c}"))));
            }
        }
        if !self.staged.is_empty() {
            o.push_str("\nChanges to be committed:\n  (use \"jg restore --staged <file>\" to unstage)\n");
            for c in &self.staged {
                o.push_str(&format!("\t{}\n", s.green(&format!("{:<14}{}", format!("{}:", c.kind.label()), c.path))));
            }
        }
        if !self.unstaged.is_empty() {
            o.push_str("\nChanges not staged for commit:\n  (use \"jg add <file>\" to stage, \"jg restore <file>\" to discard)\n");
            for c in &self.unstaged {
                o.push_str(&format!("\t{}\n", s.red(&format!("{:<14}{}", format!("{}:", c.kind.label()), c.path))));
            }
        }
        if !self.untracked.is_empty() {
            o.push_str("\nUntracked files:\n  (use \"jg add <file>\" to track)\n");
            for u in &self.untracked {
                o.push_str(&format!("\t{}\n", s.red(u)));
            }
        }
        if self.clean {
            o.push_str("\nnothing to commit, working tree clean\n");
        } else if self.staged.is_empty() && self.conflicts.is_empty() {
            o.push_str("\nno changes added to commit (use \"jg add\")\n");
        }
        o.trim_end().to_string()
    }
}
