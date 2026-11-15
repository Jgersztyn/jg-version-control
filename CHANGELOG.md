# Changelog

## Unreleased

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
