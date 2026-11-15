//! History commands: `log`, `show`, `reflog`.

use super::diff::{Source, diff_sources, render_stat};
use super::{Report, Style, open_repo, path_matches, repo_paths};
use crate::core::diff::{FileDiff, render_unified};
use crate::core::history;
use crate::core::object::Commit;
use crate::core::refs::ReflogEntry;
use crate::core::revparse::{resolve_base, resolve_commit};
use crate::core::tree::{self, FlatTree};
use crate::core::{Oid, Repo};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct LogOpts {
    /// Revisions to start from (default: HEAD)
    pub revs: Vec<String>,
    /// Show at most N commits
    #[arg(short = 'n', long = "max-count")]
    pub max_count: Option<usize>,
    /// One line per commit
    #[arg(long)]
    pub oneline: bool,
    /// Include all branches and tags
    #[arg(long)]
    pub all: bool,
    /// Only commits whose message contains this text (case-insensitive)
    #[arg(long)]
    pub grep: Option<String>,
    /// Only commits touching these paths (put them after `--`)
    #[arg(last = true)]
    pub paths: Vec<PathBuf>,
}

#[derive(Serialize, Clone)]
pub struct Person {
    pub name: String,
    pub email: String,
    pub date: String,
    pub timestamp: i64,
}

#[derive(Serialize, Clone)]
pub struct LogEntry {
    pub commit: Oid,
    pub parents: Vec<Oid>,
    pub author: Person,
    pub committer: Person,
    pub summary: String,
    pub message: String,
    /// Branch/tag names pointing at this commit.
    pub refs: Vec<String>,
}

fn person(s: &crate::core::object::Signature) -> Person {
    Person { name: s.name.clone(), email: s.email.clone(), date: s.date_iso(), timestamp: s.time }
}

pub fn entry(oid: Oid, c: &Commit, decorations: &HashMap<Oid, Vec<String>>) -> LogEntry {
    LogEntry {
        commit: oid,
        parents: c.parents.clone(),
        author: person(&c.author),
        committer: person(&c.committer),
        summary: c.summary().to_string(),
        message: c.message.clone(),
        refs: decorations.get(&oid).cloned().unwrap_or_default(),
    }
}

/// Map commit id -> names of refs pointing at it (for `log` decorations).
pub fn decorations(repo: &Repo) -> Result<HashMap<Oid, Vec<String>>> {
    let mut m: HashMap<Oid, Vec<String>> = HashMap::new();
    let current = repo.current_branch()?;
    if current.is_none()
        && let Some(h) = repo.head_oid()?
    {
        m.entry(h).or_default().push("HEAD".into());
    }
    for (name, oid) in repo.refs.list("refs/heads/")? {
        let short = name.trim_start_matches("refs/heads/").to_string();
        let label = if current.as_deref() == Some(short.as_str()) { format!("HEAD -> {short}") } else { short };
        m.entry(oid).or_default().push(label);
    }
    for (name, oid) in repo.refs.list("refs/remotes/")? {
        m.entry(oid).or_default().push(name.trim_start_matches("refs/remotes/").to_string());
    }
    for (name, oid) in repo.refs.list("refs/tags/")? {
        let target = crate::core::revparse::peel_to_commit(repo, oid).unwrap_or(oid);
        m.entry(target).or_default().push(format!("tag: {}", name.trim_start_matches("refs/tags/")));
    }
    Ok(m)
}

#[derive(Serialize)]
pub struct LogReport {
    pub commits: Vec<LogEntry>,
    #[serde(skip)]
    oneline: bool,
}

pub fn run(opts: LogOpts) -> Result<LogReport> {
    let repo = open_repo()?;
    let filters = repo_paths(&repo, &opts.paths)?;
    let mut starts = Vec::new();
    for r in &opts.revs {
        starts.push(resolve_commit(&repo, r)?);
    }
    if opts.all {
        for prefix in ["refs/heads/", "refs/tags/", "refs/remotes/"] {
            for (_, o) in repo.refs.list(prefix)? {
                if let Ok(c) = crate::core::revparse::peel_to_commit(&repo, o) {
                    starts.push(c);
                }
            }
        }
    }
    if starts.is_empty() {
        match repo.head_oid()? {
            Some(h) => starts.push(h),
            None => return Ok(LogReport { commits: vec![], oneline: opts.oneline }),
        }
    }
    let decos = decorations(&repo)?;
    let filtering = !filters.is_empty() || opts.grep.is_some();
    let walk_limit = if filtering { None } else { opts.max_count };
    let mut commits = Vec::new();
    let mut tree_cache: HashMap<Oid, FlatTree> = HashMap::new();
    for (oid, c) in history::walk(&repo.odb, &starts, walk_limit)? {
        if opts.max_count.is_some_and(|n| commits.len() >= n) {
            break;
        }
        if let Some(g) = &opts.grep
            && !c.message.to_lowercase().contains(&g.to_lowercase())
        {
            continue;
        }
        if !filters.is_empty() && !touches(&repo, &c, &filters, &mut tree_cache)? {
            continue;
        }
        commits.push(entry(oid, &c, &decos));
    }
    Ok(LogReport { commits, oneline: opts.oneline })
}

/// Did this commit change any file under `filters` relative to its first parent?
fn touches(repo: &Repo, c: &Commit, filters: &[String], cache: &mut HashMap<Oid, FlatTree>) -> Result<bool> {
    let mut get = |t: Oid| -> Result<FlatTree> {
        if let Some(m) = cache.get(&t) {
            return Ok(m.clone());
        }
        let m = tree::flatten(&repo.odb, &t)?;
        cache.insert(t, m.clone());
        Ok(m)
    };
    let new = get(c.tree)?;
    let old = match c.parents.first() {
        Some(p) => get(repo.odb.read_commit(p)?.tree)?,
        None => FlatTree::new(),
    };
    let sel = |m: &FlatTree| -> Vec<(String, (u32, Oid))> {
        m.iter().filter(|(p, _)| path_matches(p, filters)).map(|(p, v)| (p.clone(), *v)).collect()
    };
    Ok(sel(&old) != sel(&new))
}

