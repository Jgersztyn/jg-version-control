//! Command-line definition (clap). Option structs live next to each command in `jg::ops`.

use clap::{Parser, Subcommand, ValueEnum};
use jg::ops::{
    branch, commit, config, diff, gitexport, gitimport, init, log, merge, remote, reset, stage, stash, status, sync,
    update,
};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "jg",
    version,
    about = "jg — local-first, git-compatible version control",
    long_about = "jg — local-first, git-compatible version control.\n\nCommits are byte-identical to git commits, so history can be exported to git at any time (jg git-export).\nUse --json on any command for machine-readable output (for scripts and AI agents; see 'jg agent-guide').",
    after_help = "Aliases: run 'jg alias' to list shortcuts like 'jg st'. Shell shortcuts like 'gs': eval \"$(jg shell-init)\""
)]
pub struct Cli {
    /// Machine-readable JSON output (errors become {"error": "..."})
    #[arg(long, global = true)]
    pub json: bool,
    /// Colorize output
    #[arg(long, global = true, value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,
    /// Run as if jg was started in this directory
    #[arg(short = 'C', value_name = "DIR")]
    pub dir: Option<PathBuf>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    // ---- start a repository
    /// Create a new repository
    Init(init::InitOpts),
    /// Copy a repository (local path or SSH) into a new directory
    Clone(sync::CloneOpts),

    // ---- everyday work
    /// Show changed, staged and untracked files
    Status(status::StatusOpts),
    /// Stage file contents for the next commit
    Add(stage::AddOpts),
    /// Stop tracking files (and delete them unless --cached)
    Rm(stage::RmOpts),
    /// Move or rename a tracked file
    Mv(stage::MvOpts),
    /// Discard work-tree changes, or unstage with --staged
    Restore(stage::RestoreOpts),
    /// Record staged changes as a new commit
    Commit(commit::CommitOpts),
    /// Show changes (work tree, staged, or between commits)
    Diff(diff::DiffOpts),

    // ---- history
    /// Show commit history
    Log(log::LogOpts),
    /// Show a commit (or tag) and its changes
    Show(log::ShowOpts),
    /// Show where HEAD (or a branch) has pointed — use it to recover lost commits
    Reflog(log::ReflogOpts),

    // ---- branches
    /// List, create, rename or delete branches
    Branch(branch::BranchOpts),
    /// Switch branches (or restore files with `-- <paths>`)
    #[command(visible_alias = "switch")]
    Checkout(branch::CheckoutOpts),
    /// Join another branch's history into the current branch
    Merge(merge::MergeOpts),
    /// Apply the changes from an existing commit
    CherryPick(merge::PickOpts),
    /// Create a commit that undoes an earlier commit
    Revert(merge::PickOpts),
    /// Move the current branch to another commit (--soft / --mixed / --hard)
    Reset(reset::ResetOpts),
    /// List, create or delete tags
    Tag(branch::TagOpts),
    /// Shelve uncommitted changes (push/pop/apply/list/show/drop/clear)
    #[command(args_conflicts_with_subcommands = true)]
    Stash {
        #[command(subcommand)]
        cmd: Option<stash::StashCmd>,
        #[command(flatten)]
        push: stash::PushOpts,
    },

    // ---- sharing
    /// Manage remotes (add/remove/set-url/list)
    Remote {
        #[command(subcommand)]
        cmd: Option<remote::RemoteCmd>,
    },
    /// Download branches and tags from a remote
    Fetch(sync::FetchOpts),
    /// Fetch and merge the upstream branch
    Pull(sync::PullOpts),
    /// Upload branches to a remote
    Push(sync::PushOpts),
    /// Export history into a git repository (no git needed)
    GitExport(gitexport::ExportOpts),
    /// Import a git repository's history, branches and tags (no git needed)
    GitImport(gitimport::ImportOpts),

    // ---- setup & tooling
    /// Get or set configuration (user.name, user.email, ai.command, ...)
    Config(config::ConfigOpts),
    /// Manage jg aliases and shell shortcuts
    Alias {
        #[command(subcommand)]
        cmd: Option<config::AliasCmd>,
    },
    /// Print shell code that defines shortcuts like `gs` (eval "$(jg shell-init)")
    ShellInit(config::ShellInitOpts),
    /// Print shell tab-completion script
    Completions {
        /// bash, zsh, fish, powershell or elvish
        shell: clap_complete::Shell,
    },
    /// Update jg from your release location
    SelfUpdate(update::UpdateOpts),
    /// Print the usage guide for AI agents
    AgentGuide,
    /// Print version information
    Version,
    /// (internal) Serve a repository over stdin/stdout for SSH remotes
    #[command(hide = true)]
    Serve { path: PathBuf },
}
