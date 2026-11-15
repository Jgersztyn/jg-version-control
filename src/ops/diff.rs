//! `jg diff`

use super::{Report, Style, open_repo, path_matches, repo_paths};
use crate::core::diff::{FileDiff, diff_lines, render_unified};
use crate::core::revparse::resolve_commit;
use crate::core::tree::{self, FlatTree};
use crate::core::worktree::{self, ChangeKind, diff_maps};
use crate::core::{Oid, Repo};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct DiffOpts {
    /// Zero, one or two revisions (or "A..B"). None: index vs work tree
    pub revs: Vec<String>,
    /// Show staged changes (HEAD vs index)
    #[arg(long, visible_alias = "cached")]
    pub staged: bool,
    /// Show a per-file summary of changed lines
    #[arg(long)]
    pub stat: bool,
    /// Only list the changed file names
    #[arg(long = "name-only")]
    pub name_only: bool,
    /// Lines of context around each change
    #[arg(short = 'U', long = "unified", default_value_t = 3)]
    #[serde(default = "three")]
    pub context: usize,
    /// Limit to these paths (put them after `--`)
    #[arg(last = true)]
    pub paths: Vec<PathBuf>,
}

fn three() -> usize {
    3
}

/// Where the "new" side of a diff comes from.
pub enum Source<'a> {
    /// Stored objects.
    Tree(&'a FlatTree),
    /// The work tree; the map holds hashes, content is read from disk.
    Work(&'a FlatTree),
}

impl Source<'_> {
    fn map(&self) -> &FlatTree {
        match self {
            Source::Tree(m) | Source::Work(m) => m,
        }
    }
}

#[derive(Serialize)]
pub struct DiffReport {
    pub files: Vec<FileDiff>,
    pub files_changed: usize,
    pub additions: usize,
    pub deletions: usize,
    #[serde(skip)]
    mode: u8, // 0 patch, 1 stat, 2 name-only
}

pub fn run(opts: DiffOpts) -> Result<DiffReport> {
    let repo = open_repo()?;
    let mut idx = repo.load_index()?;
    let filters = repo_paths(&repo, &opts.paths)?;
    let mut revs: Vec<String> = Vec::new();
    for r in &opts.revs {
        if let Some((a, b)) = r.split_once("..") {
            revs.push(if a.is_empty() { "HEAD".into() } else { a.into() });
            revs.push(if b.is_empty() { "HEAD".into() } else { b.into() });
        } else {
            revs.push(r.clone());
        }
    }
    let tree_of = |r: &str| -> Result<FlatTree> { repo.commit_tree_map(&resolve_commit(&repo, r)?) };
    let files = match (revs.len(), opts.staged) {
        (0, false) => {
            let st = worktree::status(&repo, &mut idx)?;
            // Conflicted paths are skipped here; open the file to see the conflict markers.
            let old: FlatTree = idx.as_map().into_iter().filter(|(p, _)| !st.conflicts.contains(p)).collect();
            diff_sources(&repo, &old, &Source::Work(&st.work_map), &filters, opts.context)?
        }
        (0, true) => diff_sources(&repo, &repo.head_tree_map()?, &Source::Tree(&idx.as_map()), &filters, opts.context)?,
        (1, true) => diff_sources(&repo, &tree_of(&revs[0])?, &Source::Tree(&idx.as_map()), &filters, opts.context)?,
        (1, false) => {
            let work = worktree::work_tree_map(&repo, &idx, false)?;
            diff_sources(&repo, &tree_of(&revs[0])?, &Source::Work(&work), &filters, opts.context)?
        }
        (2, _) => diff_sources(&repo, &tree_of(&revs[0])?, &Source::Tree(&tree_of(&revs[1])?), &filters, opts.context)?,
        _ => bail!("too many revisions given to diff"),
    };
    Ok(report(
        files,
        if opts.name_only {
            2
        } else if opts.stat {
            1
        } else {
            0
        },
    ))
}

