//! Test helpers: run the real `jg` binary in a temporary directory with an
//! isolated config and a fixed identity.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub struct Env {
    pub root: tempfile::TempDir,
}

impl Env {
    pub fn new() -> Env {
        Env { root: tempfile::tempdir().unwrap() }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }

    pub fn cmd(&self, dir: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_jg"));
        c.args(args)
            .current_dir(dir)
            .env("JG_CONFIG_GLOBAL", self.root.path().join("global-config"))
            .env("JG_AUTHOR_NAME", "Test User")
            .env("JG_AUTHOR_EMAIL", "test@example.com")
            .env("NO_COLOR", "1")
            .env_remove("JG_EDITOR");
        c
    }

    /// Run jg and return the output regardless of success.
    pub fn run_raw(&self, dir: &Path, args: &[&str]) -> Output {
        self.cmd(dir, args).output().unwrap()
    }

    /// Run jg, assert success, return stdout.
    pub fn run(&self, dir: &Path, args: &[&str]) -> String {
        let o = self.run_raw(dir, args);
        assert!(
            o.status.success(),
            "jg {:?} failed\nstdout: {}\nstderr: {}",
            args,
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    }

    /// Run jg with --json and parse the result.
    pub fn json(&self, dir: &Path, args: &[&str]) -> serde_json::Value {
        let mut a = args.to_vec();
        a.push("--json");
        serde_json::from_str(&self.run(dir, &a)).unwrap()
    }

    /// Create a repo at `name` with one commit.
    pub fn repo(&self, name: &str) -> PathBuf {
        let d = self.path(name);
        std::fs::create_dir_all(&d).unwrap();
        self.run(&d, &["init"]);
        write(&d, "README.md", "hello\n");
        self.run(&d, &["add", "."]);
        self.run(&d, &["commit", "-m", "Initial commit"]);
        d
    }

    /// Full text of every commit object reachable from all branches.
    pub fn all_commit_messages(&self, dir: &Path) -> String {
        let v = self.json(dir, &["log", "--all"]);
        v["commits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["message"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
            .join("\n---\n")
    }
}

pub fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

pub fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

pub fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}
