//! Git-compatible object model: blobs, trees, commits and annotated tags.
//!
//! Every object serializes to exactly the bytes git would produce, so object
//! ids match git's and commits can be exported to a real git repo unchanged.

use super::oid::Oid;
use anyhow::{Context, Result, bail};
use serde::Serialize;

pub const MODE_FILE: u32 = 0o100644;
pub const MODE_EXEC: u32 = 0o100755;
pub const MODE_SYMLINK: u32 = 0o120000;
pub const MODE_TREE: u32 = 0o040000;
pub const MODE_GITLINK: u32 = 0o160000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Blob,
    Tree,
    Commit,
    Tag,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Blob => "blob",
            Kind::Tree => "tree",
            Kind::Commit => "commit",
            Kind::Tag => "tag",
        }
    }

    pub fn parse(s: &str) -> Result<Kind> {
        Ok(match s {
            "blob" => Kind::Blob,
            "tree" => Kind::Tree,
            "commit" => Kind::Commit,
            "tag" => Kind::Tag,
            _ => bail!("unknown object type '{s}'"),
        })
    }
}

/// Author/committer/tagger line: `Name <email> <unix-secs> <+HHMM>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Signature {
    pub name: String,
    pub email: String,
    #[serde(skip)]
    pub time: i64,
    #[serde(skip)]
    pub tz_offset_min: i32,
}

impl Signature {
    pub fn now(name: &str, email: &str) -> Signature {
        let (time, tz_offset_min) = crate::util::now_with_offset();
        Signature { name: name.into(), email: email.into(), time, tz_offset_min }
    }

    pub fn to_line(&self) -> String {
        format!("{} <{}> {} {}", self.name, self.email, self.time, crate::util::format_tz(self.tz_offset_min))
    }

    pub fn parse(s: &str) -> Result<Signature> {
        let lt = s.find('<').context("bad signature")?;
        let gt = s.rfind('>').context("bad signature")?;
        let name = s[..lt].trim_end().to_string();
        let email = s[lt + 1..gt].to_string();
        let mut rest = s[gt + 1..].split_whitespace();
        let time = rest.next().and_then(|t| t.parse().ok()).unwrap_or(0);
        let tz = rest.next().map(crate::util::parse_tz).unwrap_or(0);
        Ok(Signature { name, email, time, tz_offset_min: tz })
    }

    pub fn date_human(&self) -> String {
        crate::util::format_date(self.time, self.tz_offset_min)
    }

    pub fn date_iso(&self) -> String {
        crate::util::iso_date(self.time, self.tz_offset_min)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: u32,
    pub name: String,
    pub oid: Oid,
}

impl TreeEntry {
    pub fn is_tree(&self) -> bool {
        self.mode == MODE_TREE
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub entries: Vec<TreeEntry>,
}

impl Tree {
    /// Git sorts tree entries by name, comparing directories as if they had a trailing '/'.
    pub fn sort(&mut self) {
        self.entries.sort_by_key(sort_key);
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for e in &self.entries {
            out.extend_from_slice(format!("{:o} {}\0", e.mode, e.name).as_bytes());
            out.extend_from_slice(&e.oid.0);
        }
        out
    }

    pub fn parse(data: &[u8]) -> Result<Tree> {
        let mut entries = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let sp = data[i..].iter().position(|&b| b == b' ').context("bad tree")? + i;
            let nul = data[sp..].iter().position(|&b| b == 0).context("bad tree")? + sp;
            let mode = u32::from_str_radix(std::str::from_utf8(&data[i..sp])?, 8)?;
            let name = String::from_utf8_lossy(&data[sp + 1..nul]).into_owned();
            let oid = Oid::from_bytes(data.get(nul + 1..nul + 21).context("truncated tree")?)?;
            entries.push(TreeEntry { mode, name, oid });
            i = nul + 21;
        }
        Ok(Tree { entries })
    }
}

fn sort_key(e: &TreeEntry) -> Vec<u8> {
    let mut k = e.name.as_bytes().to_vec();
    if e.is_tree() {
        k.push(b'/');
    }
    k
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub tree: Oid,
    pub parents: Vec<Oid>,
    pub author: Signature,
    pub committer: Signature,
    /// Headers we don't interpret (e.g. `gpgsig`, `encoding`), kept verbatim for round-trips.
    pub extra_headers: Vec<(String, String)>,
    pub message: String,
}

impl Commit {
    pub fn serialize(&self) -> Vec<u8> {
        let mut s = format!("tree {}\n", self.tree);
        for p in &self.parents {
            s.push_str(&format!("parent {p}\n"));
        }
        s.push_str(&format!("author {}\n", self.author.to_line()));
        s.push_str(&format!("committer {}\n", self.committer.to_line()));
        for (k, v) in &self.extra_headers {
            // Multi-line header values continue with a leading space, as in git.
            s.push_str(&format!("{} {}\n", k, v.replace('\n', "\n ")));
        }
        s.push('\n');
        s.push_str(&self.message);
        s.into_bytes()
    }

