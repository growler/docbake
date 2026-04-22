use std::path::Path;

use similar::udiff::UnifiedHunkHeader;
use similar::{ChangeTag, TextDiff};

const DIFF_CONTEXT_RADIUS: usize = 3;

pub(crate) fn unified_file_diff(
    path: &Path,
    original: &str,
    formatted: &str,
    ansi_inline: bool,
) -> String {
    let diff = TextDiff::from_lines(original, formatted);

    if ansi_inline {
        return colorized_unified_diff(path, &diff);
    }

    diff.unified_diff()
        .context_radius(DIFF_CONTEXT_RADIUS)
        .header(
            &format!("a/{}", path.display()),
            &format!("b/{}", path.display()),
        )
        .to_string()
}

fn colorized_unified_diff(path: &Path, diff: &TextDiff<'_, '_, str>) -> String {
    let grouped_ops = diff.grouped_ops(DIFF_CONTEXT_RADIUS);
    if grouped_ops.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str(&format!("--- a/{}\n", path.display()));
    out.push_str(&format!("+++ b/{}\n", path.display()));

    for ops in grouped_ops {
        if ops.is_empty() {
            continue;
        }
        let hunk_header = UnifiedHunkHeader::new(&ops);
        out.push_str(&format!("{hunk_header}\n"));

        for op in &ops {
            for change in diff.iter_inline_changes(op) {
                let tag = change.tag();
                let prefix = match tag {
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                    ChangeTag::Equal => ' ',
                };
                out.push(prefix);

                for (emphasized, value) in change.iter_strings_lossy() {
                    if matches!(tag, ChangeTag::Equal) {
                        out.push_str(value.as_ref());
                        continue;
                    }

                    let color = match (tag, emphasized) {
                        (ChangeTag::Delete, true) => "\x1b[1;31m",
                        (ChangeTag::Delete, false) => "\x1b[31m",
                        (ChangeTag::Insert, true) => "\x1b[1;32m",
                        (ChangeTag::Insert, false) => "\x1b[32m",
                        (ChangeTag::Equal, _) => "",
                    };
                    out.push_str(color);
                    out.push_str(value.as_ref());
                    out.push_str("\x1b[0m");
                }

                if !diff.newline_terminated() {
                    out.push('\n');
                }
                if diff.newline_terminated() && change.missing_newline() {
                    out.push_str("\n\\ No newline at end of file\n");
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn plain_unified_diff_only_includes_changed_hunks() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nlineX\nline3\n";

        let diff = super::unified_file_diff(Path::new("doc.md"), old, new, false);

        assert!(diff.contains("@@ -1,3 +1,3 @@"));
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+lineX"));
        assert!(diff.contains(" line1"));
    }

    #[test]
    fn ansi_mode_emits_color_sequences() {
        let old = "aaa bbb ccc\n";
        let new = "aaa zzz ccc\n";

        let diff = super::unified_file_diff(Path::new("doc.md"), old, new, true);

        assert!(diff.contains("\x1b[31m") || diff.contains("\x1b[1;31m"));
        assert!(diff.contains("\x1b[32m") || diff.contains("\x1b[1;32m"));
        assert!(diff.contains("@@ -1 +1 @@"));
    }
}