pub fn report(files: Vec<FileDiff>, mode: u8) -> DiffReport {
    DiffReport {
        files_changed: files.len(),
        additions: files.iter().map(|f| f.additions).sum(),
        deletions: files.iter().map(|f| f.deletions).sum(),
        files,
        mode,
    }
}

/// Diff an old flat tree against a new source, producing structured file diffs.
pub fn diff_sources(
    repo: &Repo,
    old: &FlatTree,
    new: &Source,
    filters: &[String],
    context: usize,
) -> Result<Vec<FileDiff>> {
    let mut out = Vec::new();
    for ch in diff_maps(old, new.map()) {
        if !path_matches(&ch.path, filters) {
            continue;
        }
        let o = old.get(&ch.path);
        let n = new.map().get(&ch.path);
        let old_data = match o {
            Some((_, oid)) => repo.odb.read_blob(oid)?,
            None => Vec::new(),
        };
        let new_data = match (n, new) {
            (Some((_, oid)), Source::Tree(_)) => repo.odb.read_blob(oid)?,
            (Some(_), Source::Work(_)) => repo.read_work_file(&ch.path)?,
            (None, _) => Vec::new(),
        };
        let binary = crate::util::is_binary(&old_data) || crate::util::is_binary(&new_data);
        let (hunks, additions, deletions) = if binary || ch.kind == ChangeKind::Mode {
            (vec![], 0, 0)
        } else {
            diff_lines(&old_data, &new_data, context)
        };
        out.push(FileDiff {
            path: ch.path.clone(),
            status: ch.kind,
            old_oid: o.map(|x| x.1),
            new_oid: n.map(|x| x.1),
            old_mode: o.map(|x| x.0),
            new_mode: n.map(|x| x.0),
            binary,
            additions,
            deletions,
            hunks,
        });
    }
    Ok(out)
}

/// Unified diff text between two trees, truncated to `max_bytes` (used for AI prompts).
pub fn tree_diff_text(repo: &Repo, a: &Oid, b: &Oid, max_bytes: usize) -> Result<String> {
    let old = tree::flatten(&repo.odb, a)?;
    let new = tree::flatten(&repo.odb, b)?;
    let files = diff_sources(repo, &old, &Source::Tree(&new), &[], 3)?;
    let mut s = String::new();
    for f in &files {
        s.push_str(&render_unified(f, false));
        if s.len() > max_bytes {
            s.truncate(max_bytes);
            s.push_str("\n[diff truncated]\n");
            break;
        }
    }
    Ok(s)
}

pub fn render_stat(files: &[FileDiff], s: Style) -> String {
    let width = files.iter().map(|f| f.path.len()).max().unwrap_or(0).min(60);
    let mut o = String::new();
    for f in files {
        let total = f.additions + f.deletions;
        let bar_total = total.min(40);
        let plus = if total == 0 { 0 } else { (f.additions * bar_total).div_ceil(total) };
        let minus = bar_total.saturating_sub(plus);
        let count = if f.binary { "Bin".to_string() } else { total.to_string() };
        o.push_str(&format!(
            " {:<width$} | {:>5} {}{}\n",
            f.path,
            count,
            s.green(&"+".repeat(plus)),
            s.red(&"-".repeat(minus))
        ));
    }
    let adds: usize = files.iter().map(|f| f.additions).sum();
    let dels: usize = files.iter().map(|f| f.deletions).sum();
    o.push_str(&format!(" {} file(s) changed, {adds} insertion(s)(+), {dels} deletion(s)(-)\n", files.len()));
    o
}

impl Report for DiffReport {
    fn human(&self, s: Style) -> String {
        match self.mode {
            2 => self.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>().join("\n"),
            1 => render_stat(&self.files, s).trim_end().to_string(),
            _ => self.files.iter().map(|f| render_unified(f, s.color)).collect::<String>().trim_end().to_string(),
        }
    }
}
