//! git-import, line-ending handling and git coexistence.

mod common;
use common::*;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .args(["-c", "user.name=Git User", "-c", "user.email=git@example.com", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// A git repo with packed history, a branch, an annotated tag and one loose commit.
fn git_repo(env: &Env, name: &str) -> std::path::PathBuf {
    let d = env.path(name);
    std::fs::create_dir_all(&d).unwrap();
    git(&d, &["init", "-q", "."]);
    write(&d, "README.md", "hello\n");
    for i in 0..20 {
        write(&d, &format!("src/f{i}.txt"), &"same line\n".repeat(30 + i));
    }
    git(&d, &["add", "."]);
    git(&d, &["commit", "-qm", "first"]);
    for i in 0..20 {
        write(&d, &format!("src/f{i}.txt"), &format!("{}changed {i}\n", "same line\n".repeat(30 + i)));
    }
    git(&d, &["commit", "-qam", "second"]);
    git(&d, &["tag", "-a", "v1", "-m", "version 1"]);
    git(&d, &["checkout", "-qb", "dev"]);
    write(&d, "dev.txt", "dev\n");
    git(&d, &["add", "."]);
    git(&d, &["commit", "-qm", "dev work"]);
    git(&d, &["checkout", "-q", "main"]);
    git(&d, &["gc", "-q", "--aggressive"]); // packs + deltas
    write(&d, "README.md", "hello again\n");
    git(&d, &["commit", "-qam", "third (loose)"]);
    d
}

#[test]
fn import_packed_git_history_into_new_jg_repo() {
    if !git_available() {
        return;
    }
    let env = Env::new();
    let d = git_repo(&env, "g");
    let v = env.json(&d, &["git-import"]);
    assert_eq!(v["created_repository"], true);
    // Same ids as git, for every branch and tag.
    let jg_head =
        |rev: &str| env.json(&d, &["log", "-n", "1", rev])["commits"][0]["commit"].as_str().unwrap().to_string();
    assert_eq!(jg_head("main"), git(&d, &["rev-parse", "main"]));
    assert_eq!(jg_head("dev"), git(&d, &["rev-parse", "dev"]));
    assert_eq!(jg_head("v1"), git(&d, &["rev-parse", "v1^{commit}"]));
    assert_eq!(env.json(&d, &["log", "--all"])["commits"].as_array().unwrap().len(), 4);
    // Work tree matches the imported commit; git ignores .jg/.
    assert_eq!(env.json(&d, &["status"])["clean"], true);
    assert_eq!(git(&d, &["status", "--porcelain"]), "");
    // Running it again imports nothing new.
    assert_eq!(env.json(&d, &["git-import"])["objects_imported"], 0);
    // Normal work continues on top of git's history.
    write(&d, "README.md", "jg now\n");
    env.run(&d, &["commit", "-am", "first jg commit"]);
    let log = env.json(&d, &["log", "-n", "2"]);
    assert_eq!(log["commits"][1]["commit"].as_str().unwrap(), git(&d, &["rev-parse", "main"]));
}

#[test]
fn import_next_to_existing_jg_history() {
    if !git_available() {
        return;
    }
    let env = Env::new();
    let d = git_repo(&env, "g");
    // The user started jg fresh in the same folder before importing.
    env.run(&d, &["init"]);
    env.run(&d, &["add", "."]);
    env.run(&d, &["commit", "-m", "fresh jg start"]);
    let v = env.json(&d, &["git-import"]);
    assert_eq!(v["side_branches"][0]["jg"], "git/main");
    // Put jg's work on top of git's history, as the import message suggests.
    env.run(&d, &["reset", "git/main"]);
    assert_eq!(env.json(&d, &["status"])["clean"], true, "same files => nothing to commit");
    let log = env.json(&d, &["log"]);
    assert_eq!(log["commits"][0]["commit"].as_str().unwrap(), git(&d, &["rev-parse", "main"]));
}

#[test]
fn init_inside_git_repo_excludes_jg_dir() {
    if !git_available() {
        return;
    }
    let env = Env::new();
    let d = env.path("g");
    std::fs::create_dir_all(&d).unwrap();
    git(&d, &["init", "-q", "."]);
    env.run(&d, &["init"]);
    write(&d, "a.txt", "a\n");
    env.run(&d, &["add", "."]);
    env.run(&d, &["commit", "-m", "a"]);
    let st = git(&d, &["status", "--porcelain"]);
    assert!(!st.contains(".jg"), "git should not list .jg/: {st}");
}

#[test]
fn gitattributes_line_endings() {
    let env = Env::new();
    let d = env.repo("r");
    write(&d, ".gitattributes", "* text=auto\n*.bat eol=crlf\n*.bin binary\n");
    write(&d, "win.txt", "one\r\ntwo\r\n");
    write(&d, "run.bat", "echo hi\r\n");
    std::fs::write(d.join("data.bin"), b"a\r\nb\0").unwrap();
    env.run(&d, &["add", "."]);
    env.run(&d, &["commit", "-m", "eol"]);
    // CRLF on disk vs normalized blob is not a change.
    assert_eq!(env.json(&d, &["status"])["clean"], true);
    // A checkout elsewhere gets LF for text (Linux), CRLF where eol=crlf, bytes untouched for binary.
    env.run(env.root.path(), &["clone", d.to_str().unwrap(), "c"]);
    let c = env.path("c");
    let native = if cfg!(windows) { "one\r\ntwo\r\n" } else { "one\ntwo\n" };
    assert_eq!(read(&c, "win.txt"), native);
    assert_eq!(read(&c, "run.bat"), "echo hi\r\n");
    assert_eq!(std::fs::read(c.join("data.bin")).unwrap(), b"a\r\nb\0");
    assert_eq!(env.json(&c, &["status"])["clean"], true);
}
