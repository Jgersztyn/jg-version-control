//! Object database: zlib-compressed loose objects at `objects/xx/yyyy...`.
//!
//! The on-disk file format is byte-identical to git's loose objects, so
//! exporting to git is a straight file copy.

use super::object::{Commit, Kind, Tag, Tree};
use super::oid::Oid;
use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct Odb {
    pub dir: PathBuf,
}

impl Odb {
    pub fn new(dir: PathBuf) -> Odb {
        Odb { dir }
    }

    pub fn path_for(&self, oid: &Oid) -> PathBuf {
        let hex = oid.to_hex();
        self.dir.join(&hex[..2]).join(&hex[2..])
    }

    pub fn exists(&self, oid: &Oid) -> bool {
        self.path_for(oid).exists()
    }

    /// Store an object and return its id. Writing an existing object is a no-op.
    pub fn write(&self, kind: Kind, data: &[u8]) -> Result<Oid> {
        let oid = Oid::hash_object(kind.as_str(), data);
        let path = self.path_for(&oid);
        if path.exists() {
            return Ok(oid);
        }
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(format!("{} {}\0", kind.as_str(), data.len()).as_bytes())?;
        enc.write_all(data)?;
        crate::util::atomic_write(&path, &enc.finish()?)?;
        Ok(oid)
    }

    /// Store an object given in raw "type len\0data" form after verifying its hash.
    pub fn write_raw(&self, expected: &Oid, raw: &[u8]) -> Result<()> {
        let (kind, data) = split_raw(raw)?;
        let oid = self.write(kind, data)?;
        if oid != *expected {
            bail!("object hash mismatch: expected {expected}, got {oid}");
        }
        Ok(())
    }

    /// Read the decompressed `"type len\0data"` bytes of an object.
    pub fn read_raw(&self, oid: &Oid) -> Result<Vec<u8>> {
        let path = self.path_for(oid);
        let file = fs::File::open(&path).with_context(|| format!("object {oid} not found"))?;
        let mut out = Vec::new();
        ZlibDecoder::new(file).read_to_end(&mut out).with_context(|| format!("corrupt object {oid}"))?;
        Ok(out)
    }

    pub fn read(&self, oid: &Oid) -> Result<(Kind, Vec<u8>)> {
        let raw = self.read_raw(oid)?;
        let (kind, data) = split_raw(&raw)?;
        Ok((kind, data.to_vec()))
    }

    pub fn read_blob(&self, oid: &Oid) -> Result<Vec<u8>> {
        let (k, d) = self.read(oid)?;
        if k != Kind::Blob {
            bail!("{oid} is a {}, not a blob", k.as_str());
        }
        Ok(d)
    }

    pub fn read_tree(&self, oid: &Oid) -> Result<Tree> {
        let (k, d) = self.read(oid)?;
        if k != Kind::Tree {
            bail!("{oid} is a {}, not a tree", k.as_str());
        }
        Tree::parse(&d)
    }

    pub fn read_commit(&self, oid: &Oid) -> Result<Commit> {
        let (k, d) = self.read(oid)?;
        if k != Kind::Commit {
            bail!("{oid} is a {}, not a commit", k.as_str());
        }
        Commit::parse(&d)
    }

    pub fn read_tag(&self, oid: &Oid) -> Result<Tag> {
        let (k, d) = self.read(oid)?;
        if k != Kind::Tag {
            bail!("{oid} is a {}, not a tag", k.as_str());
        }
        Tag::parse(&d)
    }

    pub fn write_blob(&self, data: &[u8]) -> Result<Oid> {
        self.write(Kind::Blob, data)
    }

    pub fn write_tree(&self, tree: &Tree) -> Result<Oid> {
        self.write(Kind::Tree, &tree.serialize())
    }

    pub fn write_commit(&self, c: &Commit) -> Result<Oid> {
        self.write(Kind::Commit, &c.serialize())
    }

    /// All object ids whose hex starts with `prefix` (for abbreviated ids).
    pub fn find_prefix(&self, prefix: &str) -> Result<Vec<Oid>> {
        let prefix = prefix.to_ascii_lowercase();
        if prefix.len() < 4 || !prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(vec![]);
        }
        let sub = self.dir.join(&prefix[..2]);
        let mut out = Vec::new();
        if let Ok(rd) = fs::read_dir(&sub) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let full = format!("{}{}", &prefix[..2], name);
                if full.len() == 40 && full.starts_with(&prefix) {
                    out.push(Oid::from_hex(&full)?);
                }
            }
        }
        Ok(out)
    }

    /// Path of the compressed file (used for byte-for-byte copies to git).
    pub fn file_path(&self, oid: &Oid) -> PathBuf {
        self.path_for(oid)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Split raw object bytes into (kind, payload), validating the length header.
pub fn split_raw(raw: &[u8]) -> Result<(Kind, &[u8])> {
    let nul = raw.iter().position(|&b| b == 0).context("bad object header")?;
    let header = std::str::from_utf8(&raw[..nul])?;
    let (kind, len) = header.split_once(' ').context("bad object header")?;
    let len: usize = len.parse()?;
    let data = &raw[nul + 1..];
    if data.len() != len {
        bail!("object length mismatch");
    }
    Ok((Kind::parse(kind)?, data))
}
