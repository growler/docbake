use std::fmt::Write as _;

use rushdown::ast::{Arena, CodeBlockKind, KindData, NodeRef};

pub(super) fn escape_md_title(title: &str) -> String {
    title.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(super) fn format_inline_destination(destination: &str) -> String {
    if destination.chars().any(char::is_whitespace) {
        format!("<{destination}>")
    } else {
        destination.to_string()
    }
}

pub(super) fn backtick_fence(content: &str) -> String {
    let mut max_run = 0usize;
    let mut current_run = 0usize;
    for c in content.chars() {
        if c == '`' {
            current_run += 1;
            if current_run > max_run {
                max_run = current_run;
            }
        } else {
            current_run = 0;
        }
    }
    "`".repeat(max_run + 1)
}

pub(super) fn code_span_has_explicit_padding(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> bool {
    let Some(pos) = arena[node_ref].pos() else {
        return false;
    };
    let bytes = source.as_bytes();
    if pos >= bytes.len() || bytes[pos] != b'`' {
        return false;
    }

    let mut open_end = pos;
    while open_end < bytes.len() && bytes[open_end] == b'`' {
        open_end += 1;
    }
    let open_len = open_end - pos;
    if open_len == 0 {
        return false;
    }

    let Some(close_start) = find_backtick_closing_run(bytes, open_end, open_len) else {
        return false;
    };
    let raw_content = &source[open_end..close_start];
    raw_content.starts_with(' ')
        && raw_content.ends_with(' ')
        && raw_content.chars().any(|c| !c.is_whitespace())
}

fn find_backtick_closing_run(bytes: &[u8], mut from: usize, open_len: usize) -> Option<usize> {
    while from < bytes.len() {
        if bytes[from] != b'`' {
            from += 1;
            continue;
        }

        let mut run_end = from;
        while run_end < bytes.len() && bytes[run_end] == b'`' {
            run_end += 1;
        }
        let run_len = run_end - from;

        if run_len == open_len {
            let prev_is_backtick = from > 0 && bytes[from - 1] == b'`';
            let next_is_backtick = run_end < bytes.len() && bytes[run_end] == b'`';
            if !prev_is_backtick && !next_is_backtick {
                return Some(from);
            }
        }

        from = run_end;
    }

    None
}

pub(super) fn render_node_attrs(arena: &Arena, node_ref: NodeRef, source: &str) -> Option<String> {
    let attrs = arena[node_ref].attributes();
    if attrs.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push('{');
    let mut first = true;

    if let Some(id) = attrs.get("id") {
        write!(out, "#{}", id.str(source)).ok()?;
        first = false;
    }
    if let Some(class) = attrs.get("class") {
        for cls in class.str(source).split_whitespace() {
            if !first {
                out.push(' ');
            }
            write!(out, ".{cls}").ok()?;
            first = false;
        }
    }

    for (key, value) in attrs.iter() {
        if key == "id" || key == "class" {
            continue;
        }
        if !first {
            out.push(' ');
        }
        write!(out, "{}=\"{}\"", key, value.str(source)).ok()?;
        first = false;
    }

    out.push('}');
    Some(out)
}

pub(super) fn fenced_div_opening(arena: &Arena, node_ref: NodeRef, source: &str) -> String {
    if let Some(opening) = fenced_div_opening_from_source(arena, node_ref, source) {
        return opening;
    }

    let attrs = arena[node_ref].attributes();
    if attrs.is_empty() {
        return ":::".to_string();
    }

    let id = attrs.get("id").map(|v| v.str(source));
    let class = attrs
        .get("class")
        .map(|v| v.str(source))
        .unwrap_or_default();
    let class_items: Vec<&str> = class.split_whitespace().collect();
    let has_other_attrs = attrs.iter().any(|(k, _)| k != "id" && k != "class");

    if id.is_none() && !has_other_attrs && class_items.len() == 1 {
        return format!(":::{}", class_items[0]);
    }

    let attrs_text = render_node_attrs(arena, node_ref, source).unwrap_or_default();
    format!(":::{attrs_text}")
}

pub(super) fn fenced_div_closing_and_trailing_blank_lines(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Option<(String, usize)> {
    let pos = arena[node_ref].pos()?;
    let opening = source_line_at_pos(source, pos)?;
    let opening_run = parse_colon_fence_opening(opening)?;

    let opening_line_start = source[..pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let opening_line_end = source[opening_line_start..]
        .find('\n')
        .map(|idx| opening_line_start + idx)
        .unwrap_or(source.len());

    let mut cursor = if opening_line_end < source.len() {
        opening_line_end + 1
    } else {
        opening_line_end
    };
    let mut stack = vec![opening_run];

    while cursor < source.len() {
        let line_end = source[cursor..]
            .find('\n')
            .map(|idx| cursor + idx)
            .unwrap_or(source.len());
        let line = source[cursor..line_end].trim_end_matches('\r');

        if let Some(close_run) = parse_colon_fence_closing(line) {
            if close_run >= *stack.last().unwrap_or(&usize::MAX) {
                stack.pop();
                if stack.is_empty() {
                    let mut blank_lines = 0usize;
                    let mut next = if line_end < source.len() {
                        line_end + 1
                    } else {
                        line_end
                    };
                    while next < source.len() {
                        let next_end = source[next..]
                            .find('\n')
                            .map(|idx| next + idx)
                            .unwrap_or(source.len());
                        let next_line = source[next..next_end].trim_end_matches('\r');
                        if !next_line.trim().is_empty() {
                            break;
                        }
                        blank_lines += 1;
                        next = if next_end < source.len() {
                            next_end + 1
                        } else {
                            next_end
                        };
                    }
                    return Some((line.to_string(), blank_lines));
                }
            }
        } else if let Some(open_run) = parse_colon_fence_opening(line) {
            stack.push(open_run);
        }

        if line_end == source.len() {
            break;
        }
        cursor = line_end + 1;
    }

    None
}

fn fenced_div_opening_from_source(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Option<String> {
    let pos = arena[node_ref].pos()?;
    let line = source_line_at_pos(source, pos)?;
    let trimmed = line.trim_start();
    trimmed.starts_with(":::").then(|| line.to_string())
}

pub(super) fn opening_fence_from_source(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Option<String> {
    let pos = arena[node_ref].pos()?;
    let line = source_tail_at_pos(source, pos)?;
    parse_opening_fence(line).map(|_| line.to_string())
}

pub(super) fn verbatim_fenced_code_block_from_source(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Option<String> {
    let KindData::CodeBlock(code) = arena[node_ref].kind_data() else {
        return None;
    };
    if !matches!(code.code_block_kind(), CodeBlockKind::Fenced) {
        return None;
    }

    let pos = arena[node_ref].pos()?;
    let start = source[..pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let opening = source_line_at_pos(source, pos)?;
    let (_, marker, run) = parse_opening_fence_any_indent(opening)?;

    let mut line_start = start;
    while line_start < source.len() {
        let line_end = source[line_start..]
            .find('\n')
            .map(|idx| line_start + idx)
            .unwrap_or(source.len());
        let line = source[line_start..line_end].trim_end_matches('\r');
        if line_start > start && is_fence_close_any_indent(line, marker, run) {
            let block_end = if line_end < source.len() {
                line_end + 1
            } else {
                line_end
            };
            return Some(source[start..block_end].to_string());
        }
        if line_end == source.len() {
            break;
        }
        line_start = line_end + 1;
    }

    None
}

pub(super) fn closing_fence_from_opening(opening: &str) -> Option<String> {
    let (indent, marker, run) = parse_opening_fence(opening)?;
    Some(format!(
        "{}{}",
        " ".repeat(indent),
        marker.to_string().repeat(run)
    ))
}

pub(super) fn source_line_at_pos(source: &str, pos: usize) -> Option<&str> {
    if pos > source.len() {
        return None;
    }
    let start = source[..pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let end = source[start..]
        .find('\n')
        .map(|idx| start + idx)
        .unwrap_or(source.len());
    Some(source[start..end].trim_end_matches('\r'))
}

pub(super) fn source_tail_at_pos(source: &str, pos: usize) -> Option<&str> {
    if pos > source.len() {
        return None;
    }
    let end = source[pos..]
        .find('\n')
        .map(|idx| pos + idx)
        .unwrap_or(source.len());
    Some(source[pos..end].trim_end_matches('\r'))
}

pub(super) fn caption_prefix_from_source(
    arena: &Arena,
    node_ref: NodeRef,
    block_child: Option<NodeRef>,
    source: &str,
) -> Option<String> {
    let pos = block_child
        .and_then(|child| arena[child].pos())
        .or_else(|| arena[node_ref].pos())?;

    let start = source[..pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let preceding = source[..start].trim_end_matches(['\n', '\r']);
    let line_start = preceding.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let line = preceding.get(line_start..)?;
    let trimmed = line.trim_start();
    let colon = trimmed.find(':')?;
    let prefix = &trimmed[..=colon];
    match prefix.to_ascii_lowercase().as_str() {
        "table:" | "figure:" | "listing:" => Some(prefix.to_string()),
        _ => None,
    }
}

pub(super) fn emphasis_marker_from_source(arena: &Arena, node_ref: NodeRef, source: &str) -> char {
    let Some(pos) = arena[node_ref].pos() else {
        return '*';
    };
    match source.as_bytes().get(pos).copied() {
        Some(b'_') => '_',
        Some(b'*') => '*',
        _ => '*',
    }
}

pub(super) fn hard_line_break_marker(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> &'static str {
    let Some(pos) = arena[node_ref].pos() else {
        return "\\\n";
    };
    let Some(line) = source_line_at_pos(source, pos) else {
        return "\\\n";
    };
    if line.ends_with('\\') {
        "\\\n"
    } else if line.ends_with("  ") {
        "@@FMT_HARD_BREAK_SPACE@@\n"
    } else {
        "\\\n"
    }
}

pub(super) fn original_reference_link(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Option<String> {
    let start = arena[node_ref].pos()?;
    let first_end = parse_link_bracket(source, start)?;
    let rest = source.get(first_end..)?;

    if rest.starts_with('(') {
        return None;
    }

    if rest.starts_with('[') {
        let second_end = parse_link_bracket(source, first_end)?;
        return source.get(start..second_end).map(ToString::to_string);
    }

    source.get(start..first_end).map(ToString::to_string)
}

fn parse_link_bracket(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(start) != Some(&b'[') {
        return None;
    }

    let mut depth = 0usize;
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                index = (index + 2).min(bytes.len());
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn parse_opening_fence(line: &str) -> Option<(usize, char, usize)> {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    if indent > 3 {
        return None;
    }
    let trimmed = &line[indent..];
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' && marker != ':' {
        return None;
    }
    let run = trimmed.chars().take_while(|c| *c == marker).count();
    (run >= 3).then_some((indent, marker, run))
}

fn parse_colon_fence_opening(line: &str) -> Option<usize> {
    let (_, marker, run) = parse_opening_fence(line)?;
    (marker == ':').then_some(run)
}

fn parse_colon_fence_closing(line: &str) -> Option<usize> {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    if indent > 3 {
        return None;
    }
    let trimmed = &line[indent..];
    let run = parse_colon_fence_opening(trimmed)?;
    trimmed[run..]
        .chars()
        .all(|c| c == ' ' || c == '\t')
        .then_some(run)
}

fn parse_opening_fence_any_indent(line: &str) -> Option<(usize, char, usize)> {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    let trimmed = &line[indent..];
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|c| *c == marker).count();
    (run >= 3).then_some((indent, marker, run))
}

fn is_fence_close_any_indent(line: &str, marker: char, open_len: usize) -> bool {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    let trimmed = &line[indent..];
    let run = trimmed.chars().take_while(|c| *c == marker).count();
    if run < open_len {
        return false;
    }
    trimmed[run..].chars().all(|c| c == ' ' || c == '\t')
}

pub(super) fn definition_prefix_from_source(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Option<String> {
    let pos = arena[node_ref].pos()?;
    let tail = source_tail_at_pos(source, pos)?;
    let colon = tail.find(':')?;
    if colon > 3 {
        return None;
    }

    let whitespace_len = tail[colon + 1..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .count();
    if whitespace_len == 0 {
        return Some(tail[..=colon].to_string());
    }

    Some(tail[..colon + 1 + whitespace_len].to_string())
}

pub(super) fn definition_child_indent_from_source(
    arena: &Arena,
    def_ref: NodeRef,
    child_ref: NodeRef,
    source: &str,
) -> Option<String> {
    let def_pos = arena[def_ref].pos()?;
    let def_line_start = source[..def_pos]
        .rfind('\n')
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let def_indent = source[def_line_start..def_pos].chars().count();

    let child_pos = arena[child_ref].pos()?;
    let child_line = source_line_at_pos(source, child_pos)?;
    let child_indent = child_line.chars().take_while(|c| *c == ' ').count();

    Some(" ".repeat(child_indent.saturating_sub(def_indent)))
}

pub(super) fn footnote_has_inline_body(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Option<bool> {
    let pos = arena[node_ref].pos()?;
    let tail = source_tail_at_pos(source, pos)?;
    let colon = tail.find(':')?;
    Some(!tail[colon + 1..].trim().is_empty())
}

// ─── Multiline-table source probes ────────────────────────────────────────
//
// The four helpers below duplicate the dash-rule / column-rule predicates
// implemented inside `src/multiline_table.rs`. The duplication is
// intentional: `multiline_table.rs` is a self-contained rushdown extension
// that we may publish as its own crate, and the formatter (a downstream
// consumer of the parsed AST) should not depend on parser internals.
//
// If the predicates here ever diverge from the parser's behaviour, the
// formatter falls back to the pipe-table emitter for the affected table —
// which is the pre-existing behaviour and not a correctness regression.

/// Returns `true` if `line` is a *solid* dash rule: indent ≤ 3 spaces,
/// followed by ≥ 3 contiguous dashes, with only horizontal whitespace
/// afterwards. Tabs disqualify (the parser rejects them as well).
fn is_solid_dash_rule_line(line: &str) -> bool {
    if line.bytes().any(|b| b == b'\t') {
        return false;
    }
    let bytes = line.as_bytes();
    let indent = bytes.iter().take_while(|&&b| b == b' ').count();
    if indent > 3 {
        return false;
    }
    let rest = &bytes[indent..];
    let dashes = rest.iter().take_while(|&&b| b == b'-').count();
    if dashes < 3 {
        return false;
    }
    rest[dashes..].iter().all(|&b| b == b' ')
}

/// Parses a *grouped* dash rule (two or more dash groups separated by
/// spaces) and returns the byte/column extents of each group as
/// `(start, end)`. Returns `None` for non-grouped or invalid lines.
/// Pure ASCII (dashes + spaces), so byte offsets equal display columns.
fn parse_grouped_dash_rule_line(line: &str) -> Option<Vec<(usize, usize)>> {
    if line.bytes().any(|b| b == b'\t') {
        return None;
    }
    let bytes = line.as_bytes();
    let mut pos = bytes.iter().take_while(|&&b| b == b' ').count();
    if pos > 3 {
        return None;
    }

    // Trim trailing spaces from the meaningful content end.
    let mut content_end = bytes.len();
    while content_end > pos && bytes[content_end - 1] == b' ' {
        content_end -= 1;
    }

    for &b in &bytes[pos..content_end] {
        if b != b'-' && b != b' ' {
            return None;
        }
    }

    let mut groups: Vec<(usize, usize)> = Vec::new();
    while pos < content_end {
        if bytes[pos] == b' ' {
            pos += 1;
            continue;
        }
        let group_start = pos;
        while pos < content_end && bytes[pos] == b'-' {
            pos += 1;
        }
        groups.push((group_start, pos));
    }

    if groups.len() >= 2 {
        Some(groups)
    } else {
        None
    }
}

/// Returns `true` if `line` is any valid dash rule (solid or grouped) — the
/// signal the formatter uses to recognise a multiline table from the source
/// position the parser planted on the `Table` node.
pub(super) fn is_any_dash_rule_line(line: &str) -> bool {
    is_solid_dash_rule_line(line) || parse_grouped_dash_rule_line(line).is_some()
}

/// Returns the *display-column* extents of the grouped separator rule of a
/// multiline table whose opening dash rule begins at `table_pos`.
///
/// For with-header tables this is the second dash-rule line (after the
/// header lines). For headerless tables the opening rule itself is the
/// grouped separator. Returns `None` if no grouped rule is found before
/// the closing rule or the end of source.
pub(super) fn multiline_table_separator_extents(
    source: &str,
    table_pos: usize,
) -> Option<Vec<(usize, usize)>> {
    let mut cursor = table_pos;
    while cursor <= source.len() {
        let line_end = source[cursor..]
            .find('\n')
            .map(|idx| cursor + idx)
            .unwrap_or(source.len());
        let line = source[cursor..line_end].trim_end_matches('\r');

        if let Some(extents) = parse_grouped_dash_rule_line(line) {
            return Some(extents);
        }

        if line_end >= source.len() {
            return None;
        }
        cursor = line_end + 1;
    }
    None
}

// ─── Grid-table source probes ─────────────────────────────────────────────
//
// The two helpers below mirror the dash-rule probes above but for pandoc
// grid-table borders (`+---+---+` / `+===+===+`, optionally carrying `:`
// alignment markers). The duplication with `src/grid_table.rs` is
// intentional — same self-contained-extension rationale as the multiline-
// table block.

/// Returns `true` if `line` is a pandoc grid-table border: indent ≤ 3 spaces,
/// `+` at start and end, only `-` / `=` / `:` / `+` characters between, no
/// mixed `-` / `=`, no empty columns (`++`), at least two `+`. Tabs
/// disqualify.
pub(super) fn is_grid_border_line(line: &str) -> bool {
    if line.bytes().any(|b| b == b'\t') {
        return false;
    }
    let bytes = line.as_bytes();
    let indent = bytes.iter().take_while(|&&b| b == b' ').count();
    if indent > 3 {
        return false;
    }
    let mut content_end = bytes.len();
    while content_end > indent && bytes[content_end - 1] == b' ' {
        content_end -= 1;
    }
    let rest = &bytes[indent..content_end];
    if rest.len() < 3 {
        return false;
    }
    if rest[0] != b'+' || rest[rest.len() - 1] != b'+' {
        return false;
    }
    let mut plus_positions: Vec<usize> = Vec::new();
    let mut saw_dash = false;
    let mut saw_equals = false;
    for (i, &b) in rest.iter().enumerate() {
        match b {
            b'+' => plus_positions.push(i),
            b'-' => saw_dash = true,
            b'=' => saw_equals = true,
            b':' => {}
            _ => return false,
        }
    }
    if plus_positions.len() < 2 {
        return false;
    }
    if saw_dash && saw_equals {
        return false;
    }
    // No empty column (`++` with nothing between).
    for w in plus_positions.windows(2) {
        if w[1] - w[0] < 2 {
            return false;
        }
    }
    true
}

/// Returns the per-column widths (in bytes between consecutive `+` characters,
/// exclusive of the `+` themselves) of the grid-table top border at
/// `table_pos`. The byte counts equal display-column counts because borders
/// are pure ASCII. Returns `None` if the source line at `table_pos` is not a
/// grid-table border or contains fewer than two `+`.
pub(super) fn grid_table_column_widths(source: &str, table_pos: usize) -> Option<Vec<usize>> {
    let line = source_line_at_pos(source, table_pos)?;
    if !is_grid_border_line(line) {
        return None;
    }
    let bytes = line.as_bytes();
    let indent = bytes.iter().take_while(|&&b| b == b' ').count();
    let mut content_end = bytes.len();
    while content_end > indent && bytes[content_end - 1] == b' ' {
        content_end -= 1;
    }
    let rest = &bytes[indent..content_end];
    let plus: Vec<usize> = rest
        .iter()
        .enumerate()
        .filter_map(|(i, &b)| (b == b'+').then_some(i))
        .collect();
    if plus.len() < 2 {
        return None;
    }
    Some(plus.windows(2).map(|w| w[1] - w[0] - 1).collect())
}
