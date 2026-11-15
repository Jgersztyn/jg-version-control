//! Read-only access to a real git repository, used by `jg git-import`.
//! No git installation is needed: this parses git's on-disk formats directly.
//!
//! - Loose objects: `objects/xx/yyyy` (same format jg uses).
//! - Packfiles: `objects/pack/pack-*.pack` + `.idx` (index v1/v2), including
//!   OFS_DELTA and REF_DELTA compressed objects.
//! - Refs: loose `refs/**` files and `packed-refs`; `HEAD`.

use super::object::Kind;
use super::oid::Oid;
use anyhow::{Context, Result, bail};
use flate2::read::ZlibDecoder;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// A fully resolved packed object, shared between delta chains.
type Resolved = (Kind, std::rc::Rc<Vec<u8>>);

struct Pack {
    name: String,
    data: Vec<u8>,
    /// Object id -> byte offset in `data`.
    offsets: HashMap<Oid, usize>,
}

pub struct GitRepo {
    pub dir: PathBuf,
    packs: Vec<Pack>,
    /// Resolved objects by (pack index, offset), to avoid re-applying delta chains.
    cache: HashMap<(usize, usize), Resolved>,
    cache_bytes: usize,
}

const CACHE_LIMIT: usize = 256 * 1024 * 1024;

impl GitRepo {
    /// Open a git dir (`.git`, or a bare repo). Also accepts a work tree containing `.git`.
    pub fn open(path: &Path) -> Result<GitRepo> {
        let dir = if path.join(".git").is_dir() { path.join(".git") } else { path.to_path_buf() };
        if path.join(".git").is_file() {
            bail!(
                "{} uses a .git file (worktree/submodule); point git-import at the real git directory",
                path.display()
            );
        }
        if !dir.join("objects").is_dir() || !dir.join("HEAD").is_file() {
            bail!("not a git repository: {}", path.display());
        }
        if dir.join("shallow").exists() {
            bail!("shallow git clones can't be imported (history is incomplete). Run 'git fetch --unshallow' first.");
        }
        if dir.join("objects").join("info").join("alternates").exists() {
            bail!("git repositories using 'alternates' aren't supported. Run 'git repack -a -d' first.");
        }
        let mut packs = Vec::new();
        if let Ok(rd) = fs::read_dir(dir.join("objects").join("pack")) {
            let mut names: Vec<PathBuf> =
                rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "idx")).collect();
            names.sort();
            for idx in names {
                let pack = idx.with_extension("pack");
                if pack.exists() {
                    packs.push(load_pack(&idx, &pack)?);
                }
            }
        }
        Ok(GitRepo { dir, packs, cache: HashMap::new(), cache_bytes: 0 })
    }

    /// Every object id in the repository (loose and packed).
    pub fn all_oids(&self) -> Result<Vec<Oid>> {
        let mut out: Vec<Oid> = Vec::new();
        let objdir = self.dir.join("objects");
        if let Ok(rd) = fs::read_dir(&objdir) {
            for sub in rd.flatten() {
                let name = sub.file_name().to_string_lossy().into_owned();
                if name.len() != 2 || !name.chars().all(|c| c.is_ascii_hexdigit()) {
                    continue;
                }
                for f in fs::read_dir(sub.path())?.flatten() {
                    let rest = f.file_name().to_string_lossy().into_owned();
                    if let Ok(o) = Oid::from_hex(&format!("{name}{rest}")) {
                        out.push(o);
                    }
                }
            }
        }
        for p in &self.packs {
            out.extend(p.offsets.keys().copied());
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    pub fn pack_names(&self) -> Vec<String> {
        self.packs.iter().map(|p| p.name.clone()).collect()
    }

    /// Read an object's kind and content.
    pub fn read(&mut self, oid: &Oid) -> Result<(Kind, Vec<u8>)> {
        let hex = oid.to_hex();
        let loose = self.dir.join("objects").join(&hex[..2]).join(&hex[2..]);
        if loose.is_file() {
            let mut raw = Vec::new();
            ZlibDecoder::new(fs::File::open(&loose)?).read_to_end(&mut raw)?;
            let (k, d) = super::odb::split_raw(&raw)?;
            return Ok((k, d.to_vec()));
        }
        for i in 0..self.packs.len() {
            if let Some(&off) = self.packs[i].offsets.get(oid) {
                let (k, d) = self.read_packed(i, off)?;
                return Ok((k, (*d).clone()));
            }
        }
        bail!("git object {oid} not found")
    }

    fn read_packed(&mut self, pack: usize, offset: usize) -> Result<(Kind, std::rc::Rc<Vec<u8>>)> {
        if let Some((k, d)) = self.cache.get(&(pack, offset)) {
            return Ok((*k, d.clone()));
        }
        // Follow the delta chain down to a full object, then apply deltas back up.
        let mut chain: Vec<Vec<u8>> = Vec::new();
        let mut off = offset;
        let (base_kind, mut data) = loop {
            if let Some((k, d)) = self.cache.get(&(pack, off)) {
                break (*k, (**d).clone());
            }
            let entry = parse_entry(&self.packs[pack].data, off)
                .with_context(|| format!("corrupt object at offset {off} in {}", self.packs[pack].name))?;
            match entry.base {
                Base::None(kind) => break (kind, entry.data),
                Base::Offset(b) => {
                    chain.push(entry.data);
                    off = b;
                }
                Base::Ref(base_oid) => {
                    chain.push(entry.data);
                    let (k, d) = self.read(&base_oid)?;
                    break (k, d);
                }
            }
        };
        while let Some(delta) = chain.pop() {
            data = apply_delta(&data, &delta)?;
        }
        let rc = std::rc::Rc::new(data);
        if self.cache_bytes > CACHE_LIMIT {
            self.cache.clear();
            self.cache_bytes = 0;
        }
        self.cache_bytes += rc.len();
        self.cache.insert((pack, offset), (base_kind, rc.clone()));
        Ok((base_kind, rc))
    }

    /// All refs (`refs/heads/*`, `refs/tags/*`, ...), loose refs overriding packed ones.
    pub fn refs(&self) -> Result<BTreeMap<String, Oid>> {
        let mut out = BTreeMap::new();
        if let Ok(text) = fs::read_to_string(self.dir.join("packed-refs")) {
            for line in text.lines() {
                if line.starts_with('#') || line.starts_with('^') {
                    continue;
                }
                if let Some((hex, name)) = line.split_once(' ')
                    && let Ok(o) = Oid::from_hex(hex)
                {
                    out.insert(name.trim().to_string(), o);
                }
            }
        }
        walk_refs(&self.dir.join("refs"), "refs", &mut out);
        Ok(out)
    }

    /// The branch HEAD points at (e.g. `refs/heads/main`), if any.
    pub fn head_branch(&self) -> Option<String> {
        let s = fs::read_to_string(self.dir.join("HEAD")).ok()?;
        s.trim().strip_prefix("ref: ").map(|r| r.trim().to_string())
    }

    /// A value from the git repo's own config (simple `[section] key = value` lookup).
    pub fn config_value(&self, section: &str, key: &str) -> Option<String> {
        super::config::ConfigFile::load(&self.dir.join("config")).get(&format!("{section}.{key}"))
    }
}

fn walk_refs(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Oid>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = format!("{prefix}/{}", e.file_name().to_string_lossy());
        let p = e.path();
        if p.is_dir() {
            walk_refs(&p, &name, out);
        } else if let Ok(s) = fs::read_to_string(&p)
            && let Ok(o) = Oid::from_hex(s.trim())
        {
            out.insert(name, o);
        }
    }
}

