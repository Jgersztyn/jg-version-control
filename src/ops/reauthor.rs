//! Fixing the author/committer identity on *your own* commits.
//!
//! Used by `jg commit --amend --reset-author` (last commit) and
//! `jg commit --reset-author --history` (every matching commit, all branches/tags/stashes).
//! Default aliases: `jg fixme` and `jg fixme-all`.
//!
//! "Your" commits are those whose email is one of: your current `user.email`, any
//! email in `user.formerEmails` (comma-separated), or any `--old-email`. Commits by
//! anyone else are never changed. Timestamps and messages are kept as they are.

use super::Message;
use crate::core::object::{Commit, Kind, Signature};
use crate::core::refs::Head;
use crate::core::{Oid, Repo};
use anyhow::{Result, bail};
use serde_json::json;
use std::collections::{HashMap, HashSet};

/// Lower-cased emails that count as "you", plus your current identity.
pub fn my_identity(repo: &Repo, extra: &[String]) -> Result<(Signature, HashSet<String>)> {
    let me = repo.signature()?;
    let mut emails: HashSet<String> = HashSet::new();
    emails.insert(me.email.to_lowercase());
    let former = repo.config.get("user.formerEmails").unwrap_or_default();
    for e in former.split(',').chain(extra.iter().map(String::as_str)) {
        let e = e.trim().to_lowercase();
        if !e.is_empty() {
            emails.insert(e);
        }
    }
    Ok((me, emails))
}

fn not_yours_hint(email: &str) -> String {
    format!(
        "If that was you under an old email, either pass it: --old-email {email}\n\
         or remember it for good: jg config --global user.formerEmails \"{email}\""
    )
}

/// For `--amend --reset-author`: the new author, after checking the last commit is yours.
pub fn amend_author(repo: &Repo, head: &Commit, extra: &[String]) -> Result<Signature> {
    let (me, emails) = my_identity(repo, extra)?;
    if !emails.contains(&head.author.email.to_lowercase()) {
        bail!(
            "the last commit was authored by {} <{}>, which is not one of your emails; not changing it.\n{}",
            head.author.name,
            head.author.email,
            not_yours_hint(&head.author.email)
        );
    }
    Ok(me)
}

/// Replace name/email (keeping the original time) when the signature is yours.
fn fix(sig: &Signature, me: &Signature, emails: &HashSet<String>) -> Option<Signature> {
    if !emails.contains(&sig.email.to_lowercase()) || (sig.name == me.name && sig.email == me.email) {
        return None;
    }
    Some(Signature { name: me.name.clone(), email: me.email.clone(), time: sig.time, tz_offset_min: sig.tz_offset_min })
}

/// All commits reachable from `tips`, parents before children.
fn topo_order(repo: &Repo, tips: &[Oid]) -> Result<Vec<(Oid, Commit)>> {
    let mut order = Vec::new();
    let mut done: HashSet<Oid> = HashSet::new();
    let mut stack: Vec<(Oid, bool)> = tips.iter().map(|t| (*t, false)).collect();
    let mut cache: HashMap<Oid, Commit> = HashMap::new();
    while let Some((oid, expanded)) = stack.pop() {
        if done.contains(&oid) {
            continue;
        }
        if expanded {
            done.insert(oid);
            let c = cache.remove(&oid).expect("cached commit");
            order.push((oid, c));
            continue;
        }
        let c = repo.odb.read_commit(&oid)?;
        stack.push((oid, true));
        for p in &c.parents {
            if !done.contains(p) {
                stack.push((*p, false));
            }
        }
        cache.insert(oid, c);
    }
    Ok(order)
}

