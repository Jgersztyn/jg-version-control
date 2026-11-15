//! HARD RULE (from the project owner): jg must NEVER add AI/model context to a
//! commit unless the user passes `--ai`. These tests guard that rule.

mod common;
use common::*;

/// Words that must never appear in a commit jg wrote on its own.
const FORBIDDEN: &[&str] =
    &["co-authored-by", "ai-assisted", "generated with", "claude", "codex", "qwen", "openai", "anthropic", "gpt"];

fn assert_clean(text: &str) {
    let lower = text.to_lowercase();
    for f in FORBIDDEN {
        assert!(!lower.contains(f), "found forbidden attribution text '{f}' in:\n{text}");
    }
}

#[test]
fn no_attribution_without_ai_flag_across_all_commit_producing_commands() {
    let env = Env::new();
    let d = env.repo("r");
    // Configure an AI command and even the opt-in trailer: without --ai, neither may be used.
    env.run(&d, &["config", "ai.command", "echo 'Claude says hi'"]);
    env.run(&d, &["config", "ai.trailer", "true"]);

    write(&d, "a.txt", "one\n");
    env.run(&d, &["add", "."]);
    env.run(&d, &["commit", "-m", "Add a"]);
    write(&d, "a.txt", "two\n");
    env.run(&d, &["commit", "-am", "Edit a"]);
    env.run(&d, &["commit", "--amend", "--no-edit"]);
    env.run(&d, &["commit", "--amend", "-m", "Edit a (amended)"]);
    env.run(&d, &["checkout", "-b", "side"]);
    write(&d, "b.txt", "b\n");
    env.run(&d, &["add", "."]);
    env.run(&d, &["commit", "-m", "Add b"]);
    env.run(&d, &["checkout", "main"]);
    write(&d, "c.txt", "c\n");
    env.run(&d, &["add", "."]);
    env.run(&d, &["commit", "-m", "Add c"]);
    env.run(&d, &["merge", "side"]);
    env.run(&d, &["revert", "HEAD~1"]);
    env.run(&d, &["checkout", "side"]);
    write(&d, "d.txt", "d\n");
    env.run(&d, &["add", "."]);
    env.run(&d, &["commit", "-m", "Add d"]);
    env.run(&d, &["checkout", "main"]);
    env.run(&d, &["cherry-pick", "side"]);
    env.run(&d, &["tag", "-a", "v1", "-m", "Release one"]);
    env.run(&d, &["fixme"]);
    env.run(&d, &["fixme-all"]);
    write(&d, "a.txt", "three\n");
    env.run(&d, &["stash"]);

    assert_clean(&env.all_commit_messages(&d));
    assert_clean(&env.run(&d, &["show", "v1"]));
    // Raw object bytes as well (headers included).
    for entry in walkdir(&d.join(".jg").join("objects")) {
        let data = std::fs::read(&entry).unwrap();
        let mut raw = Vec::new();
        flate2_decode(&data, &mut raw);
        if raw.starts_with(b"commit ") || raw.starts_with(b"tag ") {
            assert_clean(&String::from_utf8_lossy(&raw));
        }
    }
}

#[test]
fn ai_flag_uses_draft_verbatim_and_adds_nothing_by_default() {
    let env = Env::new();
    let d = env.repo("r");
    env.run(&d, &["config", "ai.command", "echo 'Add feature file'"]);
    write(&d, "f.txt", "x\n");
    env.run(&d, &["add", "."]);
    let v = env.json(&d, &["commit", "--ai", "--yes"]);
    assert_eq!(v["ai"], true);
    let log = env.json(&d, &["log", "-n", "1"]);
    assert_eq!(log["commits"][0]["message"], "Add feature file\n");
}

#[test]
fn ai_trailer_only_when_opted_in_and_flag_used() {
    let env = Env::new();
    let d = env.repo("r");
    env.run(&d, &["config", "ai.command", "echo 'Add g'"]);
    env.run(&d, &["config", "ai.trailer", "true"]);
    write(&d, "g.txt", "x\n");
    env.run(&d, &["add", "."]);
    env.run(&d, &["commit", "--ai", "--yes"]);
    let log = env.json(&d, &["log", "-n", "1"]);
    let msg = log["commits"][0]["message"].as_str().unwrap();
    assert!(msg.contains("AI-Assisted: echo"), "{msg}");
}

#[test]
fn ai_without_yes_outside_terminal_does_not_commit() {
    let env = Env::new();
    let d = env.repo("r");
    env.run(&d, &["config", "ai.command", "echo 'Draft'"]);
    write(&d, "h.txt", "x\n");
    env.run(&d, &["add", "."]);
    let o = env.run_raw(&d, &["commit", "--ai"]);
    assert!(!o.status.success());
    assert_eq!(env.json(&d, &["log"])["commits"].as_array().unwrap().len(), 1);
}

fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        if e.path().is_dir() {
            out.extend(walkdir(&e.path()));
        } else {
            out.push(e.path());
        }
    }
    out
}

fn flate2_decode(data: &[u8], out: &mut Vec<u8>) {
    use std::io::Read;
    flate2::read::ZlibDecoder::new(data).read_to_end(out).unwrap();
}
