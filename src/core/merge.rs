//! Three-way merge of file contents (diff3-style) and of whole trees.
//! Used by `merge`, `pull`, `cherry-pick`, `revert` and `stash apply`.

use super::diff::split_lines;
use super::index::{Conflict, Side};
use super::object::MODE_FILE;
use super::odb::Odb;
use super::tree::FlatTree;
use anyhow::Result;
use similar::{Algorithm, DiffTag, capture_diff_slices};

/// A change to a range of base lines: replace `base[start..end]` with `lines`.
#[derive(Debug, Clone)]
struct Edit<'a> {
    start: usize,
    end: usize,
    lines: Vec<&'a [u8]>,
    side: u8, // 0 = ours, 1 = theirs
}

fn edits<'a>(base: &[&'a [u8]], other: &[&'a [u8]], side: u8) -> Vec<Edit<'a>> {
    let ops = capture_diff_slices(Algorithm::Myers, base, other);
    let mut out: Vec<Edit<'a>> = Vec::new();
    for op in ops {
        let (tag, or, nr) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        let lines: Vec<&[u8]> = other[nr].to_vec();
        // Coalesce adjacent ops (e.g. Delete followed by Insert at the same spot).
        if let Some(last) = out.last_mut()
            && last.end == or.start
        {
            last.end = or.end;
            last.lines.extend(lines);
            continue;
        }
        out.push(Edit { start: or.start, end: or.end, lines, side });
    }
    out
}

pub struct TextMerge {
    pub content: Vec<u8>,
    pub conflicts: usize,
}

/// Merge `ours` and `theirs`, both derived from `base`. Conflicting regions are
/// wrapped in `<<<<<<<` / `=======` / `>>>>>>>` markers.
pub fn merge_text(base: &[u8], ours: &[u8], theirs: &[u8], ours_label: &str, theirs_label: &str) -> TextMerge {
    let b = split_lines(base);
    let o = split_lines(ours);
    let t = split_lines(theirs);
    let mut all = edits(&b, &o, 0);
    all.extend(edits(&b, &t, 1));
    all.sort_by_key(|e| (e.start, e.end));

    let mut out: Vec<u8> = Vec::new();
    let mut conflicts = 0;
    let mut pos = 0; // next base line to copy
    let mut i = 0;
    while i < all.len() {
        // Group edits whose base ranges overlap or touch.
        let mut group = vec![all[i].clone()];
        let mut gend = all[i].end;
        let gstart = all[i].start;
        i += 1;
        while i < all.len() && all[i].start <= gend {
            // Two pure insertions at different points never overlap; touching non-empty ranges do.
            gend = gend.max(all[i].end);
            group.push(all[i].clone());
            i += 1;
        }
        for l in &b[pos..gstart] {
            out.extend_from_slice(l);
        }
        pos = gend;
        let has_ours = group.iter().any(|e| e.side == 0);
        let has_theirs = group.iter().any(|e| e.side == 1);
        let apply = |side: u8| -> Vec<u8> {
            let mut v = Vec::new();
            let mut p = gstart;
            for e in group.iter().filter(|e| e.side == side) {
                for l in &b[p..e.start] {
                    v.extend_from_slice(l);
                }
                for l in &e.lines {
                    v.extend_from_slice(l);
                }
                p = e.end;
            }
            for l in &b[p..gend] {
                v.extend_from_slice(l);
            }
            v
        };
        if has_ours && has_theirs {
            let ov = apply(0);
            let tv = apply(1);
            if ov == tv {
                out.extend_from_slice(&ov);
            } else {
                conflicts += 1;
                out.extend_from_slice(format!("<<<<<<< {ours_label}\n").as_bytes());
                push_block(&mut out, &ov);
                out.extend_from_slice(b"=======\n");
                push_block(&mut out, &tv);
                out.extend_from_slice(format!(">>>>>>> {theirs_label}\n").as_bytes());
            }
        } else {
            out.extend_from_slice(&apply(if has_ours { 0 } else { 1 }));
        }
    }
    for l in &b[pos.min(b.len())..] {
        out.extend_from_slice(l);
    }
    TextMerge { content: out, conflicts }
}

fn push_block(out: &mut Vec<u8>, block: &[u8]) {
    out.extend_from_slice(block);
    if !block.is_empty() && !block.ends_with(b"\n") {
        out.push(b'\n');
    }
}

pub struct TreeMerge {
    /// The merged tree (conflicted text files contain conflict markers).
    pub merged: FlatTree,
    /// Paths that need manual resolution.
    pub conflicts: Vec<(String, Conflict)>,
}

/// Three-way merge of flattened trees. Writes merged blobs into `odb`.
pub fn merge_trees(
    odb: &Odb,
    base: &FlatTree,
    ours: &FlatTree,
    theirs: &FlatTree,
    ours_label: &str,
    theirs_label: &str,
) -> Result<TreeMerge> {
    let mut merged = FlatTree::new();
    let mut conflicts = Vec::new();
    let mut paths: Vec<&String> = base.keys().chain(ours.keys()).chain(theirs.keys()).collect();
    paths.sort();
    paths.dedup();
    for p in paths {
        let (b, o, t) = (base.get(p), ours.get(p), theirs.get(p));
        if o == t {
            if let Some(v) = o {
                merged.insert(p.clone(), *v);
            }
            continue;
        }
        if o == b {
            if let Some(v) = t {
                merged.insert(p.clone(), *v);
            }
            continue;
        }
        if t == b {
            if let Some(v) = o {
                merged.insert(p.clone(), *v);
            }
            continue;
        }
        let side = |x: Option<&(u32, super::oid::Oid)>| x.map(|(m, o)| Side { mode: *m, oid: *o });
        let conflict = Conflict { base: side(b), ours: side(o), theirs: side(t) };
        match (o, t) {
            (Some((om, oo)), Some((tm, to))) => {
                // Both sides changed the file: try a content merge.
                let base_data = match b {
                    Some((_, bo)) => odb.read_blob(bo)?,
                    None => Vec::new(),
                };
                let od = odb.read_blob(oo)?;
                let td = odb.read_blob(to)?;
                let mode = if om == tm {
                    *om
                } else if b.is_some_and(|(bm, _)| bm == om) {
                    *tm
                } else {
                    *om
                };
                if crate::util::is_binary(&od) || crate::util::is_binary(&td) || crate::util::is_binary(&base_data) {
                    merged.insert(p.clone(), (*om, *oo));
                    conflicts.push((p.clone(), conflict));
                    continue;
                }
                let m = merge_text(&base_data, &od, &td, ours_label, theirs_label);
                let oid = odb.write_blob(&m.content)?;
                merged.insert(p.clone(), (if mode == 0 { MODE_FILE } else { mode }, oid));
                if m.conflicts > 0 {
                    conflicts.push((p.clone(), conflict));
                }
            }
            // Modified on one side, deleted on the other: keep the modified version, flag it.
            (Some(v), None) | (None, Some(v)) => {
                merged.insert(p.clone(), *v);
                conflicts.push((p.clone(), conflict));
            }
            (None, None) => {}
        }
    }
    Ok(TreeMerge { merged, conflicts })
}

/// Find a path that is both a file and a directory prefix in the map (unmergeable).
pub fn file_dir_clash(map: &FlatTree) -> Option<String> {
    for p in map.keys() {
        let prefix = format!("{p}/");
        if map.range(prefix.clone()..).next().is_some_and(|(k, _)| k.starts_with(&prefix)) {
            return Some(p.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(b: &str, o: &str, t: &str) -> (String, usize) {
        let r = merge_text(b.as_bytes(), o.as_bytes(), t.as_bytes(), "ours", "theirs");
        (String::from_utf8(r.content).unwrap(), r.conflicts)
    }

    #[test]
    fn clean_non_overlapping() {
        let (s, c) = m("a\nb\nc\nd\ne\n", "A\nb\nc\nd\ne\n", "a\nb\nc\nd\nE\n");
        assert_eq!(c, 0);
        assert_eq!(s, "A\nb\nc\nd\nE\n");
    }

    #[test]
    fn identical_changes() {
        let (s, c) = m("a\nb\n", "a\nX\n", "a\nX\n");
        assert_eq!((s.as_str(), c), ("a\nX\n", 0));
    }

    #[test]
    fn conflict_markers() {
        let (s, c) = m("a\nb\nc\n", "a\nO\nc\n", "a\nT\nc\n");
        assert_eq!(c, 1);
        assert_eq!(s, "a\n<<<<<<< ours\nO\n=======\nT\n>>>>>>> theirs\nc\n");
    }

    #[test]
    fn insert_at_end_both_sides() {
        let (s, c) = m("a\n", "a\no\n", "a\nt\n");
        assert_eq!(c, 1);
        assert!(s.contains("<<<<<<<"));
    }
}
