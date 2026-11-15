# Changelog

## Unreleased

## 0.3.0 — 2026-11-15

- `jg git-import [path]`: import a git repository's commits, branches and annotated tags
  with identical ids. Reads loose objects and packfiles (OFS/REF deltas) directly, so no
  git is needed. Incremental on re-run. When a jg branch has different history, git's
  branch is imported as `git/<name>`, with instructions to continue on top of it.
- Line endings: `.gitattributes` (`text`, `text=auto`, `-text`, `binary`, `eol=`) and
  `core.autocrlf` / `core.eol` are honored, matching git's behavior.
- `jg init` inside a git work tree adds `.jg/` to `.git/info/exclude`.

## 0.2.0 — 2026-11-15

- `jg commit --amend --reset-author` makes you the author of the last commit (alias `jg fixme`).
- `jg commit --reset-author --history [--dry-run]` rewrites the name/email on all of your
  commits and annotated tags across branches, tags and stashes (alias `jg fixme-all`).
  Only commits whose email is yours (`user.email`, `user.formerEmails`, or `--old-email`)
  change. Other people's commits are never touched.
- Windows `install.ps1` reports the previous and new version when it updates an existing install.

## 0.1.0 — 2026-11-15

First version.
- Git-compatible object store, index, and refs (commit ids identical to git).
- Commands: init, clone, status, add, rm, mv, restore, commit (`--amend`, `-a`, `--ai`),
  diff, log, show, reflog, branch, checkout/switch, merge, cherry-pick, revert, reset,
  tag, stash, remote, fetch, pull, push, git-export, config, alias, shell-init,
  completions, self-update, agent-guide, version.
- Remotes over local paths and SSH (`jg serve`). Bare repositories.
- `--json` output on every command. Agent guide built into the binary.
- Packaging: `.deb` via cargo-deb, flat APT repo, Windows zip + installer, `latest.json` for self-update.
