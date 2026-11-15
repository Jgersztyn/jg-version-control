//! Line diffs between blobs, producing structured hunks (for JSON) that can
//! also be rendered as a unified diff (for humans and `git apply`).

use super::oid::Oid;
use serde::Serialize;
use similar::{Algorithm, DiffTag, capture_diff_slices, group_diff_ops};

#[derive(Clone, Debug, Serialize)]
pub struct Line {
    /// ' ' context, '+' added, '-' removed.
    pub tag: char,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Hunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileDiff {
    pub path: String,
    pub status: super::worktree::ChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_oid: Option<Oid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_oid: Option<Oid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_mode: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_mode: Option<u32>,
    pub binary: bool,
    pub additions: usize,
    pub deletions: usize,
    pub hunks: Vec<Hunk>,
}

/// Split into lines, keeping the trailing newline on each line.
pub fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    data.split_inclusive(|&b| b == b'\n').collect()
}

/// Diff two byte buffers into hunks with `context` lines of context.
pub fn diff_lines(old: &[u8], new: &[u8], context: usize) -> (Vec<Hunk>, usize, usize) {
    let a = split_lines(old);
    let b = split_lines(new);
    let ops = capture_diff_slices(Algorithm::Myers, &a, &b);
    let mut hunks = Vec::new();
    let (mut adds, mut dels) = (0, 0);
    for group in group_diff_ops(ops, context) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else { continue };
        let old_start = first.old_range().start;
        let new_start = first.new_range().start;
        let old_end = last.old_range().end;
        let new_end = last.new_range().end;
        let mut lines = Vec::new();
        for op in &group {
            let (tag, or, nr) = op.as_tag_tuple();
            match tag {
                DiffTag::Equal => {
                    for l in &a[or] {
                        lines.push(Line { tag: ' ', text: text_of(l) });
                    }
                }
                DiffTag::Delete | DiffTag::Insert | DiffTag::Replace => {
                    for l in &a[or] {
                        lines.push(Line { tag: '-', text: text_of(l) });
                        dels += 1;
                    }
                    for l in &b[nr] {
                        lines.push(Line { tag: '+', text: text_of(l) });
                        adds += 1;
                    }
                }
            }
        }
        hunks.push(Hunk {
            old_start: if old_end > old_start { old_start + 1 } else { old_start },
            old_lines: old_end - old_start,
            new_start: if new_end > new_start { new_start + 1 } else { new_start },
            new_lines: new_end - new_start,
            lines,
        });
    }
    (hunks, adds, dels)
}

fn text_of(l: &[u8]) -> String {
    String::from_utf8_lossy(l).into_owned()
}

/// Render one file diff as a git-style unified diff.
pub fn render_unified(fd: &FileDiff, color: bool) -> String {
    let (bold, red, green, cyan, reset) =
        if color { ("\x1b[1m", "\x1b[31m", "\x1b[32m", "\x1b[36m", "\x1b[0m") } else { ("", "", "", "", "") };
    let mut s = String::new();
    s.push_str(&format!("{bold}diff --git a/{p} b/{p}{reset}\n", p = fd.path));
    use super::worktree::ChangeKind;
    match fd.status {
        ChangeKind::Added => s.push_str(&format!("{bold}new file mode {:o}{reset}\n", fd.new_mode.unwrap_or(0))),
        ChangeKind::Deleted => s.push_str(&format!("{bold}deleted file mode {:o}{reset}\n", fd.old_mode.unwrap_or(0))),
        _ => {
            if fd.old_mode != fd.new_mode {
                s.push_str(&format!(
                    "{bold}old mode {:o}\nnew mode {:o}{reset}\n",
                    fd.old_mode.unwrap_or(0),
                    fd.new_mode.unwrap_or(0)
                ));
            }
        }
    }
    if fd.binary {
        s.push_str(&format!("Binary files differ ({})\n", fd.path));
        return s;
    }
    if fd.hunks.is_empty() {
        return s;
    }
    let a = if fd.status == ChangeKind::Added { "/dev/null".to_string() } else { format!("a/{}", fd.path) };
    let b = if fd.status == ChangeKind::Deleted { "/dev/null".to_string() } else { format!("b/{}", fd.path) };
    s.push_str(&format!("{bold}--- {a}\n+++ {b}{reset}\n"));
    for h in &fd.hunks {
        s.push_str(&format!("{cyan}@@ -{},{} +{},{} @@{reset}\n", h.old_start, h.old_lines, h.new_start, h.new_lines));
        for l in &h.lines {
            let (c, r) = match l.tag {
                '+' => (green, reset),
                '-' => (red, reset),
                _ => ("", ""),
            };
            let text = l.text.strip_suffix('\n').unwrap_or(&l.text);
            s.push_str(&format!("{c}{}{text}{r}\n", l.tag));
            if !l.text.ends_with('\n') {
                s.push_str("\\ No newline at end of file\n");
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_hunk() {
        let (h, a, d) = diff_lines(b"a\nb\nc\n", b"a\nB\nc\n", 3);
        assert_eq!((a, d), (1, 1));
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].old_start, 1);
        assert_eq!(h[0].lines.len(), 4);
    }
}
