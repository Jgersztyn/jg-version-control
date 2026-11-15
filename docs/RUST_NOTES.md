# Rust notes: a tour of this codebase for learning

A map of Rust concepts to the places they show up in jg. Read the files alongside.

| Concept | Where to look | What to notice |
|---|---|---|
| Structs, `impl`, methods | `src/core/oid.rs` | `Oid([u8; 20])` is a *newtype*. `#[derive(Copy)]` makes it cheap to pass by value. |
| Traits | `ops/mod.rs` `Report` | One trait, many implementors. `emit<R: Report>` in `main.rs` is generic over them. |
| Enums with data + `match` | `core/refs.rs` `Head`, `core/object.rs` `Kind` | `match` must cover every case, so the compiler finds forgotten branches. |
| `Option` / `Result` / `?` | everywhere | `?` returns the error to the caller early. `anyhow::Result` holds any error, and `.context("…")` adds a message. |
| Borrowing (`&` vs `&mut`) | `ops::checkout_tree(repo: &Repo, idx: &mut Index, …)` | Many readers or one writer. The index is mutated, the repo only read. |
| Ownership & `clone()` | `core/index.rs` `reset_to` | `std::mem::take` moves the old map out without copying. |
| Collections | `BTreeMap` (sorted: index, flat trees), `HashSet` (graph walks) | Sorted maps give deterministic output, which git trees need. |
| Iterators & closures | `core/worktree.rs` `diff_maps`, `ops/log.rs` | `.iter().filter().map().collect()` chains. |
| Lifetimes | `core/merge.rs` `Edit<'a>` | `Edit` borrows line slices from the input buffers, so nothing is copied. |
| Derive macros | `#[derive(clap::Args, Deserialize, Serialize)]` | The CLI parser, JSON input and JSON output are all generated from struct definitions. |
| Modules & visibility | `src/lib.rs`, `core/mod.rs` | `pub mod` exposes modules. The binary (`main.rs`) uses the library as `jg::…`. |
| RAII / `Drop` | `core/repo.rs` `LockGuard` | The lock file is removed when the guard goes out of scope, even on error. |
| Conditional compilation | `#[cfg(unix)]` / `#[cfg(windows)]` in `repo.rs`, `update.rs` | One codebase, platform-specific bits. |
| Unit tests | `#[cfg(test)] mod tests` at the bottom of core files | Run with `cargo test`. |
| Integration tests | `tests/*.rs` | Each file is a separate crate that runs the built binary. |

Useful commands:
```bash
cargo check                 # fast type-check, no binary
cargo build / cargo run -- status    # run jg from source
cargo test merge            # only tests whose name contains "merge"
cargo clippy                # linter with good suggestions
cargo doc --open            # browse docs for jg and every dependency
```
