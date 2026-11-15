# jg — local-first, git-compatible version control

`jg` is a small version control tool for your own machines and servers. It works
like git (branches, staging, commits, merge, stash, amend, push/pull), but it is
fully local and **does not depend on git**.

Every jg commit is **byte-for-byte a git commit**. Objects are hashed and stored
exactly as git does it, so `jg git-export` turns your history into a normal git
repository with the **same commit ids**, ready to `git push` to GitHub if you ever
want to.

- Single binary (Rust), no runtime dependencies. Linux first, Windows supported.
- Remotes over a local path/mounted share, or over **SSH** to your own server.
- `--json` on every command, plus `jg agent-guide`, for AI agents (Claude, Codex, Qwen, …).
- Aliases (`jg st`, `jg fixme`) and shell shortcuts (`gs`, `ga`, `gco`, …).
- Self-hosted updates: an APT repository for Ubuntu, `jg self-update` everywhere.

---

## Install

### Ubuntu / Debian: from your APT repo (recommended once you publish releases)

```bash
echo "deb [trusted=yes] https://YOUR-SERVER/jg/apt ./" | sudo tee /etc/apt/sources.list.d/jg.list
sudo apt update && sudo apt install jg
# later: sudo apt update && sudo apt upgrade
```
The repo can also be a folder: `deb [trusted=yes] file:/mnt/share/jg/apt ./`.

### Ubuntu / Debian: from a .deb file

```bash
sudo apt install ./jg_0.3.0-1_amd64.deb
```

### From source (any Linux, for development)

```bash
scripts/install.sh            # builds and installs ~/.local/bin/jg
```

### Windows

Unzip `jg-<version>-windows-x86_64.zip` and run:
```powershell
powershell -ExecutionPolicy Bypass -File install.ps1
# optionally: install.ps1 -UpdateSource https://YOUR-SERVER/jg
```
This installs `jg.exe` for your user (`%LOCALAPPDATA%\Programs\jg`) and adds it to `PATH`.
**To update**, unzip a newer release and run `install.ps1` again. It replaces the exe and
keeps your settings. If `update.source` is set, `jg self-update` also works.

How to build and publish releases: [docs/RELEASING.md](docs/RELEASING.md).

### First-time setup

```bash
jg config --global user.name  "Your Name"
jg config --global user.email "you@example.com"
```

---

## Quick start

```bash
jg init                       # creates .jg/ in the current folder (branch: main)
jg add .                      # stage everything (respects .jgignore / .gitignore)
jg commit -m "First commit"
jg status
jg log --oneline

jg checkout -b feature        # new branch
# ...edit...
jg commit -am "Do the thing"  # -a stages modified tracked files
jg checkout main
jg merge feature
```

## Command reference

| Area | Commands |
|---|---|
| Start | `init [--bare]`, `clone <url> [dir]` |
| Changes | `status [-s]`, `add <paths>/-A/-u`, `rm [--cached] [-r]`, `mv`, `restore [--staged] [-s rev] <paths>`, `diff [--staged] [A [B]] [--stat] [-- paths]` |
| Commit | `commit -m … [-a] [--amend [--no-edit] [--reset-author]] [--allow-empty] [--ai]` |
| Fix identity | `commit --reset-author --history [--old-email E] [--dry-run]` (see below) |
| History | `log [-n N] [--oneline] [--all] [--grep text] [-- paths]`, `show [rev] [--stat]`, `reflog` |
| Branches | `branch [name [start]]`, `branch -d/-D/-m`, `checkout <branch\|rev>` (alias `switch`), `checkout -b <new>`, `checkout [rev] -- <paths>` |
| Combine | `merge <branch> [--no-ff\|--ff-only] [--abort\|--continue]`, `cherry-pick <rev>`, `revert <rev>` |
| Undo | `reset [--soft\|--mixed\|--hard] [rev]`, `reset -- <paths>` (unstage), `reflog` |
| Shelve | `stash [-u] [-m msg]`, `stash list/show/pop/apply/drop/clear` |
| Tags | `tag`, `tag <name> [rev]`, `tag -a <name> -m msg`, `tag -d <name>` |
| Remotes | `remote add/remove/set-url/list`, `fetch [--prune]`, `pull [--ff-only]`, `push [-u] [--force] [--tags] [remote] [branch]` |
| Git | `git-import [path] [--prefix p]`, `git-export [path] [-b branch] [--force]` |
| Setup | `config [--global] key [value]`, `alias`, `shell-init`, `completions <shell>`, `self-update`, `agent-guide`, `version` |

Run `jg <command> --help` for every option. Global flags: `--json`, `--color auto|always|never`, `-C <dir>`.

Revisions work like git's: `HEAD`, `main`, `v1.0`, `a1b2c3d`, `HEAD~2`, `HEAD^2`, `main..feature` (diff).

