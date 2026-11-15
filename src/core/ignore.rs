//! Ignore rules from `.jgignore` and `.gitignore` files (gitignore syntax subset):
//! `*`, `**`, `?`, `[abc]`, leading `/` (anchored), trailing `/` (dirs only), `!` (negate).
//! Rules in a subdirectory's ignore file apply relative to that directory.

use std::path::Path;

#[derive(Clone, Debug)]
struct Rule {
    /// Directory (repo-relative, `/`-separated, "" for root) the rule file lives in.
    base: String,
    pattern: String,
    negate: bool,
    dir_only: bool,
    /// Pattern contains a `/` (other than trailing), so it matches the full relative path.
    anchored: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Ignore {
    rules: Vec<Rule>,
}

pub const IGNORE_FILES: [&str; 2] = [".gitignore", ".jgignore"];

impl Ignore {
    /// Load ignore files that live directly in `dir` (repo-relative `base`).
    pub fn add_dir(&mut self, dir: &Path, base: &str) {
        for name in IGNORE_FILES {
            if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
                self.add_rules(&text, base);
            }
        }
    }

    pub fn add_rules(&mut self, text: &str, base: &str) {
        for line in text.lines() {
            let mut l = line.trim_end();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            let negate = l.starts_with('!');
            if negate {
                l = &l[1..];
            }
            let dir_only = l.ends_with('/');
            let l = l.trim_end_matches('/');
            let anchored = l.contains('/');
            let pattern = l.trim_start_matches('/').to_string();
            if pattern.is_empty() {
                continue;
            }
            self.rules.push(Rule { base: base.to_string(), pattern, negate, dir_only, anchored });
        }
    }

    /// Is the repo-relative path ignored? Last matching rule wins.
    pub fn is_ignored(&self, path: &str, is_dir: bool) -> bool {
        let mut ignored = false;
        for r in &self.rules {
            if r.dir_only && !is_dir {
                continue;
            }
            let rel = if r.base.is_empty() {
                path
            } else if let Some(rest) = path.strip_prefix(&r.base).and_then(|s| s.strip_prefix('/')) {
                rest
            } else {
                continue;
            };
            let matched = if r.anchored {
                glob(r.pattern.as_bytes(), rel.as_bytes())
            } else {
                let name = rel.rsplit('/').next().unwrap_or(rel);
                glob(r.pattern.as_bytes(), name.as_bytes())
            };
            if matched {
                ignored = !r.negate;
            }
        }
        ignored
    }
}

/// Glob match where `*` and `?` don't cross `/`, and `**` does.
pub fn glob(pat: &[u8], s: &[u8]) -> bool {
    if pat.is_empty() {
        return s.is_empty();
    }
    if pat.starts_with(b"**") {
        let rest = &pat[2..];
        let rest = rest.strip_prefix(b"/").unwrap_or(rest);
        // `**` matches zero or more path segments.
        for i in 0..=s.len() {
            if (i == 0 || s[i - 1] == b'/' || rest.is_empty()) && glob(rest, &s[i..]) {
                return true;
            }
        }
        return false;
    }
    match pat[0] {
        b'*' => {
            for i in 0..=s.len() {
                if i > 0 && s[i - 1] == b'/' {
                    break;
                }
                if glob(&pat[1..], &s[i..]) {
                    return true;
                }
            }
            false
        }
        b'?' => !s.is_empty() && s[0] != b'/' && glob(&pat[1..], &s[1..]),
        b'[' => {
            let Some(end) = pat.iter().position(|&c| c == b']') else {
                return !s.is_empty() && s[0] == b'[' && glob(&pat[1..], &s[1..]);
            };
            if s.is_empty() {
                return false;
            }
            let class = &pat[1..end];
            let (neg, class) = match class.first() {
                Some(b'!') | Some(b'^') => (true, &class[1..]),
                _ => (false, class),
            };
            let mut hit = false;
            let mut i = 0;
            while i < class.len() {
                if i + 2 < class.len() && class[i + 1] == b'-' {
                    hit |= class[i] <= s[0] && s[0] <= class[i + 2];
                    i += 3;
                } else {
                    hit |= class[i] == s[0];
                    i += 1;
                }
            }
            hit != neg && glob(&pat[end + 1..], &s[1..])
        }
        c => !s.is_empty() && s[0] == c && glob(&pat[1..], &s[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_rules() {
        let mut ig = Ignore::default();
        ig.add_rules("*.log\n/build/\ntarget\n!keep.log\ndocs/**/*.tmp\n", "");
        assert!(ig.is_ignored("a.log", false));
        assert!(ig.is_ignored("x/y/a.log", false));
        assert!(!ig.is_ignored("keep.log", false));
        assert!(ig.is_ignored("build", true));
        assert!(!ig.is_ignored("build", false));
        assert!(!ig.is_ignored("src/build", true));
        assert!(ig.is_ignored("src/target", true));
        assert!(ig.is_ignored("docs/a/b/c.tmp", false));
        assert!(ig.is_ignored("docs/c.tmp", false));
    }

    #[test]
    fn nested_base() {
        let mut ig = Ignore::default();
        ig.add_rules("*.o\n", "sub");
        assert!(ig.is_ignored("sub/a.o", false));
        assert!(!ig.is_ignored("a.o", false));
    }
}
