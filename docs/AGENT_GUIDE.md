# jg — guide for AI agents

You are working in a repository managed by **jg**, a local-first version control
tool whose commands mirror git's. Print this guide any time with `jg agent-guide`.

## Ground rules

1. **Never add AI attribution to commits.** Do not put `Co-Authored-By`,
   "Generated with …", model names, or similar lines in commit messages, tag
   messages, or anywhere else jg stores history. The repository owner requires this.
   The only exception is the user explicitly running `jg commit --ai`.
2. **Do not run destructive commands unless the user asked for them:**
   `reset --hard`, `checkout -f`, `restore` (discards edits), `stash clear`,
   `stash drop`, `branch -D`, `push --force`, `push --delete`, `git-export --force`.
3. **Do not push** (`jg push`) or export (`jg git-export`) unless asked.
4. Prefer small, focused commits with clear messages (imperative summary ≤ 72 chars).
5. If a command reports conflicts, stop and tell the user. Don't guess a resolution
   unless asked to resolve it.
6. If `jg` says the repository is locked, another jg process is running; wait and retry.
   Don't delete `.jg/jg.lock` unless you are sure no jg process is running.

## Machine-readable output

Add `--json` to any command. Success prints a JSON object on stdout; failure prints
`{"error": "..."}` on stdout and exits with status 1. Merge/cherry-pick/revert/stash
conflicts exit 1 with `"result": "conflict"` and a `"conflicts"` list.

| Task | Command |
|---|---|
| What changed? | `jg status --json` → `staged`, `unstaged`, `untracked`, `conflicts`, `clean`, `branch`, `tracking` |
| Show edits | `jg diff --json` (unstaged), `jg diff --staged --json`, `jg diff A B --json` |
| History | `jg log -n 20 --json`, `jg log --json -- path/to/file` |
| One commit | `jg show <rev> --json` |
| Branches | `jg branch --json` |
| Recover lost work | `jg reflog --json` |

Revisions: `HEAD`, `main`, `v1.0`, abbreviated ids (≥ 4 hex), `HEAD~2`, `HEAD^2`.

## Typical safe workflow

```bash
jg status --json                 # inspect before touching anything
jg checkout -b feature/short-name   # work on a branch (optional)
# ...edit files...
jg diff                          # review
jg add path/to/file another/file # stage specific files (avoid blanket `jg add -A`
                                 #   unless you checked `status` for stray files)
jg commit -m "Add input validation to parser"
jg log -n 3 --oneline
```

Other everyday commands: `jg restore --staged <file>` (unstage),
`jg commit --amend --no-edit` (add staged changes to the last commit — only if it
is not pushed), `jg stash` / `jg stash pop`, `jg merge <branch>`,
`jg merge --continue | --abort`.

Commit messages: always pass `-m` (no editor is available in agent sessions).
Multiple `-m` flags become separate paragraphs.

## Things that differ from git

- Metadata lives in `.jg/`, not `.git/`. Ignore rules come from `.jgignore` and `.gitignore`.
- There is no rebase or interactive mode. Use merge, cherry-pick, revert and reset.
- `jg git-export` writes this history into a real git repository (same commit ids).
- `jg checkout` is also available as `jg switch`.
