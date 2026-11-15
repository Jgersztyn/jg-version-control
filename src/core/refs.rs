//! References: `HEAD`, branches (`refs/heads/*`), tags (`refs/tags/*`),
//! remote-tracking branches (`refs/remotes/<remote>/*`), plus reflogs.
//! Layout matches git's loose refs.

use super::oid::Oid;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    /// HEAD points at a branch (full ref name, e.g. `refs/heads/main`). The branch may not exist yet.
    Branch(String),
    /// HEAD points directly at a commit.
    Detached(Oid),
}

#[derive(Clone, Debug, Serialize)]
pub struct ReflogEntry {
    pub old: Oid,
    pub new: Oid,
    pub who: String,
    pub message: String,
}

pub struct Refs {
    pub dir: PathBuf,
}

impl Refs {
    pub fn new(dir: PathBuf) -> Refs {
        Refs { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        let mut p = self.dir.clone();
        for part in name.split('/') {
            p.push(part);
        }
        p
    }

    pub fn read_head(&self) -> Result<Head> {
        let s = fs::read_to_string(self.dir.join("HEAD")).context("reading HEAD")?;
        let s = s.trim();
        if let Some(r) = s.strip_prefix("ref: ") {
            Ok(Head::Branch(r.trim().to_string()))
        } else {
            Ok(Head::Detached(Oid::from_hex(s)?))
        }
    }

    pub fn set_head_branch(&self, full_ref: &str) -> Result<()> {
        crate::util::atomic_write(&self.dir.join("HEAD"), format!("ref: {full_ref}\n").as_bytes())
    }

    pub fn set_head_detached(&self, oid: &Oid) -> Result<()> {
        crate::util::atomic_write(&self.dir.join("HEAD"), format!("{oid}\n").as_bytes())
    }

    /// Read a ref by full name (`refs/heads/main`) or `HEAD`. Follows symbolic refs.
    pub fn read(&self, name: &str) -> Result<Option<Oid>> {
        if name == "HEAD" {
            return match self.read_head()? {
                Head::Detached(o) => Ok(Some(o)),
                Head::Branch(r) => self.read(&r),
            };
        }
        let p = self.path(name);
        match fs::read_to_string(&p) {
            Ok(s) => {
                let s = s.trim();
                if let Some(target) = s.strip_prefix("ref: ") {
                    return self.read(target.trim());
                }
                Ok(Some(Oid::from_hex(s)?))
            }
            Err(_) => Ok(None),
        }
    }

    pub fn exists(&self, name: &str) -> bool {
        self.path(name).is_file()
    }

    /// Write a ref and append to its reflog (and HEAD's, when HEAD points at it).
    pub fn write(&self, name: &str, new: &Oid, who: &str, msg: &str) -> Result<()> {
        let old = self.read(name)?.unwrap_or(Oid::ZERO);
        crate::util::atomic_write(&self.path(name), format!("{new}\n").as_bytes())?;
        self.append_log(name, &old, new, who, msg)?;
        if let Ok(Head::Branch(h)) = self.read_head()
            && h == name
        {
            self.append_log("HEAD", &old, new, who, msg)?;
        }
        Ok(())
    }

    /// Update whatever HEAD points at (the current branch, or HEAD itself when detached).
    pub fn update_head(&self, new: &Oid, who: &str, msg: &str) -> Result<()> {
        match self.read_head()? {
            Head::Branch(r) => self.write(&r, new, who, msg),
            Head::Detached(old) => {
                self.set_head_detached(new)?;
                self.append_log("HEAD", &old, new, who, msg)
            }
        }
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        let p = self.path(name);
        if p.exists() {
            fs::remove_file(&p)?;
            crate::util::prune_empty_dirs(&p, &self.dir.join("refs"));
        }
        let log = self.dir.join("logs").join(name);
        if log.exists() {
            fs::remove_file(log).ok();
        }
        Ok(())
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<()> {
        let oid = self.read(from)?.with_context(|| format!("ref {from} not found"))?;
        let from_log = self.dir.join("logs").join(from);
        let to_log = self.dir.join("logs").join(to);
        crate::util::atomic_write(&self.path(to), format!("{oid}\n").as_bytes())?;
        if from_log.exists() {
            fs::create_dir_all(to_log.parent().unwrap())?;
            fs::rename(&from_log, &to_log).ok();
        }
        let p = self.path(from);
        fs::remove_file(&p)?;
        crate::util::prune_empty_dirs(&p, &self.dir.join("refs"));
        Ok(())
    }

    /// List all refs under a prefix like `refs/heads/`, sorted by name.
    pub fn list(&self, prefix: &str) -> Result<Vec<(String, Oid)>> {
        let mut out = Vec::new();
        let base = self.path(prefix.trim_end_matches('/'));
        walk(&base, prefix.trim_end_matches('/'), &mut |name, path| {
            if let Ok(s) = fs::read_to_string(path)
                && let Ok(oid) = Oid::from_hex(s.trim())
            {
                out.push((name.to_string(), oid));
            }
        });
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    fn append_log(&self, name: &str, old: &Oid, new: &Oid, who: &str, msg: &str) -> Result<()> {
        use std::io::Write;
        let p = self.dir.join("logs").join(name);
        fs::create_dir_all(p.parent().unwrap())?;
        let mut f = fs::OpenOptions::new().create(true).append(true).open(p)?;
        let msg = msg.lines().next().unwrap_or("");
        writeln!(f, "{old} {new} {who}\t{msg}")?;
        Ok(())
    }

    /// Reflog entries, newest first.
    pub fn reflog(&self, name: &str) -> Result<Vec<ReflogEntry>> {
        let p = self.dir.join("logs").join(name);
        let s = fs::read_to_string(p).unwrap_or_default();
        let mut out = Vec::new();
        for line in s.lines() {
            let (head, message) = line.split_once('\t').unwrap_or((line, ""));
            if head.len() < 82 {
                continue;
            }
            out.push(ReflogEntry {
                old: Oid::from_hex(&head[..40])?,
                new: Oid::from_hex(&head[41..81])?,
                who: head[82..].to_string(),
                message: message.to_string(),
            });
        }
        out.reverse();
        Ok(out)
    }
}

fn walk(dir: &Path, name: &str, f: &mut dyn FnMut(&str, &Path)) {
    if let Ok(rd) = fs::read_dir(dir) {
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let fname = e.file_name().to_string_lossy().into_owned();
            if fname.starts_with('.') {
                continue; // temp files
            }
            let child = format!("{name}/{fname}");
            let path = e.path();
            if path.is_dir() {
                walk(&path, &child, f);
            } else {
                f(&child, &path);
            }
        }
    }
}

/// Validate a branch/tag short name using git's rules (simplified).
pub fn check_name(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name.starts_with('-')
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.ends_with(".lock")
        || name.contains("..")
        || name.contains("//")
        || name.contains("@{")
        || name == "@"
        || name == "HEAD"
        || name.chars().any(|c| c.is_control() || " ~^:?*[\\".contains(c))
        || name.split('/').any(|p| p.starts_with('.'));
    if bad {
        bail!("'{name}' is not a valid name");
    }
    Ok(())
}