---

## Aliases and shortcuts

**jg aliases** expand inside jg (`jg st` → `jg status`). Extra arguments are passed through.

| Alias | Expands to |
|---|---|
| `st` | `status` |
| `co`, `sw` | `checkout` |
| `br` | `branch` |
| `ci` | `commit` |
| `amend` | `commit --amend --no-edit` |
| `unstage` | `restore --staged` |
| `last` | `log -n 1` |
| `lg` | `log --oneline --all` |
| `undo` | `reset --soft HEAD~1` |
| `fixme` | `commit --amend --reset-author --no-edit` (your name/email on the last commit) |
| `fixme-all` | `commit --reset-author --history` (your name/email on all your commits) |

```bash
jg alias                              # list everything
jg alias set hist log --oneline -n 30 # jg hist
jg alias remove hist
```

**Shell shortcuts** are real commands in your shell (`gs` instead of `jg status`):

```bash
echo 'eval "$(jg shell-init bash)"' >> ~/.bashrc      # zsh: shell-init zsh, fish: shell-init fish
# PowerShell ($PROFILE):  jg shell-init powershell | Out-String | Invoke-Expression
jg alias set --shell gs status        # add/change one
jg alias remove --shell gp            # disable one
```
Defaults: `gs` status, `ga` add, `gaa` add -A, `gc` commit, `gcm` commit -m, `gca` amend,
`gco` checkout, `gcb` checkout -b, `gb` branch, `gl` log, `gd` diff, `gds` diff --staged,
`gst` stash, `gsp` stash pop, `gp` push, `gpl` pull, `gm` merge.

> ⚠️ These shadow other commands with the same name in that shell: `gs` is Ghostscript
> on Linux, and in PowerShell `gc`, `gl`, `gp`, `gm` are built-in aliases. Remove any
> you don't want with `jg alias remove --shell <name>`.

---

## Changing your name/email on past commits

New commits always use the current `user.name` and `user.email`. To put your current
identity on commits you already made (for example, after testing with dummy values):

```bash
jg config --global user.formerEmails "dummy@example.com"   # emails that were you (comma-separated)
jg fixme-all --dry-run    # show what would change
jg fixme-all              # rewrite all your commits and annotated tags (all branches + stashes)
jg fixme                  # or: only the last commit
```

- **Only your commits change.** A commit counts as yours if its email is your current
  `user.email`, one of `user.formerEmails`, or given with `--old-email <email>`
  (e.g. `jg fixme-all --old-email dummy@example.com`). Other people's commits are never
  touched, and `jg fixme` refuses if the last commit isn't yours.
- Timestamps, messages and file contents stay the same.
- Commit ids change, because the identity is part of the commit. The old commits stay
  reachable through `jg reflog`. If you had already pushed or exported those branches,
  use `jg push --force` / `jg git-export --force` afterwards.

---

## Remotes and your own server

A remote is another jg repository: a folder (local disk, NAS, mounted share) or a
repository on a server reached over SSH. On the server, `jg` must be installed and on `PATH`.

```bash
# On the server (once):
ssh me@myserver 'jg init --bare ~/repos/project.jg'

# On your machine:
jg remote add origin me@myserver:repos/project.jg     # or ssh://me@myserver:2222/home/me/repos/project.jg
jg push -u origin main
jg pull
jg clone me@myserver:repos/project.jg                  # on another machine
```

- Use **bare** repositories (`init --bare`) as push targets. Pushing to a branch that is
  checked out in a non-bare repo is refused.
- SSH uses your system `ssh` (keys, `~/.ssh/config` and agents all work). To use another
  client or options: `jg config core.sshCommand "ssh -i ~/.ssh/other_key"`. If `jg` isn't
  on the server's `PATH`: `jg config remote.origin.jgpath /opt/jg/bin/jg`.
- Folder remotes: `jg remote add backup /mnt/nas/project.jg`.

## Moving an existing git repository to jg

```bash
cd path/to/git/project
jg git-import          # no git needed; creates the jg repo if there isn't one
jg status              # should be clean (or show your uncommitted edits)
jg log --oneline
```

- Every commit, branch and tag comes over with the **same ids** as in git. Packed
  history (`git gc`) is supported.
- The work tree and the `.git` folder are left alone. `.jg/` is added to `.git/info/exclude`
  so git and tools like Sourcetree don't list jg's files. (`jg init` does this too.)
- Running it again imports only what's new.
- Files staged or modified in git but not committed aren't imported. They're just files
  on disk, so `jg add` them.
- Remote-tracking branches (`origin/...`) aren't imported. Check out any you need in git first.
- **Already started a separate jg history in the same folder?** Git's branch comes in as
  `git/<name>`, and the command prints how to continue on top of it:
  ```bash
  jg reset git/main      # your branch now sits on git's history; files on disk unchanged
  jg status              # only real differences from git's last commit
  jg add -A
  jg commit -m "Continue from git history"
  ```
  The earlier jg commits stay reachable via `jg reflog`. Delete the leftover branch later
  with `jg branch -D git/main`.

