//! Line-ending handling compatible with git: `.gitattributes` (`text`, `text=auto`,
//! `-text`, `binary`, `eol=lf|crlf`) plus the `core.autocrlf` / `core.eol` settings.
//!
//! - **Clean** (work tree → stored blob): text files get CRLF → LF.
//! - **Smudge** (stored blob → work tree): text files get LF → CRLF when the
//!   configured end-of-line is CRLF (e.g. `eol=crlf`, or `core.eol=native` on Windows).
//!
//! With no `.gitattributes` and no `core.autocrlf`, files are stored byte-for-byte.

use super::ignore::glob;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Val {
    Set,
    Unset,
    Value(String),
}

#[derive(Clone, Debug)]
struct Rule {
    pattern: String,
    anchored: bool,
    attrs: Vec<(String, Val)>,
}

/// Parse one `.gitattributes` file.
fn parse(text: &str) -> Vec<Rule> {
    let mut rules = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(pat) = parts.next() else { continue };
        let mut attrs = Vec::new();
        for a in parts {
            if a == "binary" {
                // Macro: binary = -text -diff -merge
                attrs.push(("text".to_string(), Val::Unset));
            } else if let Some(n) = a.strip_prefix('-') {
                attrs.push((n.to_string(), Val::Unset));
            } else if let Some(n) = a.strip_prefix('!') {
                attrs.push((n.to_string(), Val::Unset));
            } else if let Some((n, v)) = a.split_once('=') {
                attrs.push((n.to_string(), Val::Value(v.to_string())));
            } else {
                attrs.push((a.to_string(), Val::Set));
            }
        }
        let anchored = pat.trim_end_matches('/').contains('/');
        rules.push(Rule { pattern: pat.trim_start_matches('/').to_string(), anchored, attrs });
    }
    rules
}

/// What to do with one file's line endings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Text {
    /// Not text (or no conversion configured): store as-is.
    Binary,
    /// Explicitly text: always normalize.
    Text,
    /// Auto-detect: normalize if the content looks like text.
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    Lf,
    Crlf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conversion {
    pub text: Text,
    /// End-of-line to write into the work tree.
    pub eol: Eol,
}

impl Conversion {
    pub const NONE: Conversion = Conversion { text: Text::Binary, eol: Eol::Lf };
}

/// Loads `.gitattributes` files lazily (per directory) and answers per-path questions.
#[derive(Default)]
pub struct Attributes {
    cache: HashMap<String, Vec<Rule>>,
}

impl Attributes {
    /// Forget cached rules (after a `.gitattributes` file was written).
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    fn rules_for_dir(&mut self, root: &Path, dir: &str) -> &Vec<Rule> {
        self.cache.entry(dir.to_string()).or_insert_with(|| {
            let base = if dir.is_empty() { root.to_path_buf() } else { crate::util::from_repo_path(root, dir) };
            std::fs::read_to_string(base.join(".gitattributes")).map(|t| parse(&t)).unwrap_or_default()
        })
    }

    /// Value of attribute `name` for `path` (last matching rule wins, deeper files win).
    fn lookup(&mut self, root: &Path, path: &str, name: &str) -> Option<Val> {
        let mut dirs = vec![String::new()];
        let parts: Vec<&str> = path.split('/').collect();
        for i in 1..parts.len() {
            dirs.push(parts[..i].join("/"));
        }
        let mut result = None;
        for dir in dirs {
            let rel = if dir.is_empty() { path.to_string() } else { path[dir.len() + 1..].to_string() };
            let basename = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            for r in self.rules_for_dir(root, &dir) {
                let target = if r.anchored { &rel } else { &basename };
                if glob(r.pattern.as_bytes(), target.as_bytes()) {
                    for (n, v) in &r.attrs {
                        if n == name {
                            result = Some(v.clone());
                        }
                    }
                }
            }
        }
        result
    }