pub fn render_entry(e: &LogEntry, s: Style, oneline: bool) -> String {
    let deco = if e.refs.is_empty() { String::new() } else { format!(" ({})", e.refs.join(", ")) };
    if oneline {
        return format!("{}{} {}", s.yellow(&e.commit.short()), s.cyan(&deco), e.summary);
    }
    let mut o = format!("{}{}\n", s.yellow(&format!("commit {}", e.commit)), s.cyan(&deco));
    if e.parents.len() > 1 {
        let ps: Vec<String> = e.parents.iter().map(|p| p.short()).collect();
        o.push_str(&format!("Merge: {}\n", ps.join(" ")));
    }
    o.push_str(&format!("Author: {} <{}>\n", e.author.name, e.author.email));
    let sig_date = chrono::DateTime::parse_from_rfc3339(&e.author.date)
        .map(|d| d.format("%a %b %e %H:%M:%S %Y %z").to_string())
        .unwrap_or_else(|_| e.author.date.clone());
    o.push_str(&format!("Date:   {sig_date}\n\n"));
    for l in e.message.trim_end().lines() {
        o.push_str(&format!("    {l}\n"));
    }
    o
}

impl Report for LogReport {
    fn human(&self, s: Style) -> String {
        if self.commits.is_empty() {
            return "no commits yet".into();
        }
        self.commits
            .iter()
            .map(|e| render_entry(e, s, self.oneline))
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string()
    }
}

// ---------------------------------------------------------------- show

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct ShowOpts {
    /// Commit or tag to show (default: HEAD)
    pub rev: Option<String>,
    /// Show a diffstat instead of the full patch
    #[arg(long)]
    pub stat: bool,
    /// Only list changed file names
    #[arg(long = "name-only")]
    pub name_only: bool,
}

#[derive(Serialize)]
pub struct ShowReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<serde_json::Value>,
    pub commit: LogEntry,
    pub files: Vec<FileDiff>,
    #[serde(skip)]
    mode: u8,
}

pub fn show(opts: ShowOpts) -> Result<ShowReport> {
    let repo = open_repo()?;
    let rev = opts.rev.clone().unwrap_or_else(|| "HEAD".into());
    let mut tag = None;
    if !rev.contains(['~', '^']) {
        let base = resolve_base(&repo, &rev)?;
        if let Ok(t) = repo.odb.read_tag(&base) {
            tag = Some(serde_json::json!({
                "name": t.name,
                "tagger": t.tagger.as_ref().map(|s| format!("{} <{}>", s.name, s.email)),
                "date": t.tagger.as_ref().map(|s| s.date_iso()),
                "message": t.message,
            }));
        }
    }
    let oid = resolve_commit(&repo, &rev)?;
    let c = repo.odb.read_commit(&oid)?;
    let new = tree::flatten(&repo.odb, &c.tree)?;
    let old = match c.parents.first() {
        Some(p) => repo.commit_tree_map(p)?,
        None => FlatTree::new(),
    };
    let files = diff_sources(&repo, &old, &Source::Tree(&new), &[], 3)?;
    let decos = decorations(&repo)?;
    Ok(ShowReport {
        tag,
        commit: entry(oid, &c, &decos),
        files,
        mode: if opts.name_only {
            2
        } else if opts.stat {
            1
        } else {
            0
        },
    })
}

impl Report for ShowReport {
    fn human(&self, s: Style) -> String {
        let mut o = String::new();
        if let Some(t) = &self.tag {
            o.push_str(&format!("{}\n", s.yellow(&format!("tag {}", t["name"].as_str().unwrap_or("")))));
            if let Some(who) = t["tagger"].as_str() {
                o.push_str(&format!("Tagger: {who}\n"));
            }
            o.push_str(&format!("\n{}\n\n", t["message"].as_str().unwrap_or("").trim_end()));
        }
        o.push_str(&render_entry(&self.commit, s, false));
        o.push('\n');
        match self.mode {
            2 => o.push_str(&self.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>().join("\n")),
            1 => o.push_str(&render_stat(&self.files, s)),
            _ => {
                for f in &self.files {
                    o.push_str(&render_unified(f, s.color));
                }
            }
        }
        o.trim_end().to_string()
    }
}

// ---------------------------------------------------------------- reflog

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct ReflogOpts {
    /// Ref whose history to show (default: HEAD)
    pub refname: Option<String>,
    /// Show at most N entries
    #[arg(short = 'n', long = "max-count")]
    pub max_count: Option<usize>,
}

#[derive(Serialize)]
pub struct ReflogReport {
    pub refname: String,
    pub entries: Vec<ReflogEntry>,
}

pub fn reflog(opts: ReflogOpts) -> Result<ReflogReport> {
    let repo = open_repo()?;
    let name = match opts.refname.as_deref() {
        None | Some("HEAD") => "HEAD".to_string(),
        Some(n) if n.starts_with("refs/") => n.to_string(),
        Some(n) => format!("refs/heads/{n}"),
    };
    let mut entries = repo.refs.reflog(&name)?;
    if let Some(n) = opts.max_count {
        entries.truncate(n);
    }
    Ok(ReflogReport { refname: name, entries })
}

impl Report for ReflogReport {
    fn human(&self, s: Style) -> String {
        let short = self.refname.trim_start_matches("refs/heads/");
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| format!("{} {short}@{{{i}}}: {}", s.yellow(&e.new.short()), e.message))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
