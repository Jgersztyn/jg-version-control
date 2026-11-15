//! Commit graph algorithms: history walks, ancestry checks, merge bases,
//! and "which objects are reachable" (used by sync and git export).

use super::object::{Commit, Kind};
use super::odb::Odb;
use super::oid::Oid;
use anyhow::Result;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

/// Walk commits reachable from `starts`, newest (by committer time) first.
/// Stops early when `limit` commits have been yielded.
pub fn walk(odb: &Odb, starts: &[Oid], limit: Option<usize>) -> Result<Vec<(Oid, Commit)>> {
    let mut seen = HashSet::new();
    let mut pending: HashMap<Oid, Commit> = HashMap::new();
    let mut heap: BinaryHeap<(i64, Oid)> = BinaryHeap::new();
    for s in starts {
        if seen.insert(*s) {
            let c = odb.read_commit(s)?;
            heap.push((c.committer.time, *s));
            pending.insert(*s, c);
        }
    }
    let mut out = Vec::new();
    while let Some((_, oid)) = heap.pop() {
        if limit.is_some_and(|l| out.len() >= l) {
            break;
        }
        let c = pending.remove(&oid).expect("queued commit");
        for p in &c.parents {
            if seen.insert(*p) {
                let pc = odb.read_commit(p)?;
                heap.push((pc.committer.time, *p));
                pending.insert(*p, pc);
            }
        }
        out.push((oid, c));
    }
    Ok(out)
}

/// All commit ids reachable from `starts` (inclusive).
pub fn ancestors(odb: &Odb, starts: &[Oid]) -> Result<HashSet<Oid>> {
    let mut seen: HashSet<Oid> = HashSet::new();
    let mut queue: VecDeque<Oid> = starts.iter().copied().collect();
    while let Some(o) = queue.pop_front() {
        if !seen.insert(o) {
            continue;
        }
        for p in odb.read_commit(&o)?.parents {
            if !seen.contains(&p) {
                queue.push_back(p);
            }
        }
    }
    Ok(seen)
}

/// Is `ancestor` reachable from `descendant` (or equal to it)?
pub fn is_ancestor(odb: &Odb, ancestor: &Oid, descendant: &Oid) -> Result<bool> {
    if ancestor == descendant {
        return Ok(true);
    }
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([*descendant]);
    while let Some(o) = queue.pop_front() {
        if o == *ancestor {
            return Ok(true);
        }
        if !seen.insert(o) {
            continue;
        }
        queue.extend(odb.read_commit(&o)?.parents);
    }
    Ok(false)
}

/// Best common ancestor of two commits (None if histories are unrelated).
/// When there are several (criss-cross merges) the most recent one is chosen.
pub fn merge_base(odb: &Odb, a: &Oid, b: &Oid) -> Result<Option<Oid>> {
    let anc_a = ancestors(odb, &[*a])?;
    let anc_b = ancestors(odb, &[*b])?;
    let common: Vec<Oid> = anc_a.intersection(&anc_b).copied().collect();
    if common.is_empty() {
        return Ok(None);
    }
    // Remove any common ancestor that is itself an ancestor of another one.
    let mut redundant: HashSet<Oid> = HashSet::new();
    for c in &common {
        if redundant.contains(c) {
            continue;
        }
        let mut queue: VecDeque<Oid> = odb.read_commit(c)?.parents.into();
        while let Some(o) = queue.pop_front() {
            if !redundant.insert(o) {
                continue;
            }
            queue.extend(odb.read_commit(&o)?.parents);
        }
    }
    let mut best: Vec<(i64, Oid)> = common
        .into_iter()
        .filter(|c| !redundant.contains(c))
        .map(|c| odb.read_commit(&c).map(|cm| (cm.committer.time, c)))
        .collect::<Result<_>>()?;
    best.sort();
    Ok(best.last().map(|(_, o)| *o))
}

/// Every object id (commits, trees, blobs, tags) reachable from `wants`
/// but not from `haves`. Used to decide what to transfer on push/fetch/export.
pub fn objects_to_send(odb: &Odb, wants: &[Oid], haves: &[Oid]) -> Result<Vec<Oid>> {
    // Everything reachable from `haves` (that we actually have locally) is excluded.
    let have_known: Vec<Oid> = haves.iter().copied().filter(|h| odb.exists(h)).collect();
    let mut excluded: HashSet<Oid> = HashSet::new();
    let mut ex_commits = Vec::new();
    for h in &have_known {
        collect_tag_chain(odb, *h, &mut excluded, &mut ex_commits)?;
    }
    for c in ancestors(odb, &ex_commits)? {
        excluded.insert(c);
        let tree = odb.read_commit(&c)?.tree;
        mark_tree(odb, &tree, &mut excluded, &mut None)?;
    }

    let mut out_set: HashSet<Oid> = HashSet::new();
    let mut out = Vec::new();
    let mut commits = Vec::new();
    for w in wants {
        let mut tag_objs = HashSet::new();
        collect_tag_chain(odb, *w, &mut tag_objs, &mut commits)?;
        for t in tag_objs {
            if !excluded.contains(&t) && out_set.insert(t) {
                out.push(t);
            }
        }
    }
    // Walk commits not in the excluded set.
    let mut queue: VecDeque<Oid> = commits.into();
    while let Some(c) = queue.pop_front() {
        if excluded.contains(&c) || !out_set.insert(c) {
            continue;
        }
        out.push(c);
        let commit = odb.read_commit(&c)?;
        let mut sink = Some((&mut out_set, &mut out));
        mark_tree(odb, &commit.tree, &mut excluded, &mut sink)?;
        queue.extend(commit.parents);
    }
    Ok(out)
}

/// Follow tag objects to the commit at the end. Tag ids go into `tags`; the commit into `commits`.
fn collect_tag_chain(odb: &Odb, mut oid: Oid, tags: &mut HashSet<Oid>, commits: &mut Vec<Oid>) -> Result<()> {
    for _ in 0..16 {
        let (kind, data) = odb.read(&oid)?;
        match kind {
            Kind::Tag => {
                tags.insert(oid);
                oid = super::object::Tag::parse(&data)?.object;
            }
            Kind::Commit => {
                commits.push(oid);
                return Ok(());
            }
            _ => {
                tags.insert(oid);
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Recursively visit a tree. With `sink == None`, ids are added to `seen` (exclusion).
/// With a sink, unseen ids are appended to the output list.
#[allow(clippy::type_complexity)]
fn mark_tree(
    odb: &Odb,
    tree: &Oid,
    seen: &mut HashSet<Oid>,
    sink: &mut Option<(&mut HashSet<Oid>, &mut Vec<Oid>)>,
) -> Result<()> {
    match sink {
        None => {
            if !seen.insert(*tree) {
                return Ok(());
            }
        }
        Some((out_set, out)) => {
            if seen.contains(tree) || !out_set.insert(*tree) {
                return Ok(());
            }
            out.push(*tree);
        }
    }
    for e in odb.read_tree(tree)?.entries {
        if e.is_tree() {
            mark_tree(odb, &e.oid, seen, sink)?;
        } else if e.mode == super::object::MODE_GITLINK {
            continue; // submodule commit: not stored here
        } else {
            match sink {
                None => {
                    seen.insert(e.oid);
                }
                Some((out_set, out)) => {
                    if !seen.contains(&e.oid) && out_set.insert(e.oid) {
                        out.push(e.oid);
                    }
                }
            }
        }
    }
    Ok(())
}