## Line endings (Windows and Linux)

jg follows git's rules, so a repo behaves the same with both tools:

- `.gitattributes`: `text`, `text=auto`, `-text`, `binary`, `eol=lf`, `eol=crlf`.
  Text files are stored with LF and written out with the platform's line ending
  (CRLF on Windows) unless `eol=` says otherwise.
- `core.autocrlf` (`true` / `input` / `false`) and `core.eol` (`lf` / `crlf` / `native`)
  config, for files without attributes. `jg git-import` copies git's `core.autocrlf`.
- Without `.gitattributes` or `core.autocrlf`, files are stored byte-for-byte.

## Exporting to git / GitHub

```bash
jg git-export                       # creates/updates .git next to .jg (no git needed)
git remote add origin git@github.com:you/project.git
git push -u origin --all && git push --tags
```

- Commit ids are identical in jg and git.
- Running it again exports only new objects.
- Branches git has changed independently are not overwritten unless you pass `--force`.
- `.jg/` is added to `.git/info/exclude`.
- To export to a separate (bare) git repo: `jg git-export /path/to/project.git`.

---

## AI integration

- **`--json` everywhere.** Agents parse results instead of scraping text. Errors are
  `{"error": "..."}` with exit status 1.
- **`jg agent-guide`** prints rules and recipes for agents. Tell your agent:
  *"This repo uses jg; run `jg agent-guide` first."* The same guide is in
  [docs/AGENT_GUIDE.md](docs/AGENT_GUIDE.md).
- **Optional AI commit messages:** `jg commit --ai` sends the staged diff to a command
  you choose and shows you the draft to accept, edit or cancel:
  ```bash
  jg config --global ai.command "claude -p"                 # Claude Code
  jg config --global ai.command "codex exec"                # Codex CLI
  jg config --global ai.command "ollama run qwen2.5-coder"  # local model, nothing leaves your machine
  ```
- **No attribution.** jg never adds AI or model attribution to commits. With `--ai`,
  the commit holds exactly the draft you approved. An `AI-Assisted: <tool>` trailer is
  added only if you set `ai.trailer = true` **and** use `--ai`. Tests enforce this
  (`tests/ai_attribution.rs`).

## Updating

| Installed via | Update with |
|---|---|
| APT repo | `sudo apt update && sudo apt upgrade` |
| Windows zip | run `install.ps1` from the newer zip, or `jg self-update` if `update.source` is set |
| `scripts/install.sh` | rerun it, or `jg self-update` |

`jg self-update` reads `update.source` (a URL or folder with `latest.json`, produced by
`scripts/release.sh`). It verifies the SHA-256 before swapping the binary.

## Configuration keys

| Key | Meaning |
|---|---|
| `user.name`, `user.email` | Commit identity (or env `JG_AUTHOR_NAME` / `JG_AUTHOR_EMAIL`) |
| `user.formerEmails` | Comma-separated emails that were you (used by `fixme` / `fixme-all`) |
| `init.defaultBranch` | Branch name for `jg init` (default `main`) |
| `core.editor` | Editor for commit messages (else `$VISUAL`, `$EDITOR`, nano/notepad) |
| `core.filemode` | Track the executable bit (default true on Linux) |
| `core.autocrlf`, `core.eol` | Line-ending conversion (see "Line endings") |
| `core.sshCommand` | SSH client command (env `JG_SSH` overrides) |
| `remote.<name>.url` / `.jgpath` | Remote location / path of `jg` on the server |
| `ai.command`, `ai.trailer` | `commit --ai` drafting command / opt-in trailer |
| `update.source` | Release URL or folder for `jg self-update` |
| `alias.<name>`, `shell-alias.<name>` | Aliases and shell shortcuts |

The global config is `~/.config/jg/config` (Windows: `%APPDATA%\jg\config`); the repo config is `.jg/config`.

---

## For maintainers

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): how jg works inside, storage formats, how to add a command
- [docs/RELEASING.md](docs/RELEASING.md): building, packaging (.deb, APT, Windows), publishing updates
- [docs/ROADMAP.md](docs/ROADMAP.md): status, known gaps, next steps (including the MCP server plan)
- [docs/RUST_NOTES.md](docs/RUST_NOTES.md): a short tour of the Rust used here, for learning
- [AGENTS.md](AGENTS.md): instructions for AI agents working **on** this codebase
- [CHANGELOG.md](CHANGELOG.md)

```bash
cargo build              # debug build → target/debug/jg
cargo test               # unit + end-to-end tests
cargo clippy --all-targets && cargo fmt
```