// ---------------------------------------------------------------- pack parsing

fn be32(b: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(b.get(at..at + 4).context("truncated pack index")?.try_into()?))
}

fn load_pack(idx_path: &Path, pack_path: &Path) -> Result<Pack> {
    let idx = fs::read(idx_path).with_context(|| format!("reading {}", idx_path.display()))?;
    let data = fs::read(pack_path).with_context(|| format!("reading {}", pack_path.display()))?;
    if data.len() < 12 || &data[..4] != b"PACK" {
        bail!("{} is not a git packfile", pack_path.display());
    }
    let mut offsets = HashMap::new();
    if idx.len() >= 8 && idx[..4] == [0xff, b't', b'O', b'c'] {
        // Index version 2.
        if be32(&idx, 4)? != 2 {
            bail!("unsupported pack index version in {}", idx_path.display());
        }
        let n = be32(&idx, 8 + 255 * 4)? as usize;
        let sha_at = 8 + 256 * 4;
        let crc_at = sha_at + n * 20;
        let off_at = crc_at + n * 4;
        let large_at = off_at + n * 4;
        for i in 0..n {
            let oid = Oid::from_bytes(idx.get(sha_at + i * 20..sha_at + i * 20 + 20).context("truncated index")?)?;
            let raw = be32(&idx, off_at + i * 4)?;
            let off = if raw & 0x8000_0000 != 0 {
                let j = (raw & 0x7fff_ffff) as usize;
                let b = idx.get(large_at + j * 8..large_at + j * 8 + 8).context("truncated index")?;
                u64::from_be_bytes(b.try_into()?) as usize
            } else {
                raw as usize
            };
            offsets.insert(oid, off);
        }
    } else {
        // Index version 1: fanout, then (4-byte offset, 20-byte id) entries.
        let n = be32(&idx, 255 * 4)? as usize;
        for i in 0..n {
            let at = 256 * 4 + i * 24;
            let off = be32(&idx, at)? as usize;
            offsets.insert(Oid::from_bytes(idx.get(at + 4..at + 24).context("truncated index")?)?, off);
        }
    }
    let name = pack_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    Ok(Pack { name, data, offsets })
}

