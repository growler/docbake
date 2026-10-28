//! Source probes for grid and multiline tables.
//!
//! The table parsers do not keep the column widths in the AST. These
//! helpers read them again from the source, at the position that the
//! parser sets on the `Table` node. The Markdown formatter and the DOCX
//! renderer use them.

pub(crate) fn source_line_at_pos(source: &str, pos: usize) -> Option<&str> {
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

pub(crate) fn source_tail_at_pos(source: &str, pos: usize) -> Option<&str> {
    if pos > source.len() {
        return None;
    }
    let end = source[pos..]
        .find('\n')
        .map(|idx| pos + idx)
        .unwrap_or(source.len());
    Some(source[pos..end].trim_end_matches('\r'))
}

// ─── Multiline-table source probes ────────────────────────────────────────
//
// The four helpers below duplicate the dash-rule / column-rule predicates
// implemented inside `src/multiline_table.rs`. The duplication is
// intentional: `multiline_table.rs` is a self-contained rushdown extension
// that we may publish as its own crate, and the formatter and the DOCX
// renderer (downstream consumers of the parsed AST) should not depend on
// parser internals.
//
// If the predicates here ever diverge from the parser's behaviour, the
// formatter falls back to the pipe-table emitter for the affected table —
// which is the pre-existing behaviour and not a correctness regression.
// The DOCX renderer divides the text width equally.

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
pub(crate) fn is_any_dash_rule_line(line: &str) -> bool {
    is_solid_dash_rule_line(line) || parse_grouped_dash_rule_line(line).is_some()
}

/// Returns the *display-column* extents of the grouped separator rule of a
/// multiline table whose opening dash rule begins at `table_pos`.
///
/// For with-header tables this is the second dash-rule line (after the
/// header lines). For headerless tables the opening rule itself is the
/// grouped separator. Returns `None` if no grouped rule is found before
/// the closing rule or the end of source.
pub(crate) fn multiline_table_separator_extents(
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
pub(crate) fn is_grid_border_line(line: &str) -> bool {
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
pub(crate) fn grid_table_column_widths(source: &str, table_pos: usize) -> Option<Vec<usize>> {
    grid_border_column_widths(source_line_at_pos(source, table_pos)?)
}

/// Returns the per-column widths of the grid-table border `line`, as
/// [`grid_table_column_widths`] does.
fn grid_border_column_widths(line: &str) -> Option<Vec<usize>> {
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

/// The maximum number of header lines between the solid top rule and the
/// column rule of a multiline table. The multiline table parser uses the
/// same limit.
const MULTILINE_MAX_HEADER_LINES: usize = 10;

/// Returns the relative column widths of the grid or multiline table that
/// starts at `table_pos`: the widths of the column groups of the top border
/// of a grid table, or of the column rule of a multiline table.
///
/// The probe reads the source from `table_pos` to the end of the line, thus
/// a table in a block quote, a list item or a definition also gives its
/// widths. For a multiline table with a header, the probe removes the
/// container prefix from each next line as the multiline table parser does
/// (see [`container_prefix_steps`]), and reads the lines until a blank line
/// or past the header line limit.
///
/// Returns `None` when:
///
/// - the table is a pipe table;
/// - the line at `table_pos` is not a grid border or a dash rule;
/// - a multiline table with a header has no column rule after at most
///   [`MULTILINE_MAX_HEADER_LINES`] header lines, or has a blank line
///   before its column rule;
/// - the container prefix before `table_pos` has a byte that is not a
///   space or `>`, or has a tab;
/// - a line between the top rule and the column rule does not start with
///   the container prefix, or has a tab in its container prefix.
pub(crate) fn column_weights(source: &str, table_pos: usize) -> Option<Vec<usize>> {
    let first = source_tail_at_pos(source, table_pos)?;
    if is_grid_border_line(first) {
        return grid_border_column_widths(first);
    }
    let extents = if let Some(extents) = parse_grouped_dash_rule_line(first) {
        extents
    } else if is_solid_dash_rule_line(first) {
        let line_start = source[..table_pos].rfind('\n').map_or(0, |idx| idx + 1);
        let steps = container_prefix_steps(&source[line_start..table_pos])?;
        source[table_pos..]
            .lines()
            .skip(1)
            .take(MULTILINE_MAX_HEADER_LINES + 1)
            .map_while(|line| strip_container_prefix(line, &steps))
            .take_while(|line| !line.trim().is_empty())
            .find_map(parse_grouped_dash_rule_line)?
    } else {
        return None;
    };
    Some(extents.iter().map(|(start, end)| end - start).collect())
}

/// One step of the container prefix that the multiline table parser removes
/// from each line of a table.
#[derive(Debug, Clone, Copy)]
enum PrefixStep {
    /// A run of spaces with this length (list item or definition indent).
    Spaces(usize),
    /// A block quote marker: spaces, `>`, then one optional space.
    QuoteMarker,
}

/// Divides `prefix`, the source between the line start and the top rule of
/// a multiline table, into the steps that the multiline table parser
/// (`ParentStripSeq::build` in `src/multiline_table.rs`) removes from each
/// next line. Returns `None` if `prefix` has a byte that is not a space or
/// `>`. Also returns `None` if `prefix` has a tab, because the column
/// arithmetic for tabs is not mirrored here.
fn container_prefix_steps(prefix: &str) -> Option<Vec<PrefixStep>> {
    let mut steps = Vec::new();
    let mut rest = prefix;
    while !rest.is_empty() {
        let spaces = rest.bytes().take_while(|&b| b == b' ').count();
        if spaces > 0 {
            steps.push(PrefixStep::Spaces(spaces));
            rest = &rest[spaces..];
        }
        if let Some(after) = rest.strip_prefix('>') {
            steps.push(PrefixStep::QuoteMarker);
            rest = after.strip_prefix(' ').unwrap_or(after);
        } else if !rest.is_empty() {
            return None;
        }
    }
    Some(steps)
}

/// Removes the container prefix `steps` from `line`, as the multiline table
/// parser does. Returns `None` if `line` does not start with the prefix, or
/// if it has a tab in the prefix.
fn strip_container_prefix<'a>(line: &'a str, steps: &[PrefixStep]) -> Option<&'a str> {
    let mut rest = line;
    for step in steps {
        rest = match *step {
            PrefixStep::Spaces(count) => {
                let head = rest.get(..count)?;
                if head.bytes().any(|b| b != b' ') {
                    return None;
                }
                &rest[count..]
            }
            PrefixStep::QuoteMarker => {
                let after = rest.trim_start_matches(' ').strip_prefix('>')?;
                after.strip_prefix(' ').unwrap_or(after)
            }
        };
    }
    Some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_table_weights_come_from_top_border() {
        let source = "+----+--------+\n| a  | b      |\n+----+--------+\n";
        assert_eq!(column_weights(source, 0), Some(vec![4, 8]));
    }

    #[test]
    fn grid_table_in_block_quote_gives_weights() {
        let source = "> +--+------+\n> | a | b   |\n> +--+------+\n";
        assert_eq!(column_weights(source, 2), Some(vec![2, 6]));
    }

    #[test]
    fn multiline_table_weights_come_from_column_rule() {
        let source = "\
--------------------
Name      Value
------    ----------
one       1

two       2
--------------------
";
        assert_eq!(column_weights(source, 0), Some(vec![6, 10]));
        let headerless = "---  ------\none  1\n---  ------\n";
        assert_eq!(column_weights(headerless, 0), Some(vec![3, 6]));
    }

    #[test]
    fn multiline_header_scan_stops_at_blank_line() {
        let source = "\
-----

---  ------
";
        assert_eq!(column_weights(source, 0), None);
    }

    /// The weights of the multiline table with a header in `source`, with
    /// the table position at the first dash of the top rule.
    fn multiline_weights(source: &str) -> Option<Vec<usize>> {
        column_weights(source, source.find("---").unwrap())
    }

    #[test]
    fn multiline_table_with_header_in_block_quote_gives_weights() {
        let source = "\
> -------------------
> Name   Value
> -----  -----------
> one    1
> -------------------
";
        assert_eq!(multiline_weights(source), Some(vec![5, 11]));
        let no_space = "\
> -------------------
>Name   Value
>-----  -----------
>one    1
> -------------------
";
        assert_eq!(multiline_weights(no_space), Some(vec![5, 11]));
        let nested = "\
> > -------------------
>> Name   Value
> >-----  -----------
> > one    1
> > -------------------
";
        assert_eq!(multiline_weights(nested), Some(vec![5, 11]));
    }

    #[test]
    fn multiline_table_with_header_in_ordered_list_gives_weights() {
        let source = "\
1.  Ordered item

    -------------------
    Name   Value
    -----  -----------
    one    1
    -------------------
";
        assert_eq!(multiline_weights(source), Some(vec![5, 11]));
    }

    #[test]
    fn multiline_table_with_header_in_definition_gives_weights() {
        let source = "\
Term
:   Definition.

    -------------------
    Name   Value
    -----  -----------
    one    1
    -------------------
";
        assert_eq!(multiline_weights(source), Some(vec![5, 11]));
    }

    #[test]
    fn multiline_header_scan_stops_at_short_container_prefix() {
        let source = "\
1.  Ordered item

    -------------------
   Name   Value
    -----  -----------
";
        assert_eq!(multiline_weights(source), None);
        let quote = "\
> -------------------
Name   Value
> -----  -----------
";
        assert_eq!(multiline_weights(quote), None);
    }

    #[test]
    fn pipe_table_gives_no_weights() {
        let source = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert_eq!(column_weights(source, 0), None);
    }
}
