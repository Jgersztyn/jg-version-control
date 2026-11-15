//! Revision parsing: `HEAD`, `@`, branch/tag/remote names, (abbreviated) ids,
//! and suffixes `~N`, `^`, `^N` (e.g. `main~2`, `HEAD^2`, `a1b2c3d^`).

use super::object::Kind;
use super::oid::Oid;
use super::repo::Repo;
use anyhow::{Context, Result, bail};

/// Resolve a revision to a commit id.
pub fn resolve_commit(repo: &Repo, spec: &str) -> Result<Oid> {
    let (base, suffix) = split_suffix(spec);
    let mut oid = resolve_base(repo, base)?;
    oid = peel_to_commit(repo, oid)?;
    let mut rest = suffix;
    while !rest.is_empty() {
        let op = rest.as_bytes()[0];
        rest = &rest[1..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        rest = &rest[digits.len()..];
        let n: usize = if digits.is_empty() { 1 } else { digits.parse()? };
        match op {
            b'~' => {
                for _ in 0..n {
                    let c = repo.odb.read_commit(&oid)?;
                    oid = *c.parents.first().with_context(|| format!("'{spec}': commit has no parent"))?;
                }
            }
            b'^' => {
                if n == 0 {
                    continue;
                }
                let c = repo.odb.read_commit(&oid)?;
                oid = *c.parents.get(n - 1).with_context(|| format!("'{spec}': commit has no parent #{n}"))?;
            }
            _ => bail!("bad revision '{spec}'"),
        }
    }
    Ok(oid)
}

fn split_suffix(spec: &str) -> (&str, &str) {
    match spec.find(['~', '^']) {
        Some(i) => (&spec[..i], &spec[i..]),
        None => (spec, ""),
    }
}

/// Resolve a name or id (no suffixes) to an object id (not peeled).
pub fn resolve_base(repo: &Repo, name: &str) -> Result<Oid> {
    if name == "HEAD" || name == "@" || name.is_empty() {
        return repo.head_oid()?.context("HEAD does not point to a commit yet (no commits on this branch)");
    }
    let candidates = [
        name.to_string(),
        format!("refs/{name}"),
        format!("refs/heads/{name}"),
        format!("refs/tags/{name}"),
        format!("refs/remotes/{name}"),
        format!("refs/remotes/{name}/HEAD"),
    ];
    for c in &candidates {
        if c.starts_with("refs/")
            && let Some(o) = repo.refs.read(c)?
        {
            return Ok(o);
        }
    }
    if name.len() == 40
        && let Ok(o) = Oid::from_hex(name)
        && repo.odb.exists(&o)
    {
        return Ok(o);
    }
    let matches = repo.odb.find_prefix(name)?;
    match matches.len() {
        1 => Ok(matches[0]),
        0 => bail!("unknown revision '{name}'"),
        _ => bail!("ambiguous revision '{name}' (matches {} objects)", matches.len()),
    }
}

/// Follow annotated tags until reaching a commit.
pub fn peel_to_commit(repo: &Repo, mut oid: Oid) -> Result<Oid> {
    for _ in 0..16 {
        let (kind, _) = repo.odb.read(&oid)?;
        match kind {
            Kind::Commit => return Ok(oid),
            Kind::Tag => oid = repo.odb.read_tag(&oid)?.object,
            _ => bail!("{oid} is a {}, not a commit", kind.as_str()),
        }
    }
    bail!("tag chain too deep")
}