enum Base {
    None(Kind),
    Offset(usize),
    Ref(Oid),
}

struct Entry {
    base: Base,
    /// Full object content, or the delta instructions for deltified objects.
    data: Vec<u8>,
}

fn parse_entry(pack: &[u8], offset: usize) -> Result<Entry> {
    let mut pos = offset;
    let mut c = *pack.get(pos).context("offset past end of pack")?;
    pos += 1;
    let typ = (c >> 4) & 7;
    let mut size = (c & 0x0f) as usize;
    let mut shift = 4;
    while c & 0x80 != 0 {
        c = *pack.get(pos).context("truncated pack")?;
        pos += 1;
        size |= ((c & 0x7f) as usize) << shift;
        shift += 7;
    }
    let base = match typ {
        1 => Base::None(Kind::Commit),
        2 => Base::None(Kind::Tree),
        3 => Base::None(Kind::Blob),
        4 => Base::None(Kind::Tag),
        6 => {
            c = *pack.get(pos).context("truncated pack")?;
            pos += 1;
            let mut rel = (c & 0x7f) as usize;
            while c & 0x80 != 0 {
                c = *pack.get(pos).context("truncated pack")?;
                pos += 1;
                rel = ((rel + 1) << 7) | (c & 0x7f) as usize;
            }
            Base::Offset(offset.checked_sub(rel).context("bad delta base offset")?)
        }
        7 => {
            let oid = Oid::from_bytes(pack.get(pos..pos + 20).context("truncated pack")?)?;
            pos += 20;
            Base::Ref(oid)
        }
        t => bail!("unknown pack object type {t}"),
    };
    let mut data = vec![0u8; size];
    ZlibDecoder::new(pack.get(pos..).context("truncated pack")?).read_exact(&mut data)?;
    Ok(Entry { base, data })
}

fn varint(d: &[u8], pos: &mut usize) -> Result<usize> {
    let mut v = 0usize;
    let mut shift = 0;
    loop {
        let c = *d.get(*pos).context("truncated delta")?;
        *pos += 1;
        v |= ((c & 0x7f) as usize) << shift;
        shift += 7;
        if c & 0x80 == 0 {
            return Ok(v);
        }
    }
}

/// Apply a git delta (copy/insert instructions) to `base`.
pub fn apply_delta(base: &[u8], delta: &[u8]) -> Result<Vec<u8>> {
    let mut i = 0;
    let src = varint(delta, &mut i)?;
    let dst = varint(delta, &mut i)?;
    if src != base.len() {
        bail!("delta base size mismatch");
    }
    let mut out = Vec::with_capacity(dst);
    while i < delta.len() {
        let op = delta[i];
        i += 1;
        if op & 0x80 != 0 {
            let mut off = 0usize;
            let mut size = 0usize;
            for b in 0..4 {
                if op & (1 << b) != 0 {
                    off |= (*delta.get(i).context("truncated delta")? as usize) << (8 * b);
                    i += 1;
                }
            }
            for b in 0..3 {
                if op & (0x10 << b) != 0 {
                    size |= (*delta.get(i).context("truncated delta")? as usize) << (8 * b);
                    i += 1;
                }
            }
            if size == 0 {
                size = 0x10000;
            }
            out.extend_from_slice(base.get(off..off + size).context("delta copy out of range")?);
        } else if op != 0 {
            let n = op as usize;
            out.extend_from_slice(delta.get(i..i + n).context("truncated delta")?);
            i += n;
        } else {
            bail!("invalid delta opcode 0");
        }
    }
    if out.len() != dst {
        bail!("delta result size mismatch");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_copy_and_insert() {
        let base = b"hello world";
        // src=11, dst=11; copy off 0 len 6 ("hello "), insert "there"
        let delta = [11u8, 11, 0x90, 6, 5, b't', b'h', b'e', b'r', b'e'];
        assert_eq!(apply_delta(base, &delta).unwrap(), b"hello there");
    }
}
