//! Git-style INI config files.
//!
//! Keys are addressed as `section.key` or `section.subsection.key`
//! (e.g. `user.name`, `remote.origin.url`, `alias.st`).
//! Lookup order: repo config (`.jg/config`) overrides global config.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
struct Section {
    name: String,
    sub: Option<String>,
    items: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct ConfigFile {
    pub path: PathBuf,
    sections: Vec<Section>,
}

/// Split `a.b.c` into (section, subsection, key). Subsection may contain dots.
fn split_key(key: &str) -> Result<(String, Option<String>, String)> {
    let first = key.find('.');
    let last = key.rfind('.');
    match (first, last) {
        (Some(f), Some(l)) if f == l => Ok((key[..f].to_lowercase(), None, key[l + 1..].to_string())),
        (Some(f), Some(l)) => Ok((key[..f].to_lowercase(), Some(key[f + 1..l].to_string()), key[l + 1..].to_string())),
        _ => bail!("config key must look like 'section.key' (got '{key}')"),
    }
}

impl ConfigFile {
    pub fn load(path: &Path) -> ConfigFile {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut cfg = ConfigFile { path: path.to_path_buf(), sections: vec![] };
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                let inner = &line[1..line.len() - 1];
                let (name, sub) = match inner.split_once(' ') {
                    Some((n, s)) => (n.to_lowercase(), Some(s.trim().trim_matches('"').to_string())),
                    None => (inner.to_lowercase(), None),
                };
                cfg.sections.push(Section { name, sub, items: vec![] });
            } else if let Some(sec) = cfg.sections.last_mut() {
                let (k, v) = match line.split_once('=') {
                    Some((k, v)) => (k.trim(), unquote(v.trim())),
                    None => (line, "true".to_string()),
                };
                sec.items.push((k.to_string(), v));
            }
        }
        cfg
    }

    pub fn save(&self) -> Result<()> {
        let mut s = String::new();
        for sec in &self.sections {
            if sec.items.is_empty() {
                continue;
            }
            match &sec.sub {
                Some(sub) => s.push_str(&format!("[{} \"{}\"]\n", sec.name, sub)),
                None => s.push_str(&format!("[{}]\n", sec.name)),
            }
            for (k, v) in &sec.items {
                s.push_str(&format!("\t{} = {}\n", k, quote(v)));
            }
        }
        crate::util::atomic_write(&self.path, s.as_bytes())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let (name, sub, k) = split_key(key).ok()?;
        let mut found = None;
        for sec in &self.sections {
            if sec.name == name && sec.sub == sub {
                for (ik, iv) in &sec.items {
                    if ik.eq_ignore_ascii_case(&k) {
                        found = Some(iv.clone());
                    }
                }
            }
        }
        found
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let (name, sub, k) = split_key(key)?;
        for sec in self.sections.iter_mut() {
            if sec.name == name && sec.sub == sub {
                for item in sec.items.iter_mut() {
                    if item.0.eq_ignore_ascii_case(&k) {
                        item.1 = value.to_string();
                        return Ok(());
                    }
                }
                sec.items.push((k, value.to_string()));
                return Ok(());
            }
        }
        self.sections.push(Section { name, sub, items: vec![(k, value.to_string())] });
        Ok(())
    }

    /// Remove a key. Returns true if anything was removed.
    pub fn unset(&mut self, key: &str) -> Result<bool> {
        let (name, sub, k) = split_key(key)?;
        let mut removed = false;
        for sec in self.sections.iter_mut() {
            if sec.name == name && sec.sub == sub {
                let before = sec.items.len();
                sec.items.retain(|(ik, _)| !ik.eq_ignore_ascii_case(&k));
                removed |= sec.items.len() != before;
            }
        }
        Ok(removed)
    }

    /// Remove a whole `[section "sub"]`.
    pub fn remove_section(&mut self, name: &str, sub: Option<&str>) -> bool {
        let before = self.sections.len();
        self.sections.retain(|s| !(s.name == name && s.sub.as_deref() == sub));
        before != self.sections.len()
    }

    /// All `(full.key, value)` pairs in file order.
    pub fn entries(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for sec in &self.sections {
            for (k, v) in &sec.items {
                let full = match &sec.sub {
                    Some(sub) => format!("{}.{}.{}", sec.name, sub, k),
                    None => format!("{}.{}", sec.name, k),
                };
                out.push((full, v.clone()));
            }
        }
        out
    }

    /// Subsection names for a section, e.g. remote names for `remote`.
    pub fn subsections(&self, name: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for sec in &self.sections {
            if sec.name == name
                && let Some(s) = &sec.sub
                && !out.contains(s)
                && !sec.items.is_empty()
            {
                out.push(s.clone());
            }
        }
        out
    }
}