    /// Decide how to convert `path`, given `core.autocrlf` and `core.eol` settings.
    pub fn conversion(&mut self, root: &Path, path: &str, autocrlf: &str, core_eol: &str) -> Conversion {
        let text_attr = self.lookup(root, path, "text");
        let eol_attr = self.lookup(root, path, "eol");
        let native = if cfg!(windows) { Eol::Crlf } else { Eol::Lf };
        let config_eol = match (autocrlf, core_eol) {
            ("true", _) => Eol::Crlf,
            ("input", _) => Eol::Lf,
            (_, "crlf") => Eol::Crlf,
            (_, "lf") => Eol::Lf,
            _ => native,
        };
        let eol = match &eol_attr {
            Some(Val::Value(v)) if v == "crlf" => Some(Eol::Crlf),
            Some(Val::Value(v)) if v == "lf" => Some(Eol::Lf),
            _ => None,
        };
        let text = match &text_attr {
            Some(Val::Unset) => Text::Binary,
            Some(Val::Set) => Text::Text,
            Some(Val::Value(v)) if v == "auto" => Text::Auto,
            // `eol=...` alone implies text.
            _ if eol.is_some() => Text::Text,
            _ if autocrlf == "true" || autocrlf == "input" => Text::Auto,
            _ => Text::Binary,
        };
        Conversion { text, eol: eol.unwrap_or(config_eol) }
    }
}

fn looks_text(data: &[u8]) -> bool {
    !crate::util::is_binary(data)
}

/// Work tree → blob.
pub fn clean(conv: Conversion, data: Vec<u8>) -> Vec<u8> {
    match conv.text {
        Text::Binary => data,
        Text::Auto if !looks_text(&data) => data,
        _ => {
            if !data.windows(2).any(|w| w == b"\r\n") {
                return data;
            }
            let mut out = Vec::with_capacity(data.len());
            let mut i = 0;
            while i < data.len() {
                if data[i] == b'\r' && data.get(i + 1) == Some(&b'\n') {
                    i += 1;
                    continue;
                }
                out.push(data[i]);
                i += 1;
            }
            out
        }
    }
}

/// Blob → work tree.
pub fn smudge(conv: Conversion, data: Vec<u8>) -> Vec<u8> {
    if conv.eol != Eol::Crlf || conv.text == Text::Binary {
        return data;
    }
    // Like git: leave content alone if it is binary or already has CRLF.
    if (conv.text == Text::Auto && !looks_text(&data)) || data.windows(2).any(|w| w == b"\r\n") {
        return data;
    }
    let mut out = Vec::with_capacity(data.len() + data.len() / 20);
    for &b in &data {
        if b == b'\n' {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_and_conversion() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitattributes"), "* text=auto\n*.sh eol=lf\n*.bat eol=crlf\n*.png binary\n")
            .unwrap();
        let mut a = Attributes::default();
        let r = dir.path();
        assert_eq!(a.conversion(r, "src/x.cs", "false", "lf"), Conversion { text: Text::Auto, eol: Eol::Lf });
        assert_eq!(a.conversion(r, "run.bat", "false", "lf").eol, Eol::Crlf);
        assert_eq!(a.conversion(r, "x/run.sh", "true", "").eol, Eol::Lf);
        assert_eq!(a.conversion(r, "img.png", "true", "").text, Text::Binary);
    }

    #[test]
    fn clean_and_smudge() {
        let c = Conversion { text: Text::Auto, eol: Eol::Crlf };
        assert_eq!(clean(c, b"a\r\nb\r\n".to_vec()), b"a\nb\n");
        assert_eq!(smudge(c, b"a\nb\n".to_vec()), b"a\r\nb\r\n");
        assert_eq!(clean(c, b"a\0\r\n".to_vec()), b"a\0\r\n"); // binary untouched
        let none = Conversion::NONE;
        assert_eq!(clean(none, b"a\r\n".to_vec()), b"a\r\n");
    }
}