/// `jg commit --reset-author --history`
pub fn rewrite_history(repo: &Repo, extra: &[String], dry_run: bool) -> Result<Message> {
    if let Some(op) = super::merge::in_progress(repo) {
        bail!("a {op} is in progress; finish or abort it first");
    }
    let (me, emails) = my_identity(repo, extra)?;

    // Every local starting point: branches, tags, stashes, detached HEAD.
    let branches = repo.refs.list("refs/heads/")?;
    let tags = repo.refs.list("refs/tags/")?;
    let stashes = super::stash::load_stack(repo)?;
    let head = repo.head()?;
    let mut tips: Vec<Oid> = branches.iter().map(|(_, o)| *o).collect();
    tips.extend(stashes.iter().copied());
    for (_, o) in &tags {
        if let Ok(c) = crate::core::revparse::peel_to_commit(repo, *o) {
            tips.push(c);
        }
    }
    if let Head::Detached(o) = &head {
        tips.push(*o);
    }

    // Rewrite commits oldest-first so parents are mapped before children.
    let mut map: HashMap<Oid, Oid> = HashMap::new();
    let mut changed = 0usize;
    for (oid, c) in topo_order(repo, &tips)? {
        let parents: Vec<Oid> = c.parents.iter().map(|p| *map.get(p).unwrap_or(p)).collect();
        let author = fix(&c.author, &me, &emails);
        let committer = fix(&c.committer, &me, &emails);
        if author.is_none() && committer.is_none() && parents == c.parents {
            map.insert(oid, oid);
            continue;
        }
        if author.is_some() || committer.is_some() {
            changed += 1;
        }
        let mut n = c.clone();
        n.parents = parents;
        n.author = author.unwrap_or(c.author);
        n.committer = committer.unwrap_or(c.committer);
        // Old signatures would no longer verify.
        n.extra_headers.retain(|(k, _)| !k.starts_with("gpgsig"));
        let new = if dry_run { Oid::hash_object("commit", &n.serialize()) } else { repo.odb.write_commit(&n)? };
        map.insert(oid, new);
    }
    let remap = |o: &Oid| *map.get(o).unwrap_or(o);

    // Plan ref updates.
    let mut updates: Vec<(String, Oid, Oid)> = Vec::new();
    for (name, old) in &branches {
        if remap(old) != *old {
            updates.push((name.clone(), *old, remap(old)));
        }
    }
    for (name, old) in &tags {
        let (kind, _) = repo.odb.read(old)?;
        if kind != Kind::Tag {
            if remap(old) != *old {
                updates.push((name.clone(), *old, remap(old)));
            }
            continue;
        }
        let mut t = repo.odb.read_tag(old)?;
        let new_target = remap(&t.object);
        let new_tagger = t.tagger.as_ref().and_then(|s| fix(s, &me, &emails));
        if new_target == t.object && new_tagger.is_none() {
            continue;
        }
        t.object = new_target;
        if new_tagger.is_some() {
            t.tagger = new_tagger;
            changed += 1;
        }
        let new = if dry_run {
            Oid::hash_object("tag", &t.serialize())
        } else {
            repo.odb.write(Kind::Tag, &t.serialize())?
        };
        updates.push((name.clone(), *old, new));
    }
    let new_stashes: Vec<Oid> = stashes.iter().map(remap).collect();
    let stash_changed = new_stashes != stashes;

    let mut lines = Vec::new();
    let verb = if dry_run { "Would update" } else { "Updated" };
    for (name, old, new) in &updates {
        lines.push(format!("  {}: {} -> {}", name.trim_start_matches("refs/"), old.short(), new.short()));
    }

    if !dry_run && (!updates.is_empty() || stash_changed) {
        let _ = std::fs::write(
            repo.state_file("ORIG_HEAD"),
            format!("{}\n", repo.head_oid()?.map(|o| o.to_hex()).unwrap_or_default()),
        );
        let who = repo.who();
        for (name, _, new) in &updates {
            repo.refs.write(name, new, &who, "rewrite author (your commits)")?;
        }
        if let Head::Detached(o) = &head
            && remap(o) != *o
        {
            repo.refs.update_head(&remap(o), &who, "rewrite author (your commits)")?;
        }
        if stash_changed {
            let s: String = new_stashes.iter().map(|o| format!("{o}\n")).collect();
            crate::util::atomic_write(&repo.dir.join("stash"), s.as_bytes())?;
        }
    }

    let mut text = if changed == 0 {
        format!(
            "Nothing to change: no commits by {} need a new name/email.",
            emails.iter().cloned().collect::<Vec<_>>().join(", ")
        )
    } else {
        format!("{verb} {changed} commit(s)/tag(s) to {} <{}>:\n{}", me.name, me.email, lines.join("\n"))
    };
    if changed > 0 {
        if stash_changed {
            text.push_str("\n  (stashes updated too)");
        }
        if dry_run {
            text.push_str("\n(dry run: nothing was changed)");
        } else {
            text.push_str(
                "\nCommit ids changed. Old commits stay reachable via 'jg reflog'.\n\
                 If you already pushed or exported these branches: 'jg push --force' / 'jg git-export --force'.",
            );
        }
    }
    let refs_json: Vec<_> = updates.iter().map(|(n, o, nw)| json!({ "ref": n, "old": o, "new": nw })).collect();
    Ok(Message::new(
        text,
        json!({
            "dry_run": dry_run,
            "rewritten": changed,
            "author": { "name": me.name, "email": me.email },
            "matched_emails": emails,
            "refs": refs_json,
            "stashes_updated": stash_changed,
        }),
    ))
}
