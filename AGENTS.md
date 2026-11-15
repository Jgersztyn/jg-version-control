# Instructions for AI agents working on this codebase

jg is a local-first, git-compatible version control CLI written in Rust.
Start with `README.md` (what it does) and `docs/ARCHITECTURE.md` (how it works).
Then check `docs/ROADMAP.md` for current status and next steps.

## Commands

```bash
source ~/.cargo/env        # if cargo isn't on PATH
cargo build                # target/debug/jg
cargo test                 # unit + end-to-end (tests/), must pass before you finish
cargo clippy --all-targets # must be warning-free
cargo fmt                  # rustfmt.toml: max_width 120
cargo check --target x86_64-pc-windows-gnu   # keep Windows compiling
```

## Hard rules

1. **No AI attribution in commits, ever, unless the user passes `jg commit --ai`.** This
   is the owner's explicit, non-negotiable requirement. Never add code that injects
   `Co-Authored-By`, "Generated with", model names or similar into commits, tags or
   messages. `tests/ai_attribution.rs` must keep passing. When *you* make commits in a
   jg- or git-managed repo for this user, don't add attribution lines either.
2. **Git byte compatibility.** Object serialization in `src/core/object.rs` must match
   git exactly. `tests/workflow.rs::git_export_produces_identical_commits` checks this.
3. `core/` never prints. Commands return `Report`s. Mutating commands hold `repo.lock()`.
4. New option structs derive `clap::Args` + `Deserialize` + `Default` with `#[serde(default)]`
   (keeps the planned MCP server cheap; see `docs/ROADMAP.md`).
5. Prefer the standard library. Discuss new dependencies with the owner first.
6. Linux is the primary target, but don't break Windows (`#[cfg]` platform code).

## When you finish a change

- Add or adjust tests, run the full suite and clippy.
- Update `README.md` / `docs/*` / `docs/AGENT_GUIDE.md` if behavior or commands changed.
- Add a line to `CHANGELOG.md` under "Unreleased".
- Keep `docs/ROADMAP.md` current (move items to Done).
