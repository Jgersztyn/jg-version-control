//! Small helpers shared across the crate: hex, time, atomic file writes, paths.

use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0xf) as usize] as char);
    }
    s
}

/// SHA-256 of a byte slice as lowercase hex (used for release verification).
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex(Sha256::digest(data).as_slice())
}

/// Current time as (unix seconds, timezone offset in minutes east of UTC).
pub fn now_with_offset() -> (i64, i32) {
    let now = chrono::Local::now();
    (now.timestamp(), now.offset().local_minus_utc() / 60)
}

/// Format a timezone offset in git's `+HHMM` / `-HHMM` form.
pub fn format_tz(offset_min: i32) -> String {
    let sign = if offset_min < 0 { '-' } else { '+' };
    let m = offset_min.abs();
    format!("{}{:02}{:02}", sign, m / 60, m % 60)
}

/// Parse git's `+HHMM` form back into minutes.
pub fn parse_tz(s: &str) -> i32 {
    if s.len() != 5 {
        return 0;
    }
    let sign = if s.starts_with('-') { -1 } else { 1 };
    let h: i32 = s[1..3].parse().unwrap_or(0);
    let m: i32 = s[3..5].parse().unwrap_or(0);
    sign * (h * 60 + m)
}

/// Human-readable date like `Sun Nov 15 10:04:00 2026 -0500`.
pub fn format_date(secs: i64, offset_min: i32) -> String {
    use chrono::{FixedOffset, TimeZone};
    let tz = FixedOffset::east_opt(offset_min * 60).unwrap_or(FixedOffset::east_opt(0).unwrap());
    match tz.timestamp_opt(secs, 0).single() {
        Some(dt) => dt.format("%a %b %e %H:%M:%S %Y %z").to_string(),
        None => secs.to_string(),
    }
}

/// ISO-8601 date for JSON output.
pub fn iso_date(secs: i64, offset_min: i32) -> String {
    use chrono::{FixedOffset, TimeZone};
    let tz = FixedOffset::east_opt(offset_min * 60).unwrap_or(FixedOffset::east_opt(0).unwrap());
    match tz.timestamp_opt(secs, 0).single() {
        Some(dt) => dt.to_rfc3339(),
        None => secs.to_string(),
    }
}

/// Write a file atomically: write to a temp file next to it, then rename.
/// Readers never observe a half-written file.
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    fs::create_dir_all(dir)?;
    let tmp =
        dir.join(format!(".{}.tmp{}", path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    {
        let mut f = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(data)?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Convert a repo-relative path to the canonical `/`-separated form stored in the index.
pub fn to_repo_path(rel: &Path) -> String {
    let mut parts = Vec::new();
    for c in rel.components() {
        if let std::path::Component::Normal(p) = c {
            parts.push(p.to_string_lossy().into_owned());
        }
    }
    parts.join("/")
}

/// Convert a `/`-separated repo path to a native path under `root`.
pub fn from_repo_path(root: &Path, p: &str) -> PathBuf {
    let mut out = root.to_path_buf();
    for part in p.split('/') {
        out.push(part);
    }
    out
}

/// Lexically normalize a path (resolve `.` and `..`) without touching the filesystem.
pub fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True if `data` looks binary (contains a NUL byte in the first 8000 bytes, like git).
pub fn is_binary(data: &[u8]) -> bool {
    data.iter().take(8000).any(|&b| b == 0)
}

/// Remove empty parent directories of `path` up to (not including) `stop`.
pub fn prune_empty_dirs(path: &Path, stop: &Path) {
    let mut cur = path.parent();
    while let Some(dir) = cur {
        if dir == stop || !dir.starts_with(stop) {
            break;
        }
        if fs::remove_dir(dir).is_err() {
            break;
        }
        cur = dir.parent();
    }
}

/// Add `.jg/` to a git repository's `info/exclude` so git ignores jg's metadata.
/// Returns true if the file was changed.
pub fn exclude_jg_from_git(git_dir: &Path) -> Result<bool> {
    let ex = git_dir.join("info").join("exclude");
    let cur = fs::read_to_string(&ex).unwrap_or_default();
    if cur.lines().any(|l| l.trim() == ".jg/" || l.trim() == ".jg" || l.trim() == "/.jg/") {
        return Ok(false);
    }
    fs::create_dir_all(git_dir.join("info"))?;
    let sep = if cur.is_empty() || cur.ends_with('\n') { "" } else { "\n" };
    fs::write(&ex, format!("{cur}{sep}.jg/\n"))?;
    Ok(true)
}