    pub fn parse(data: &[u8]) -> Result<Commit> {
        let text = String::from_utf8_lossy(data);
        let (headers, message) = split_headers(&text);
        let mut tree = None;
        let mut parents = Vec::new();
        let mut author = None;
        let mut committer = None;
        let mut extra_headers = Vec::new();
        for (k, v) in headers {
            match k.as_str() {
                "tree" => tree = Some(Oid::from_hex(&v)?),
                "parent" => parents.push(Oid::from_hex(&v)?),
                "author" => author = Some(Signature::parse(&v)?),
                "committer" => committer = Some(Signature::parse(&v)?),
                _ => extra_headers.push((k, v)),
            }
        }
        Ok(Commit {
            tree: tree.context("commit missing tree")?,
            parents,
            author: author.context("commit missing author")?,
            committer: committer.context("commit missing committer")?,
            extra_headers,
            message,
        })
    }

    /// First line of the message.
    pub fn summary(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub object: Oid,
    pub kind: Kind,
    pub name: String,
    pub tagger: Option<Signature>,
    pub message: String,
}

impl Tag {
    pub fn serialize(&self) -> Vec<u8> {
        let mut s = format!("object {}\ntype {}\ntag {}\n", self.object, self.kind.as_str(), self.name);
        if let Some(t) = &self.tagger {
            s.push_str(&format!("tagger {}\n", t.to_line()));
        }
        s.push('\n');
        s.push_str(&self.message);
        s.into_bytes()
    }

    pub fn parse(data: &[u8]) -> Result<Tag> {
        let text = String::from_utf8_lossy(data);
        let (headers, message) = split_headers(&text);
        let mut object = None;
        let mut kind = Kind::Commit;
        let mut name = String::new();
        let mut tagger = None;
        for (k, v) in headers {
            match k.as_str() {
                "object" => object = Some(Oid::from_hex(&v)?),
                "type" => kind = Kind::parse(&v)?,
                "tag" => name = v,
                "tagger" => tagger = Some(Signature::parse(&v)?),
                _ => {}
            }
        }
        Ok(Tag { object: object.context("tag missing object")?, kind, name, tagger, message })
    }
}

/// Split `key value` header lines (with space-continued lines) from the message body.
fn split_headers(text: &str) -> (Vec<(String, String)>, String) {
    let (head, msg) = match text.find("\n\n") {
        Some(i) => (&text[..i], text[i + 2..].to_string()),
        None => (text.trim_end_matches('\n'), String::new()),
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in head.lines() {
        if let Some(cont) = line.strip_prefix(' ') {
            if let Some(last) = headers.last_mut() {
                last.1.push('\n');
                last.1.push_str(cont);
            }
        } else if let Some((k, v)) = line.split_once(' ') {
            headers.push((k.to_string(), v.to_string()));
        }
    }
    (headers, msg)
}

/// Ensure a commit message ends with exactly one newline, as git does.
pub fn normalize_message(msg: &str) -> String {
    let trimmed = msg.trim_end();
    if trimmed.is_empty() { String::new() } else { format!("{trimmed}\n") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_roundtrip() {
        let sig = Signature { name: "A".into(), email: "a@x".into(), time: 1700000000, tz_offset_min: -300 };
        let c = Commit {
            tree: Oid::hash(b"t"),
            parents: vec![Oid::hash(b"p")],
            author: sig.clone(),
            committer: sig,
            extra_headers: vec![],
            message: "hello\n\nbody\n".into(),
        };
        let bytes = c.serialize();
        assert_eq!(Commit::parse(&bytes).unwrap(), c);
        assert!(String::from_utf8(bytes).unwrap().contains("author A <a@x> 1700000000 -0500"));
    }

    #[test]
    fn tree_sort_dirs_with_slash() {
        let mut t = Tree {
            entries: vec![
                TreeEntry { mode: MODE_FILE, name: "a.txt".into(), oid: Oid::ZERO },
                TreeEntry { mode: MODE_TREE, name: "a".into(), oid: Oid::ZERO },
                TreeEntry { mode: MODE_FILE, name: "a-b".into(), oid: Oid::ZERO },
            ],
        };
        t.sort();
        let names: Vec<_> = t.entries.iter().map(|e| e.name.as_str()).collect();
        // "a-b" < "a.txt" < "a/" because '-' (0x2d) < '.' (0x2e) < '/' (0x2f)
        assert_eq!(names, vec!["a-b", "a.txt", "a"]);
    }
}
