//! `jg commit` (including `--amend` and the opt-in `--ai` message drafting).
//!
//! HARD RULE: jg never adds AI/model attribution (trailers, names, metadata) to a
//! commit. The only exception is the explicit, opt-in `ai.trailer` setting, and it
//! only applies when `--ai` was passed. See tests/ai_attribution.rs.

use super::{Message, ensure_no_conflicts, open_repo};
use crate::core::object::normalize_message;
use crate::core::{Repo, tree, worktree};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;
use std::io::{IsTerminal, Write};

#[derive(clap::Args, Deserialize, Default, Debug)]
#[serde(default)]
pub struct CommitOpts {
    /// Commit message (may be given multiple times; each becomes a paragraph)
    #[arg(short = 'm', long = "message")]
    pub message: Vec<String>,
    /// Read the commit message from a file ("-" for stdin)
    #[arg(short = 'F', long = "file")]
    pub file: Option<std::path::PathBuf>,
    /// Stage all modified/deleted tracked files before committing
    #[arg(short = 'a', long)]
    pub all: bool,
    /// Replace the last commit instead of creating a new one
    #[arg(long)]
    pub amend: bool,
    /// With --amend: keep the previous message
    #[arg(long = "no-edit")]
    pub no_edit: bool,
    /// Allow a commit with no changes
    #[arg(long = "allow-empty")]
    pub allow_empty: bool,
    /// Draft the message with your configured AI command (ai.command); you review it first
    #[arg(long)]
    pub ai: bool,
    /// With --ai: accept the draft without prompting (needed when not in a terminal)
    #[arg(short = 'y', long)]
    pub yes: bool,
}

pub fn run(opts: CommitOpts) -> Result<Message> {
    let repo = open_repo()?;
    let _lock = repo.lock()?;
    let mut idx = repo.load_index()?;
    ensure_no_conflicts(&idx)?;

    if opts.all {
        stage_tracked(&repo, &mut idx)?;
    }
    let head = repo.head_oid()?;
    let head_commit = match head {
        Some(h) => Some(repo.odb.read_commit(&h)?),
        None => None,
    };
    if opts.amend && head_commit.is_none() {
        bail!("nothing to amend: there are no commits yet");
    }

    let tree_oid = tree::build(&repo.odb, &idx.as_map())?;
    let merge_head = super::merge::read_merge_head(&repo)?;
    let compare_tree = if opts.amend {
        head_commit
            .as_ref()
            .and_then(|c| c.parents.first())
            .map(|p| repo.odb.read_commit(p))
            .transpose()?
            .map(|c| c.tree)
    } else {
        head_commit.as_ref().map(|c| c.tree)
    };
    let empty = tree::empty_tree(&repo.odb)?;
    let unchanged = compare_tree.unwrap_or(empty) == tree_oid;
    if unchanged && !opts.allow_empty && !opts.amend && merge_head.is_none() {
        bail!("nothing to commit (use 'jg add' to stage changes, or --allow-empty)");
    }

    // ---- message ----
    let mut message = if !opts.message.is_empty() {
        opts.message.join("\n\n")
    } else if let Some(f) = &opts.file {
        if f.as_os_str() == "-" {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
            s
        } else {
            std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?
        }
    } else if opts.amend && opts.no_edit {
        head_commit.as_ref().unwrap().message.clone()
    } else if opts.ai {
        draft_with_ai(&repo, compare_tree, tree_oid, &opts)?
    } else {
        let template = if opts.amend {
            head_commit.as_ref().unwrap().message.clone()
        } else {
            std::fs::read_to_string(repo.state_file("MERGE_MSG")).unwrap_or_default()
        };
        edit_message(&repo, &template)?
    };
    if opts.ai && repo.config.get_bool("ai.trailer", false) {
        // Opt-in only, and only reachable with --ai.
        let tool = repo.config.get("ai.command").unwrap_or_default();
        let tool = tool.split_whitespace().next().unwrap_or("ai");
        message = format!("{}\n\nAI-Assisted: {tool}\n", message.trim_end());
    }
    let message = normalize_message(&message);
    if message.is_empty() {
        bail!("aborting commit due to empty commit message");
    }

    // ---- parents / author ----
    let (parents, author, reflog) = if opts.amend {
        let hc = head_commit.as_ref().unwrap();
        (hc.parents.clone(), hc.author.clone(), "commit (amend)")
    } else {
        let mut ps: Vec<_> = head.into_iter().collect();
        if let Some(m) = merge_head {
            ps.push(m);
        }
        let reflog = if ps.len() > 1 {
            "commit (merge)"
        } else if ps.is_empty() {
            "commit (initial)"
        } else {
            "commit"
        };
        (ps, repo.signature()?, reflog)
    };
    let oid = super::make_commit_with_author(&repo, tree_oid, parents, &message, author, reflog)?;
    repo.save_index(&mut idx)?;
    super::merge::clear_state(&repo);

    let branch = repo.current_branch()?;
    let summary = message.lines().next().unwrap_or("").to_string();
    Ok(Message::new(
        format!(
            "[{} {}] {}{}",
            branch.clone().unwrap_or_else(|| "detached HEAD".into()),
            oid.short(),
            summary,
            if opts.amend { " (amended)" } else { "" }
        ),
        json!({ "commit": oid, "branch": branch, "summary": summary, "amend": opts.amend, "ai": opts.ai }),
    ))
}

