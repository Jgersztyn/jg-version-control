# Architecture

## Design decisions

| Decision | Why |
|---|---|
| **Git object format** (SHA-1, zlib loose objects, identical bytes) | Commit ids equal git's, so export to git is a file copy and the format is well understood. |
| **Loose objects only** (no packfiles) | Simple and robust. Fine for personal-scale repos. Packing is on the roadmap. |
| **JSON index** (`.jg/index`) instead of git's binary index | Purely local state, so readability wins. The git binary index is written only by `git-export`. |
| **Library + thin binary** (`src/lib.rs` + `src/main.rs`) | Every command is a function returning a `Serialize` report. The CLI, `--json` and a future MCP server share it. |
| **Options structs derive both `clap::Args` and `serde::Deserialize`** | The same struct parses from CLI flags or from JSON (for MCP or an API later). |
| **System `ssh` and `curl`, no network crates** | No TLS/SSH code to maintain. Uses the user's existing keys and config. |
| **One repo write lock** (`.jg/jg.lock`) | Humans and AI agents can't corrupt state by running jg at the same time. |

## Source layout

```
src/
  main.rs            entry: alias expansion → clap parse → dispatch → print (human or JSON)
  cli.rs             clap command tree (option structs come from ops/*)
  lib.rs             crate root: core, ops, transport, util
  util.rs            hex, time formatting, atomic writes, path helpers
  core/              repository engine — never prints, never parses argv
    oid.rs           Oid (SHA-1), git-style object hashing
    object.rs        Blob/Tree/Commit/Tag parse + serialize (git byte format)
    odb.rs           object store: .jg/objects/xx/yyyy (zlib)
    refs.rs          HEAD, refs/heads|tags|remotes, reflogs (.jg/logs)
    index.rs         staging area (JSON), conflict records
    tree.rs          nested tree <-> flat map {path: (mode, oid)}
    worktree.rs      scanning files, ignore handling, status computation
    ignore.rs        .jgignore/.gitignore matching
    revparse.rs      HEAD~2, main^, abbreviated ids, tags
    history.rs       log walk, merge-base, ancestry, "objects to send"
    diff.rs          line diffs → structured hunks → unified diff text
    merge.rs         diff3 text merge + three-way tree merge
    config.rs        git-style INI config (repo + global layers)
    repo.rs          Repo: open/discover/init, locking, identity, work-tree file I/O
  ops/               one module per command family; each returns a Report
    mod.rs           Report trait, Style, Message, checkout_tree(), make_commit()
    status, stage (add/rm/mv/restore), commit, diff, log (log/show/reflog),
    branch (branch/checkout/tag), merge (merge/cherry-pick/revert), reset,
    stash, remote, sync (clone/fetch/pull/push), gitexport, config (config/alias/
    shell-init), update (self-update), agent (agent-guide)
  transport/mod.rs   Remote trait; local-path + SSH implementations; `jg serve`
tests/               end-to-end tests that run the real binary
```

## On-disk layout (`.jg/`)

```
HEAD                 "ref: refs/heads/main" or a commit id (detached)
config               INI: core.*, remote.*, branch.<name>.remote/merge
index                JSON staging area
objects/xx/yyyy...   git loose objects
refs/heads|tags|remotes/...   one file per ref, containing an id
logs/HEAD, logs/refs/...      reflogs (git format)
stash                stash stack, newest first (ids of git-style stash commits)
OPERATION, MERGE_HEAD, MERGE_MSG, ORIG_HEAD   in-progress merge/cherry-pick/revert state
git-export.json      last exported tips per git target (incremental export)
jg.lock              present while a jg command is writing
```

## Key flows

- **status**: `HEAD tree` vs `index` = staged; `index` vs files = unstaged
  (uses the cached size/mtime, and re-hashes files modified within 2s of the index
  write, the "racy git" case). Files that aren't in the index and aren't ignored are untracked.
- **checkout_tree(from, to, force)** (`ops/mod.rs`): the one function that changes the
  work tree. Checkout, merge, reset --hard, stash, clone and pull all use it. It
  checks every affected path *before* writing anything, so it refuses instead of losing data.
- **merge**: merge-base → fast-forward if possible → otherwise `merge_trees` (per path:
  take the changed side; if both changed, diff3 text merge) → `checkout_tree` →
  conflicts recorded on index entries (`conflict: {base, ours, theirs}`) plus `.jg/MERGE_*`.
- **push/fetch**: advertise refs → compute `objects_to_send(wants, haves)` → copy
  compressed objects → compare-and-swap ref updates (fast-forward only unless forced).
- **SSH protocol** (`transport/mod.rs`): `ssh host "jg serve <path>"`. Frames are a
  4-byte length + payload. JSON requests: `ls-refs`, `fetch`, `push`, `quit`. Object
  frames are `oid(20) || zlib bytes`, ended by an empty frame.

## Adding a command

1. Create or extend a module in `src/ops/`. Define `XOpts` with
   `#[derive(clap::Args, Deserialize, Default, Debug)] #[serde(default)]` and doc
   comments (they become `--help`).
2. Write `pub fn run(opts: XOpts) -> Result<impl Report>`. Use `Message::new(text, json!(..))`
   for simple results, or a custom struct that implements `Report`. Take `repo.lock()?`
   before mutating anything.
3. Add a variant to `Cmd` in `src/cli.rs` and an arm in `dispatch()` in `src/main.rs`.
4. Add an end-to-end test in `tests/workflow.rs`. If it creates commits, also add it to
   `tests/ai_attribution.rs`.
5. Document it in `README.md` (command table) and, if agents should use it, `docs/AGENT_GUIDE.md`.

## Invariants (do not break)

- Object serialization must stay byte-identical to git's (`tests/workflow.rs::git_export_produces_identical_commits`).
- No code path may add AI or model attribution to a commit or tag unless `--ai` was used
  (`tests/ai_attribution.rs`).
- `core/` never prints to stdout. Output goes through `Report`.
- Every mutating command holds `repo.lock()`.
