//! Converting between nested git trees and flat `path -> (mode, oid)` maps.
//! Most algorithms (status, diff, merge, checkout) work on flat maps.

use super::object::{MODE_TREE, Tree, TreeEntry};
use super::odb::Odb;
use super::oid::Oid;
use anyhow::Result;
use std::collections::BTreeMap;

pub type FlatTree = BTreeMap<String, (u32, Oid)>;

/// Recursively flatten a tree into `"dir/file" -> (mode, oid)` (files only).
pub fn flatten(odb: &Odb, tree: &Oid) -> Result<FlatTree> {
    let mut out = BTreeMap::new();
    flatten_into(odb, tree, "", &mut out)?;
    Ok(out)
}

fn flatten_into(odb: &Odb, tree: &Oid, prefix: &str, out: &mut FlatTree) -> Result<()> {
    for e in odb.read_tree(tree)?.entries {
        let path = if prefix.is_empty() { e.name.clone() } else { format!("{prefix}/{}", e.name) };
        if e.is_tree() {
            flatten_into(odb, &e.oid, &path, out)?;
        } else {
            out.insert(path, (e.mode, e.oid));
        }
    }
    Ok(())
}

/// Build (and store) nested trees from a flat map. Returns the root tree id.
pub fn build(odb: &Odb, files: &FlatTree) -> Result<Oid> {
    // Group into a recursive structure first, then write bottom-up.
    #[derive(Default)]
    struct Dir {
        files: Vec<TreeEntry>,
        dirs: BTreeMap<String, Dir>,
    }
    let mut root = Dir::default();
    for (path, (mode, oid)) in files {
        let mut parts: Vec<&str> = path.split('/').collect();
        let name = parts.pop().unwrap();
        let mut d = &mut root;
        for p in parts {
            d = d.dirs.entry(p.to_string()).or_default();
        }
        d.files.push(TreeEntry { mode: *mode, name: name.to_string(), oid: *oid });
    }
    fn write(odb: &Odb, d: Dir) -> Result<Oid> {
        let mut t = Tree { entries: d.files };
        for (name, sub) in d.dirs {
            let oid = write(odb, sub)?;
            t.entries.push(TreeEntry { mode: MODE_TREE, name, oid });
        }
        t.sort();
        odb.write_tree(&t)
    }
    write(odb, root)
}

/// The id of the empty tree (same as git's well-known 4b825dc...).
pub fn empty_tree(odb: &Odb) -> Result<Oid> {
    odb.write_tree(&Tree::default())
}