/// `commit -a`: stage modifications and deletions of tracked files.
fn stage_tracked(repo: &Repo, idx: &mut crate::core::index::Index) -> Result<()> {
    let files = worktree::scan(repo, idx)?;
    let tracked: Vec<String> = idx.entries.keys().cloned().collect();
    for p in tracked {
        match files.get(&p) {
            None => {
                idx.entries.remove(&p);
            }
            Some(wf) => {
                let ((mode, oid), _) = worktree::hash_work_file(repo, idx, &p, wf)?;
                if idx.entries[&p].oid != oid || idx.entries[&p].mode != mode {
                    repo.odb.write_blob(&repo.read_work_file(&p)?)?;
                    idx.set(&p, mode, oid);
                }
            }
        }
    }
    Ok(())
}

/// Pick the editor: JG_EDITOR, core.editor, VISUAL, EDITOR, then a platform default.
fn editor(repo: &Repo) -> String {
    std::env::var("JG_EDITOR")
        .ok()
        .or_else(|| repo.config.get("core.editor"))
        .or_else(|| std::env::var("VISUAL").ok())
        .or_else(|| std::env::var("EDITOR").ok())
        .unwrap_or_else(|| if cfg!(windows) { "notepad".into() } else { "nano".into() })
}

/// Run a command line through the platform shell.
pub fn shell_command(cmdline: &str) -> std::process::Command {
    if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(cmdline);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(cmdline);
        c
    }
}

fn edit_message(repo: &Repo, template: &str) -> Result<String> {
    if !std::io::stdin().is_terminal() && std::env::var("JG_EDITOR").is_err() {
        bail!("no commit message given; use -m \"message\" (no terminal available for an editor)");
    }
    let path = repo.dir.join("COMMIT_EDITMSG");
    let mut text = template.to_string();
    text.push_str("\n# Write your commit message above. Lines starting with '#' are ignored.\n# An empty message aborts the commit.\n");
    std::fs::write(&path, &text)?;
    let ed = editor(repo);
    let status = shell_command(&format!("{ed} \"{}\"", path.display())).status().context("launching editor")?;
    if !status.success() {
        bail!("editor exited with an error; commit aborted");
    }
    let edited = std::fs::read_to_string(&path)?;
    Ok(edited.lines().filter(|l| !l.starts_with('#')).collect::<Vec<_>>().join("\n"))
}

/// Ask the user's configured AI command for a commit message draft.
fn draft_with_ai(
    repo: &Repo,
    base_tree: Option<crate::core::Oid>,
    tree_oid: crate::core::Oid,
    opts: &CommitOpts,
) -> Result<String> {
    let cmd = repo.config.get("ai.command").context(
        "no AI command configured. Set one, for example:\n  jg config --global ai.command \"claude -p\"\n  jg config --global ai.command \"ollama run qwen2.5-coder\"",
    )?;
    let empty = tree::empty_tree(&repo.odb)?;
    let diff_text = super::diff::tree_diff_text(repo, &base_tree.unwrap_or(empty), &tree_oid, 40_000)?;
    let prompt = format!(
        "Write a git commit message for the following staged diff.\n\
         Rules: a concise summary line (max 72 chars, imperative mood), then optionally a blank line and a short body.\n\
         Output ONLY the commit message text. No code fences, no preamble, no attribution or Co-Authored-By lines.\n\n{diff_text}"
    );
    eprintln!("Asking `{cmd}` for a commit message draft...");
    let mut child = shell_command(&cmd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("running ai.command `{cmd}`"))?;
    child.stdin.take().unwrap().write_all(prompt.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("ai.command `{cmd}` failed ({})", out.status);
    }
    let draft = String::from_utf8_lossy(&out.stdout).trim().trim_matches('`').trim().to_string();
    if draft.is_empty() {
        bail!("ai.command returned an empty message");
    }
    if opts.yes {
        return Ok(draft);
    }
    if !std::io::stdin().is_terminal() {
        bail!("AI draft (not committed; re-run with --yes to accept, or use -m):\n\n{draft}");
    }
    loop {
        eprintln!("\n----- AI draft -----\n{draft}\n--------------------");
        eprint!("[a]ccept, [e]dit, [c]ancel? ");
        std::io::stderr().flush().ok();
        let mut ans = String::new();
        std::io::stdin().read_line(&mut ans)?;
        match ans.trim().to_lowercase().as_str() {
            "a" | "accept" | "y" | "yes" => return Ok(draft),
            "e" | "edit" => return edit_message(repo, &draft),
            "c" | "cancel" | "n" | "no" | "q" => bail!("commit cancelled"),
            _ => continue,
        }
    }
}
