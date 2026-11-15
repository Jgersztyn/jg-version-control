# Status and roadmap

## Done (v0.1.0)

Core object model (git-identical), index, status/add/rm/mv/restore, commit (amend,
`-a`, `--ai`), diff, log/show/reflog, branch/checkout/switch/tag, merge (ff, three-way,
conflicts, `--abort`/`--continue`), cherry-pick, revert, reset (soft/mixed/hard/paths),
stash (with `-u`), remotes over local paths and SSH (clone/fetch/pull/push, bare repos),
git export (objects, refs, index, incremental), config, aliases, shell shortcuts,
completions, self-update, `.deb` + APT repo + Windows zip packaging, `--json`
everywhere, agent guide.

v0.3.0: `jg git-import` (loose + packed history, same ids), `.gitattributes` /
`core.autocrlf` line-ending handling, `jg init` adds `.jg/` to `.git/info/exclude`.

v0.2.0: author fix-ups for your own commits (`commit --amend --reset-author`,
`commit --reset-author --history`; aliases `fixme`, `fixme-all`).

## Known gaps / next steps (rough priority)

1. **Import git remote-tracking branches / uncommitted git index** (currently only
   local branches and tags come over).
2. **Packfiles / gc**: pack loose objects to save space and speed up transfers. Prune
   unreachable objects (respect reflogs and stashes).
3. **Rename detection** in status/diff/merge (currently delete + add).
4. **`log --graph`** ASCII graph.
5. **Rebase** (non-interactive) built on cherry-pick.
6. **Pager** for long output (`less -R` when stdout is a TTY).
7. **Criss-cross merges**: use a recursive merge base (currently picks the most recent one).
8. **`.gitattributes` beyond line endings** (`diff`, `merge` drivers, `filter`/LFS).
9. **Submodules**: not supported (gitlink entries are preserved but not handled).
10. **Hooks** (`pre-commit` etc.).

## MCP server (deferred; architecture is ready)

Goal: `jg mcp` speaks the Model Context Protocol over stdio, so MCP clients (Claude
Code, Codex, Qwen-Agent, Claude desktop) call jg operations as typed tools.

Plan, about 300 lines, no new dependencies:
- New `src/ops/mcp.rs` plus a `Cmd::Mcp` variant. Read JSON-RPC 2.0 messages from stdin
  (one JSON per line) and handle `initialize`, `tools/list` and `tools/call`.
- Tool inputs deserialize directly into the existing option structs (they already derive
  `Deserialize` with `#[serde(default)]`). Results are the existing reports through
  `serde_json::to_value`. Tool schemas: hand-write them, or generate with `schemars` if
  the extra dependency is acceptable.
- Ops currently read the cwd (`open_repo()`). Add a `repo` (path) argument per tool and
  `std::env::set_current_dir`, or thread a `&Repo` through the ops.
- Safety: expose read tools (status, diff, log, show, branch list) and `add`/`commit`
  by default. Destructive tools (reset --hard, checkout -f, push, stash drop) only with
  `jg mcp --allow-destructive`. Keep the no-attribution rule.
- Register in a client, e.g.: `claude mcp add jg -- jg mcp`.
