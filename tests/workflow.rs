//! End-to-end tests of everyday workflows, driving the real binary.

mod common;
use common::*;

#[test]
fn status_add_commit_log() {
    let env = Env::new();
    let d = env.repo("r");
    write(&d, "new.txt", "n\n");
    write(&d, "README.md", "changed\n");
    let st = env.json(&d, &["status"]);
    assert_eq!(st["untracked"][0], "new.txt");
    assert_eq!(st["unstaged"][0]["path"], "README.md");
    env.run(&d, &["add", "-A"]);
    let st = env.json(&d, &["status"]);
    assert_eq!(st["staged"].as_array().unwrap().len(), 2);
    env.run(&d, &["commit", "-m", "Second"]);
    assert_eq!(env.json(&d, &["status"])["clean"], true);
    let log = env.json(&d, &["log"]);
    assert_eq!(log["commits"].as_array().unwrap().len(), 2);
    assert_eq!(log["commits"][0]["summary"], "Second");
}

#[test]
fn ignore_files_are_respected() {
    let env = Env::new();
    let d = env.repo("r");
    write(&d, ".jgignore", "*.log\nbuild/\n");
    write(&d, "x.log", "x");
    write(&d, "build/out.bin", "x");
    write(&d, "keep.txt", "x");
    let st = env.json(&d, &["status"]);
    let untracked: Vec<&str> = st["untracked"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(untracked, vec![".jgignore", "keep.txt"]);
    let o = env.run_raw(&d, &["add", "x.log"]);
    assert!(!o.status.success(), "adding an ignored file without -f must fail");
    env.run(&d, &["add", "-f", "x.log"]);
}

#[test]
fn branch_merge_conflict_and_continue() {
    let env = Env::new();
    let d = env.repo("r");
    env.run(&d, &["checkout", "-b", "feature"]);
    write(&d, "README.md", "feature\n");
    env.run(&d, &["commit", "-am", "feature"]);
    env.run(&d, &["checkout", "main"]);
    write(&d, "README.md", "main\n");
    env.run(&d, &["commit", "-am", "main"]);
    let o = env.run_raw(&d, &["merge", "feature"]);
    assert_eq!(o.status.code(), Some(1), "conflicts exit with 1");
    assert!(read(&d, "README.md").contains("<<<<<<< main"));
    let st = env.json(&d, &["status"]);
    assert_eq!(st["conflicts"][0], "README.md");
    assert_eq!(st["in_progress"], "merge");
    assert!(!env.run_raw(&d, &["commit", "-m", "x"]).status.success(), "commit must refuse with conflicts");
    write(&d, "README.md", "resolved\n");
    env.run(&d, &["add", "README.md"]);
    env.run(&d, &["merge", "--continue"]);
    let log = env.json(&d, &["log", "-n", "1"]);
    assert_eq!(log["commits"][0]["parents"].as_array().unwrap().len(), 2);
    assert_eq!(env.json(&d, &["status"])["clean"], true);
}

#[test]
fn clean_merge_and_fast_forward() {
    let env = Env::new();
    let d = env.repo("r");
    write(&d, "f.txt", "1\n2\n3\n4\n5\n6\n7\n8\n");
    env.run(&d, &["add", "."]);
    env.run(&d, &["commit", "-m", "f"]);
    env.run(&d, &["branch", "b"]);
    env.run(&d, &["checkout", "b"]);
    write(&d, "f.txt", "ONE\n2\n3\n4\n5\n6\n7\n8\n");
    env.run(&d, &["commit", "-am", "top"]);
    env.run(&d, &["checkout", "main"]);
    let v = env.json(&d, &["merge", "b"]);
    assert_eq!(v["result"], "fast-forward");
    env.run(&d, &["checkout", "-b", "c", "HEAD~1"]);
    write(&d, "f.txt", "1\n2\n3\n4\n5\n6\n7\nEIGHT\n");
    env.run(&d, &["commit", "-am", "bottom"]);
    env.run(&d, &["checkout", "main"]);
    let v = env.json(&d, &["merge", "c"]);
    assert_eq!(v["result"], "merged");
    assert_eq!(read(&d, "f.txt"), "ONE\n2\n3\n4\n5\n6\n7\nEIGHT\n");
}

#[test]
fn checkout_refuses_to_clobber_local_changes() {
    let env = Env::new();
    let d = env.repo("r");
    env.run(&d, &["checkout", "-b", "other"]);
    write(&d, "README.md", "other\n");
    env.run(&d, &["commit", "-am", "other"]);
    env.run(&d, &["checkout", "main"]);
    write(&d, "README.md", "local edit\n");
    assert!(!env.run_raw(&d, &["checkout", "other"]).status.success());
    assert_eq!(read(&d, "README.md"), "local edit\n");
    env.run(&d, &["checkout", "-f", "other"]);
    assert_eq!(read(&d, "README.md"), "other\n");
}

#[test]
fn stash_push_pop_with_untracked() {
    let env = Env::new();
    let d = env.repo("r");
    write(&d, "README.md", "wip\n");
    write(&d, "scratch.txt", "s\n");
    env.run(&d, &["stash", "-u", "-m", "my work"]);
    assert_eq!(read(&d, "README.md"), "hello\n");
    assert!(!d.join("scratch.txt").exists());
    let l = env.json(&d, &["stash", "list"]);
    assert_eq!(l["stashes"][0]["message"], "On main: my work");
    env.run(&d, &["stash", "pop"]);
    assert_eq!(read(&d, "README.md"), "wip\n");
    assert_eq!(read(&d, "scratch.txt"), "s\n");
    let st = env.json(&d, &["status"]);
    assert_eq!(st["untracked"][0], "scratch.txt");
    assert_eq!(env.json(&d, &["stash", "list"])["stashes"].as_array().unwrap().len(), 0);
}

#[test]
fn reset_restore_amend_alias() {
    let env = Env::new();
    let d = env.repo("r");
    write(&d, "README.md", "two\n");
    env.run(&d, &["commit", "-am", "two"]);
    env.run(&d, &["reset", "--soft", "HEAD~1"]);
    assert_eq!(env.json(&d, &["status"])["staged"][0]["path"], "README.md");
    env.run(&d, &["restore", "--staged", "README.md"]);
    assert_eq!(env.json(&d, &["status"])["unstaged"][0]["path"], "README.md");
    env.run(&d, &["restore", "README.md"]);
    assert_eq!(read(&d, "README.md"), "hello\n");
    // Built-in alias `amend` = commit --amend --no-edit; `st` = status.
    write(&d, "x.txt", "x");
    env.run(&d, &["add", "x.txt"]);
    env.run(&d, &["amend"]);
    let log = env.json(&d, &["log"]);
    assert_eq!(log["commits"].as_array().unwrap().len(), 1);
    assert!(env.run(&d, &["st"]).contains("working tree clean"));
    // User-defined alias.
    env.run(&d, &["alias", "set", "hist", "log", "--oneline"]);
    assert!(env.run(&d, &["hist"]).contains("Initial commit"));
}

#[test]
fn push_pull_clone_local_path() {
    let env = Env::new();
    let a = env.repo("a");
    let server = env.path("server.jg");
    env.run(env.root.path(), &["init", "--bare", server.to_str().unwrap()]);
    env.run(&a, &["remote", "add", "origin", server.to_str().unwrap()]);
    env.run(&a, &["push", "-u", "origin", "main"]);
    env.run(env.root.path(), &["clone", server.to_str().unwrap(), "b"]);
    let b = env.path("b");
    assert_eq!(read(&b, "README.md"), "hello\n");
    write(&b, "from_b.txt", "b\n");
    env.run(&b, &["add", "."]);
    env.run(&b, &["commit", "-m", "from b"]);
    env.run(&b, &["push"]);
    // a diverges, push is rejected, pull merges, push succeeds.
    write(&a, "from_a.txt", "a\n");
    env.run(&a, &["add", "."]);
    env.run(&a, &["commit", "-m", "from a"]);
    assert!(!env.run_raw(&a, &["push"]).status.success());
    env.run(&a, &["pull"]);
    assert!(a.join("from_b.txt").exists());
    env.run(&a, &["push"]);
    env.run(&b, &["pull"]);
    assert!(b.join("from_a.txt").exists());
    let st = env.json(&b, &["status"]);
    assert_eq!(st["tracking"]["ahead"], 0);
    assert_eq!(st["tracking"]["behind"], 0);
}

#[cfg(unix)]
#[test]
fn ssh_transport_via_fake_ssh() {
    let env = Env::new();
    let a = env.repo("a");
    let server = env.path("srv.jg");
    env.run(env.root.path(), &["init", "--bare", server.to_str().unwrap()]);
    let fake = env.path("fakessh");
    // Drops ssh options and the host, then runs the remote command locally.
    std::fs::write(
        &fake,
        "#!/bin/sh\nwhile [ $# -gt 1 ]; do case \"$1\" in -p) shift 2;; *) shift;; esac; done\nexec sh -c \"$1\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let bin_dir = std::path::Path::new(env!("CARGO_BIN_EXE_jg")).parent().unwrap().to_path_buf();
    let path = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap());
    let url = format!("me@host:{}", server.display());
    let run = |dir: &std::path::Path, args: &[&str]| {
        let o = env.cmd(dir, args).env("JG_SSH", &fake).env("PATH", &path).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    };
    run(&a, &["remote", "add", "origin", &url]);
    run(&a, &["push", "-u", "origin", "main"]);
    run(env.root.path(), &["clone", &url, "c"]);
    assert_eq!(read(&env.path("c"), "README.md"), "hello\n");
}

#[test]
fn git_export_produces_identical_commits() {
    if !git_available() {
        eprintln!("git not installed; skipping");
        return;
    }
    let env = Env::new();
    let d = env.repo("r");
    write(&d, "bin/run.sh", "#!/bin/sh\necho hi\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d.join("bin/run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    env.run(&d, &["add", "."]);
    env.run(&d, &["commit", "-m", "Add script"]);
    env.run(&d, &["tag", "-a", "v1.0", "-m", "Version 1.0"]);
    let head = env.json(&d, &["log", "-n", "1"])["commits"][0]["commit"].as_str().unwrap().to_string();
    env.run(&d, &["git-export"]);
    let git = |args: &[&str]| {
        let o = std::process::Command::new("git").args(args).current_dir(&d).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    assert_eq!(git(&["rev-parse", "HEAD"]), head);
    assert_eq!(git(&["rev-parse", "v1.0^{commit}"]), head);
    git(&["fsck", "--strict"]);
    assert_eq!(git(&["status", "--porcelain"]), "", "git sees a clean work tree");
    // A second export after a new commit only copies new objects.
    write(&d, "more.txt", "m\n");
    env.run(&d, &["add", "."]);
    env.run(&d, &["commit", "-m", "More"]);
    let v = env.json(&d, &["git-export"]);
    assert_eq!(v["objects_copied"], 3); // commit + root tree + blob
    assert_eq!(git(&["status", "--porcelain"]), "");
}

#[test]
fn json_errors_and_exit_codes() {
    let env = Env::new();
    let d = env.path("empty");
    std::fs::create_dir_all(&d).unwrap();
    let o = env.run_raw(&d, &["status", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert!(v["error"].as_str().unwrap().contains("not a jg repository"));
}

#[test]
fn diff_outputs() {
    let env = Env::new();
    let d = env.repo("r");
    write(&d, "README.md", "hello\nworld\n");
    let text = env.run(&d, &["diff"]);
    assert!(text.contains("+world"));
    let v = env.json(&d, &["diff"]);
    assert_eq!(v["additions"], 1);
    assert_eq!(v["files"][0]["hunks"][0]["lines"][1]["tag"], "+");
    env.run(&d, &["add", "."]);
    assert_eq!(env.run(&d, &["diff"]).trim(), "");
    assert!(env.run(&d, &["diff", "--staged"]).contains("+world"));
}

/// Run jg as a specific identity.
fn run_as(env: &Env, dir: &std::path::Path, name: &str, email: &str, args: &[&str]) -> std::process::Output {
    env.cmd(dir, args).env("JG_AUTHOR_NAME", name).env("JG_AUTHOR_EMAIL", email).output().unwrap()
}

fn ok(o: std::process::Output) -> String {
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn reset_author_only_touches_your_commits() {
    let env = Env::new();
    let d = env.path("r");
    std::fs::create_dir_all(&d).unwrap();
    let dummy = ("Dummy", "dummy@example.com");
    let other = ("Someone Else", "else@example.com");
    let me = ("Real Me", "me@example.com");
    ok(run_as(&env, &d, dummy.0, dummy.1, &["init"]));
    write(&d, "a.txt", "a\n");
    ok(run_as(&env, &d, dummy.0, dummy.1, &["add", "."]));
    ok(run_as(&env, &d, dummy.0, dummy.1, &["commit", "-m", "mine 1"]));
    write(&d, "b.txt", "b\n");
    ok(run_as(&env, &d, other.0, other.1, &["add", "."]));
    ok(run_as(&env, &d, other.0, other.1, &["commit", "-m", "theirs"]));
    write(&d, "c.txt", "c\n");
    ok(run_as(&env, &d, dummy.0, dummy.1, &["add", "."]));
    ok(run_as(&env, &d, dummy.0, dummy.1, &["commit", "-m", "mine 2"]));
    ok(run_as(&env, &d, dummy.0, dummy.1, &["tag", "-a", "v1", "-m", "tag"]));
    ok(run_as(&env, &d, dummy.0, dummy.1, &["branch", "side", "HEAD~1"]));

    // Without knowing the old email, the last commit isn't recognised as yours.
    assert!(!run_as(&env, &d, me.0, me.1, &["fixme"]).status.success());
    // Dry run changes nothing.
    ok(run_as(&env, &d, me.0, me.1, &["fixme-all", "--old-email", dummy.1, "--dry-run"]));
    let log = |args: &[&str]| -> serde_json::Value {
        let mut a = vec!["log", "--json"];
        a.extend_from_slice(args);
        serde_json::from_str(&ok(run_as(&env, &d, me.0, me.1, &a))).unwrap()
    };
    assert_eq!(log(&[])["commits"][0]["author"]["email"], dummy.1);

    // Remember the old email, then rewrite everything.
    env.run(&d, &["config", "user.formerEmails", dummy.1]);
    ok(run_as(&env, &d, me.0, me.1, &["fixme-all"]));
    let all = log(&["--all"]);
    let commits = all["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 3);
    for c in commits {
        let email = c["author"]["email"].as_str().unwrap();
        if c["summary"] == "theirs" {
            assert_eq!(email, other.1, "other people's commits keep their author");
            assert_eq!(c["committer"]["email"], other.1);
        } else {
            assert_eq!(email, me.1);
            assert_eq!(c["author"]["name"], me.0);
        }
    }
    // Branch `side` and tag `v1` follow the rewritten commits.
    assert_eq!(log(&["side", "-n", "1"])["commits"][0]["summary"], "theirs");
    let show: serde_json::Value =
        serde_json::from_str(&ok(run_as(&env, &d, me.0, me.1, &["show", "v1", "--json"]))).unwrap();
    assert!(show["tag"]["tagger"].as_str().unwrap().contains(me.1));
    assert_eq!(show["commit"]["commit"], log(&[])["commits"][0]["commit"]);
    // Running again is a no-op, and the work tree is untouched.
    let again: serde_json::Value =
        serde_json::from_str(&ok(run_as(&env, &d, me.0, me.1, &["fixme-all", "--json"]))).unwrap();
    assert_eq!(again["rewritten"], 0);
    assert_eq!(env.json(&d, &["status"])["clean"], true);

    // Last-commit alias: works on your own commit, refuses someone else's.
    write(&d, "d.txt", "d\n");
    ok(run_as(&env, &d, dummy.0, dummy.1, &["add", "."]));
    ok(run_as(&env, &d, dummy.0, dummy.1, &["commit", "-m", "mine 3"]));
    ok(run_as(&env, &d, me.0, me.1, &["fixme"]));
    let top = &log(&["-n", "1"])["commits"][0];
    assert_eq!((top["author"]["email"].as_str(), top["summary"].as_str()), (Some(me.1), Some("mine 3")));
    write(&d, "e.txt", "e\n");
    ok(run_as(&env, &d, other.0, other.1, &["add", "."]));
    ok(run_as(&env, &d, other.0, other.1, &["commit", "-m", "theirs 2"]));
    assert!(!run_as(&env, &d, me.0, me.1, &["fixme"]).status.success());
    assert_eq!(log(&["-n", "1"])["commits"][0]["author"]["email"], other.1);
}