fn unquote(v: &str) -> String {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        v[1..v.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\")
    } else {
        v.to_string()
    }
}

fn quote(v: &str) -> String {
    if v.is_empty() || v.starts_with(' ') || v.ends_with(' ') || v.contains(['#', ';', '"', '\\']) {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        v.to_string()
    }
}

/// Path to the per-user config file.
/// Linux: `$XDG_CONFIG_HOME/jg/config` or `~/.config/jg/config`.
/// Windows: `%APPDATA%\jg\config`.
/// `JG_CONFIG_GLOBAL` overrides (used by tests).
pub fn global_path() -> PathBuf {
    if let Ok(p) = std::env::var("JG_CONFIG_GLOBAL") {
        return PathBuf::from(p);
    }
    if cfg!(windows)
        && let Ok(a) = std::env::var("APPDATA")
    {
        return PathBuf::from(a).join("jg").join("config");
    }
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME")
        && !x.is_empty()
    {
        return PathBuf::from(x).join("jg").join("config");
    }
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("jg").join("config")
}

/// Layered view: repo config first, then global.
#[derive(Clone, Debug)]
pub struct Config {
    pub repo: Option<ConfigFile>,
    pub global: ConfigFile,
}

impl Config {
    pub fn load(repo_config: Option<&Path>) -> Config {
        Config { repo: repo_config.map(ConfigFile::load), global: ConfigFile::load(&global_path()) }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.repo.as_ref().and_then(|r| r.get(key)).or_else(|| self.global.get(key))
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        match self.get(key).map(|v| v.to_lowercase()) {
            Some(v) if ["true", "yes", "on", "1"].contains(&v.as_str()) => true,
            Some(v) if ["false", "no", "off", "0"].contains(&v.as_str()) => false,
            _ => default,
        }
    }

    /// Merged `(key, value)` list, repo values overriding global ones.
    pub fn entries(&self) -> Vec<(String, String, &'static str)> {
        let mut out: Vec<(String, String, &'static str)> =
            self.global.entries().into_iter().map(|(k, v)| (k, v, "global")).collect();
        if let Some(r) = &self.repo {
            for (k, v) in r.entries() {
                out.retain(|(ek, _, _)| !ek.eq_ignore_ascii_case(&k));
                out.push((k, v, "repo"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config");
        let mut c = ConfigFile::load(&p);
        c.set("user.name", "Jane Doe").unwrap();
        c.set("remote.origin.url", "me@host:/srv/repo.jg").unwrap();
        c.set("alias.amend", "commit --amend --no-edit").unwrap();
        c.save().unwrap();
        let c2 = ConfigFile::load(&p);
        assert_eq!(c2.get("user.name").unwrap(), "Jane Doe");
        assert_eq!(c2.get("remote.origin.url").unwrap(), "me@host:/srv/repo.jg");
        assert_eq!(c2.get("alias.amend").unwrap(), "commit --amend --no-edit");
        assert_eq!(c2.subsections("remote"), vec!["origin".to_string()]);
    }
}
