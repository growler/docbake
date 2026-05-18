//! Pandoc-style multiline tables for rushdown.
//!
//! Implements the `multiline_tables` extension described in the [pandoc manual].
//! Header and body rows may span multiple physical lines; blank lines separate
//! body rows from each other.
//!
//! # Syntax
//!
//! **With header** — a solid dash rule opens the table, a grouped dash rule
//! (two or more dash groups separated by spaces) acts as the separator between
//! header and body, and another solid dash rule closes the table:
//!
//! ```markdown
//! -------------------------------------------------------
//!  Centered   Default           Right Left
//!   Header    Aligned         Aligned Aligned
//! ----------- ------- --------------- -----------------
//!    First    row                12.0 Example of a row
//!                                     that spans two
//!                                     lines.
//!
//!    Second   row                 5.0 Another row.
//! -------------------------------------------------------
//! ```
//!
//! **Without header** — a grouped dash rule opens the table (it also defines
//! the columns), body rows follow, and another dash rule (solid or grouped)
//! closes the table:
//!
//! ```markdown
//! ----------- ------- --------------- -----------------
//!    First    row                12.0 Spans multiple
//!                                     lines.
//!
//!    Second   row                 5.0 Another row.
//! ----------- ------- --------------- -----------------
//! ```
//!
//! ## Column alignment
//!
//! Alignment is derived from the position of the header (or first body line
//! for headerless tables) relative to the column's dash extents in the
//! separator rule:
//!
//! | Text position             | Alignment |
//! |---------------------------|-----------|
//! | Flush left and right      | Default   |
//! | Flush right, inset left   | Right     |
//! | Flush left, inset right   | Left      |
//! | Inset on both sides       | Center    |
//!
//! ## Notes
//!
//! - Tab characters inside rule lines or body text are **not** supported and
//!   support is not planned: lines containing tab-expanded content
//!   (padding > 0) cause the table to fall through to other parsers.
//!
//! [pandoc manual]: https://pandoc.org/demo/example33/8.9-tables.html

use rushdown::as_type_data_mut;
use rushdown::ast::{
    Arena, NodeRef, Table, TableBody, TableCell, TableCellAlignment, TableHeader, TableRow,
};
use rushdown::parser::{
    AnyBlockParser, BlockParser, Context, NoParserOptions, Parser, ParserExtension,
    ParserExtensionFn, State,
};
use rushdown::text::{self, Reader as _, Segment};
use rushdown::util::{
    indent_width, is_blank, is_space, trim_left_space_length, trim_right_space_length,
};

// ============================================================================
// Low-level helpers (byte-level, no regex, no string allocation)
// ============================================================================

/// Maps a *display-column* position to a byte offset within the UTF-8 string
/// `s`.
///
/// Pandoc multiline tables describe column boundaries in terms of the visible
/// columns on the dash rule (which is pure ASCII — bytes == columns). Content
/// lines, however, may contain multi-byte UTF-8 characters (Cyrillic, accented
/// Latin, etc.), so the same column index does not generally land at the same
/// *byte* offset on a content line. Using the raw column index to slice
/// content bytes splits multi-byte characters in half and produces invalid
/// UTF-8 — see <https://… >.
///
/// This helper approximates each Unicode scalar value as one display column
/// (good for ASCII and most European scripts; wide CJK characters will under-
/// count, which mirrors what a writer authoring such a table by visual
/// alignment would see in a monospace editor that renders them as one column).
/// Returns `s.len()` when `target_col` is at or past the end of `s`'s column
/// width.
fn byte_offset_for_col(s: &str, target_col: usize) -> usize {
    for (col, (byte_idx, _ch)) in s.char_indices().enumerate() {
        if col == target_col {
            return byte_idx;
        }
    }
    s.len()
}

/// Returns the byte length of `bytes` after stripping a trailing `\n` or `\r\n`.
fn strip_newline(bytes: &[u8]) -> usize {
    let n = bytes.len();
    if n >= 2 && bytes[n - 2] == b'\r' && bytes[n - 1] == b'\n' {
        n - 2
    } else if n >= 1 && bytes[n - 1] == b'\n' {
        n - 1
    } else {
        n
    }
}

/// Returns `true` if `line` is a *solid* dash rule: a single contiguous run
/// of `≥ 3` dashes with indent `≤ 3` and no other characters.
///
/// `offset` is the current byte distance from the logical line head (for tab
/// width calculation), as returned by [`Reader::line_offset`].
fn is_solid_rule(line: &[u8], offset: usize) -> bool {
    let (w, pos) = indent_width(line, offset);
    if w > 3 {
        return false;
    }
    let rest = &line[pos..];
    let dash_count = rest.iter().take_while(|&&b| b == b'-').count();
    if dash_count < 3 {
        return false;
    }
    // Anything after the dashes must be horizontal whitespace.
    rest[dash_count..].iter().all(|&b| is_space(b))
}

/// Parses a *grouped* dash rule (two or more consecutive dash groups separated
/// by at least one space) and returns the byte extents of each group as
/// `(start, end)` — half-open, relative to the start of the `line` slice.
///
/// Returns `None` if the line is not a valid grouped rule (fewer than two
/// groups, non-dash/space characters, indent `> 3`).
fn parse_column_rule(line: &[u8], offset: usize) -> Option<Vec<(usize, usize)>> {
    let (w, mut pos) = indent_width(line, offset);
    if w > 3 {
        return None;
    }

    // Exclusive end of meaningful content (trim trailing spaces).
    let content_end = line.len() - trim_right_space_length(line);

    // Ensure every content byte is either a dash or a space.
    for &b in &line[pos..content_end] {
        if b != b'-' && !is_space(b) {
            return None;
        }
    }

    let mut groups: Vec<(usize, usize)> = Vec::new();
    while pos < content_end {
        if is_space(line[pos]) {
            pos += 1;
            continue;
        }
        // Must be a dash (verified above).
        let group_start = pos;
        while pos < content_end && line[pos] == b'-' {
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

/// Returns `true` if `line` is any valid dash rule (solid **or** grouped with
/// ≥ 2 groups).
fn is_any_dash_rule(line: &[u8], offset: usize) -> bool {
    is_solid_rule(line, offset) || parse_column_rule(line, offset).is_some()
}

/// Extracts the cell content segment for column `[col_start, col_end)` from
/// a physical source line described by `line_seg`.
///
/// When `is_last_col` is `true` the column extends to the end of the content
/// (supporting over-wide last-column content). Leading and trailing spaces
/// are trimmed. Returns `None` when the slice is entirely whitespace or the
/// line is shorter than `col_start`.
///
/// The returned segment **never** includes the trailing newline of the
/// physical source line. Inter-line soft line breaks for multi-line cells
/// are encoded by [`populate_cell`] via [`Segment::with_force_newline`],
/// which makes `parse_block` see a virtual `\n` at the end of each
/// non-final cell line and emit a `SOFT_LINE_BREAK` text qualifier.
fn extract_cell_segment(
    source: &str,
    line_seg: Segment,
    col_start: usize,
    col_end: usize,
    is_last_col: bool,
) -> Option<Segment> {
    debug_assert_eq!(line_seg.padding(), 0);

    let line_bytes = &source.as_bytes()[line_seg.start()..line_seg.stop()];
    let content_byte_len = strip_newline(line_bytes);
    // SAFETY-ish: `source` is `&str` and we only trimmed a trailing ASCII
    // `\n` / `\r\n`, so the resulting range is still on a UTF-8 boundary.
    let content = &source[line_seg.start()..line_seg.start() + content_byte_len];

    let slice_start = byte_offset_for_col(content, col_start);
    let slice_end = if is_last_col {
        content.len()
    } else {
        byte_offset_for_col(content, col_end)
    };

    if slice_start >= slice_end {
        return None;
    }

    let seg = Segment::from((line_seg.start() + slice_start, line_seg.start() + slice_end))
        .trim_left_space(source)
        .trim_right_space(source);

    if seg.is_empty() {
        None
    } else {
        Some(seg)
    }
}

/// Computes column alignments by comparing the text extents in
/// `reference_lines` against the column dash-group extents `col_extents`.
///
/// For with-header tables, pass all header line segments as `reference_lines`.
/// For headerless tables, pass only the first physical line of the first body
/// row (pandoc derives alignment from the first body line when the header is
/// omitted).
fn compute_alignments(
    source: &str,
    col_extents: &[(usize, usize)],
    reference_lines: &[Segment],
) -> Vec<TableCellAlignment> {
    col_extents
        .iter()
        .map(|&(col_start, col_end)| {
            let mut t_left_min: Option<usize> = None;
            let mut t_right_max: Option<usize> = None;

            for &line_seg in reference_lines {
                let line_bytes = &source.as_bytes()[line_seg.start()..line_seg.stop()];
                let content_byte_len = strip_newline(line_bytes);
                let content =
                    &source[line_seg.start()..line_seg.start() + content_byte_len];
                let line_col_width = content.chars().count();

                // Always use col_end as the right boundary for alignment
                // (do not extend to end-of-line even for the last column).
                let s_col_start = col_start.min(line_col_width);
                let s_col_end = col_end.min(line_col_width);
                if s_col_start >= s_col_end {
                    continue;
                }

                let b_start = byte_offset_for_col(content, s_col_start);
                let b_end = byte_offset_for_col(content, s_col_end);
                let cell_bytes = &content.as_bytes()[b_start..b_end];
                let left_trim = trim_left_space_length(cell_bytes);
                let right_trim = trim_right_space_length(cell_bytes);
                if left_trim + right_trim >= cell_bytes.len() {
                    continue; // entirely whitespace
                }

                // `left_trim` / `right_trim` only count ASCII spaces, so each
                // trimmed byte equals one display column — we can stay in
                // column space for the alignment comparison below.
                let t_left = s_col_start + left_trim;
                let t_right = s_col_end - right_trim;
                t_left_min = Some(t_left_min.map_or(t_left, |v| v.min(t_left)));
                t_right_max = Some(t_right_max.map_or(t_right, |v| v.max(t_right)));
            }

            let (Some(t_left), Some(t_right)) = (t_left_min, t_right_max) else {
                return TableCellAlignment::None;
            };

            // Pandoc alignment rules (from simple_tables spec):
            // - flush both sides  → default (None)
            // - flush right only  → Right
            // - flush left only   → Left
            // - inset both sides  → Center
            match (t_left == col_start, t_right == col_end) {
                (true, true) => TableCellAlignment::None,
                (false, true) => TableCellAlignment::Right,
                (true, false) => TableCellAlignment::Left,
                (false, false) => TableCellAlignment::Center,
            }
        })
        .collect()
}

/// Appends cell content from all physical `lines` of a row to `cell_ref`'s
/// Block source.
///
/// Each physical line is sliced to its column boundary
/// (`[col_start, col_end)`) by [`extract_cell_segment`] and trimmed; lines
/// whose column slice is empty are skipped (they neither contribute text
/// nor a soft line break).
///
/// To emit a soft line break between consecutive physical lines of the
/// same cell we use a two-segment pattern per non-final line:
///
/// 1. A **content segment** extended by one trailing byte past the
///    trimmed text (typically the column-internal or inter-column
///    space). The extension forces `parse_block`'s inline loop into the
///    `is_space` branch at the boundary character, which in turn calls
///    `merge_or_append_text` and commits the line's text node before
///    `BlockReader::advance` crosses the segment boundary. Without the
///    extra byte, the inline parser would advance past the segment end,
///    detect that the reader had moved to the next line
///    (`l != current_l`) and silently drop the text node.
/// 2. A **separator segment** that is exactly one byte long and points
///    at the originating physical line's trailing `\n` in the source.
///    `parse_block` sees a one-byte line that ends with `\n` and emits
///    an empty text node carrying the `SOFT_LINE_BREAK` qualifier — the
///    HTML renderer turns that qualifier into a `\n` between cell
///    lines, matching pandoc's `--wrap=none` semantics.
///
/// The final content line is emitted as the trimmed segment with no
/// extension and no following separator: when the reader is on the last
/// block segment, slow-path advance no longer triggers `advance_line`
/// (because `pos.stop == self.last`) and the end-of-line text node is
/// produced normally.
fn populate_cell(
    arena: &mut Arena,
    cell_ref: NodeRef,
    source: &str,
    lines: &[Segment],
    col_start: usize,
    col_end: usize,
    is_last_col: bool,
) {
    // First pass: extract a column-confined, trimmed segment per physical
    // line. Remember the originating line segment so we can locate its
    // trailing `\n` for separator emission.
    let mut collected: Vec<(Segment, Segment)> = Vec::with_capacity(lines.len());
    for &ls in lines {
        if let Some(seg) = extract_cell_segment(source, ls, col_start, col_end, is_last_col) {
            collected.push((seg, ls));
        }
    }

    if collected.is_empty() {
        return;
    }

    let last_idx = collected.len() - 1;
    let source_bytes = source.as_bytes();
    for (i, (content_seg, line_seg)) in collected.into_iter().enumerate() {
        if i < last_idx {
            // Extend by one byte so the trailing character forces either
            // `merge_or_append_text` (space extension, non-final cell on
            // non-last column) or a normal end-of-line text node (`\n`
            // extension, last column whose content reaches the line's
            // newline) to commit the cell's text before advance crosses
            // the segment boundary.
            let extended_stop = (content_seg.stop() + 1).min(line_seg.stop());
            let extended = Segment::from((content_seg.start(), extended_stop));
            as_type_data_mut!(arena, cell_ref, Block).append_source_line(extended);

            // Emit a separator segment carrying only the physical line's
            // `\n` byte so `parse_block` produces an empty text node with
            // the `SOFT_LINE_BREAK` qualifier. This is needed only when
            // the extension byte was *not* the line's `\n`: when the
            // extended segment already ends with `\n` (last column case),
            // `parse_block` keeps advance on its fast path through the
            // content and emits the soft line break on the content text
            // node itself, so a second separator segment would render an
            // unwanted blank line.
            let extension_is_newline =
                source_bytes.get(extended_stop.saturating_sub(1)) == Some(&b'\n');
            if !extension_is_newline && line_seg.stop() > line_seg.start() {
                let nl_pos = line_seg.stop() - 1;
                if source_bytes.get(nl_pos) == Some(&b'\n') {
                    let sep = Segment::from((nl_pos, line_seg.stop()));
                    as_type_data_mut!(arena, cell_ref, Block).append_source_line(sep);
                }
            }
        } else {
            as_type_data_mut!(arena, cell_ref, Block).append_source_line(content_seg);
        }
    }
}

// ============================================================================
// Block parser
// ============================================================================

/// [`BlockParser`] for pandoc-style multiline tables.
///
/// Triggered on `b"-"` at priority 50 — ahead of setext headings (100) and
/// thematic breaks (200).  The entire table is validated and materialised in
/// [`open`][Self::open]; [`cont`][Self::cont] is a no-op.  On look-ahead
/// failure the reader position is restored so downstream parsers can claim
/// the dashes line.
#[derive(Debug, Default)]
pub(crate) struct MultilineTableParser;

impl MultilineTableParser {
    pub fn new() -> Self {
        Self
    }
}

impl BlockParser for MultilineTableParser {
    fn trigger(&self) -> &[u8] {
        b"-"
    }

    fn open(
        &self,
        arena: &mut Arena,
        _parent_ref: NodeRef,
        reader: &mut text::BasicReader,
        _ctx: &mut Context,
    ) -> Option<(NodeRef, State)> {
        let source = reader.source();
        let (saved_line, saved_pos) = reader.position();

        // ── Step 1: classify the top rule line ───────────────────────────────
        let (top_bytes, top_seg) = reader.peek_line_bytes()?;
        if top_seg.padding() > 0 {
            // Tab-expanded leading content — not supported in v1.
            return None;
        }
        let top_offset = reader.line_offset();
        let top_bytes = top_bytes.as_ref();
        let top_for_analysis = &top_bytes[..strip_newline(top_bytes)];

        let maybe_grouped = parse_column_rule(top_for_analysis, top_offset);
        let solid = maybe_grouped.is_none() && is_solid_rule(top_for_analysis, top_offset);

        if !solid && maybe_grouped.is_none() {
            return None; // not a dash rule at all
        }

        let with_header = solid;
        let table_pos = top_seg.start();

        // Advance past the top rule.
        reader.advance_line();

        // ── Step 2: collect header lines / resolve column extents ─────────────
        let (col_extents, header_segs): (Vec<(usize, usize)>, Vec<Segment>) = if with_header {
            // Collect header lines until we hit the grouped separator.
            let mut headers: Vec<Segment> = Vec::new();
            let cols = loop {
                let Some((lb, ls)) = reader.peek_line_bytes() else {
                    // EOF before separator.
                    reader.set_position(saved_line, saved_pos);
                    return None;
                };
                if ls.padding() > 0 {
                    reader.set_position(saved_line, saved_pos);
                    return None;
                }
                let lb = lb.as_ref();
                let la = &lb[..strip_newline(lb)];

                // Grouped dash rule → separator found.
                if let Some(cols) = parse_column_rule(la, reader.line_offset()) {
                    reader.advance_line(); // consume separator
                    break cols;
                }
                // Blank line inside the header section is not allowed.
                if is_blank(la) {
                    reader.set_position(saved_line, saved_pos);
                    return None;
                }
                headers.push(ls);
                reader.advance_line();
            };

            // A with-header table must have at least one header line.
            if headers.is_empty() {
                reader.set_position(saved_line, saved_pos);
                return None;
            }
            (cols, headers)
        } else {
            // Headerless: the top grouped rule IS the column rule.
            (maybe_grouped.unwrap(), Vec::new())
        };

        let num_cols = col_extents.len();

        // ── Step 3: collect body rows until the bottom dash rule ──────────────
        let mut body_rows: Vec<Vec<Segment>> = Vec::new();
        let mut current_row: Vec<Segment> = Vec::new();

        loop {
            let Some((lb, ls)) = reader.peek_line_bytes() else {
                // EOF without bottom rule.
                reader.set_position(saved_line, saved_pos);
                return None;
            };
            if ls.padding() > 0 {
                reader.set_position(saved_line, saved_pos);
                return None;
            }
            let lb = lb.as_ref();
            let la = &lb[..strip_newline(lb)];

            if is_any_dash_rule(la, reader.line_offset()) {
                // Bottom rule found.
                if !current_row.is_empty() {
                    body_rows.push(std::mem::take(&mut current_row));
                }
                reader.advance_to_eol(); // leave reader at EOL; outer loop calls advance_line()
                break;
            }

            if is_blank(la) {
                // Blank line separates body rows.
                if !current_row.is_empty() {
                    body_rows.push(std::mem::take(&mut current_row));
                }
                reader.advance_line();
                continue;
            }

            // Regular content line — accumulate into the current row.
            current_row.push(ls);
            reader.advance_line();
        }

        // A multiline table must have at least one body row.
        if body_rows.is_empty() {
            reader.set_position(saved_line, saved_pos);
            return None;
        }

        // ── Step 4: compute column alignments ────────────────────────────────
        // With-header: all header lines determine alignment.
        // Headerless:  only the first physical line of the first body row.
        let ref_lines: &[Segment] = if with_header {
            &header_segs
        } else {
            let first = body_rows[0].as_slice();
            &first[..1.min(first.len())]
        };
        let alignments = compute_alignments(source, &col_extents, ref_lines);

        // ── Step 5: build the AST ─────────────────────────────────────────────
        let table_ref = arena.new_node(Table::new());
        arena[table_ref].set_pos(table_pos);

        if with_header {
            let header_ref = arena.new_node(TableHeader::new());
            arena[header_ref].set_pos(table_pos);
            let row_ref = arena.new_node(TableRow::new());
            arena[row_ref].set_pos(table_pos);

            for (i, &(col_start, col_end)) in col_extents.iter().enumerate() {
                let is_last = i == num_cols - 1;
                let cell_ref = arena.new_node(TableCell::with_alignment(alignments[i]));
                populate_cell(
                    arena,
                    cell_ref,
                    source,
                    &header_segs,
                    col_start,
                    col_end,
                    is_last,
                );
                row_ref.append_child(arena, cell_ref);
            }

            header_ref.append_child(arena, row_ref);
            table_ref.append_child(arena, header_ref);
        }

        let body_ref = arena.new_node(TableBody::new());
        arena[body_ref].set_pos(table_pos);

        for row_lines in &body_rows {
            let row_ref = arena.new_node(TableRow::new());

            for (i, &(col_start, col_end)) in col_extents.iter().enumerate() {
                let is_last = i == num_cols - 1;
                let cell_ref = arena.new_node(TableCell::with_alignment(alignments[i]));
                populate_cell(
                    arena, cell_ref, source, row_lines, col_start, col_end, is_last,
                );
                row_ref.append_child(arena, cell_ref);
            }

            body_ref.append_child(arena, row_ref);
        }

        if arena[body_ref].first_child().is_some() {
            table_ref.append_child(arena, body_ref);
        }

        Some((table_ref, State::NO_CHILDREN))
    }

    fn cont(
        &self,
        _arena: &mut Arena,
        _node_ref: NodeRef,
        _reader: &mut text::BasicReader,
        _ctx: &mut Context,
    ) -> Option<State> {
        // The entire table is consumed eagerly in open().
        None
    }

    fn can_interrupt_paragraph(&self) -> bool {
        false
    }
}

impl From<MultilineTableParser> for AnyBlockParser {
    fn from(p: MultilineTableParser) -> Self {
        AnyBlockParser::Extension(Box::new(p))
    }
}

/// Returns a [`ParserExtension`] that adds pandoc-style multiline table
/// support to a rushdown parser.
///
/// The extension registers [`MultilineTableParser`] as a block parser at
/// priority **50**, ahead of setext headings (100) and thematic breaks (200).
/// It does not register any renderer extension — rushdown's built-in table
/// rendering handles the emitted `Table` / `TableHeader` / `TableBody` /
/// `TableRow` / `TableCell` nodes.
pub(crate) fn multiline_table_parser_extension() -> impl ParserExtension {
    ParserExtensionFn::new(|p: &mut Parser| {
        p.add_block_parser(MultilineTableParser::new, NoParserOptions, 50);
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rushdown::ast::{walk, WalkStatus};
    use rushdown::matches_kind;
    use rushdown::parser;
    use rushdown::renderer::html;
    use rushdown::text::BasicReader;
    use rushdown::Result;

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_parser() -> parser::Parser {
        parser::Parser::with_extensions(
            parser::Options {
                attributes: false,
                auto_heading_ids: false,
                ..Default::default()
            },
            parser::gfm(parser::GfmOptions::default()).and(multiline_table_parser_extension()),
        )
    }

    fn parse(input: &str) -> (Arena, NodeRef) {
        let p = make_parser();
        let mut reader = BasicReader::new(input);
        p.parse(&mut reader)
    }

    fn render(input: &str) -> String {
        let p = make_parser();
        let mut reader = BasicReader::new(input);
        let (arena, doc) = p.parse(&mut reader);
        let renderer =
            html::Renderer::with_extensions(html::Options::default(), html::NO_EXTENSIONS);
        let mut out = String::new();
        renderer.render(&mut out, input, &arena, doc).unwrap();
        out
    }

    /// Walk the AST and return the first node of the given kind.
    macro_rules! find_first {
        ($arena:expr, $doc:expr, $kind:ident) => {{
            let mut found: Option<NodeRef> = None;
            walk($arena, $doc, &mut |arena: &Arena,
                                     node_ref: NodeRef,
                                     entering: bool|
             -> Result<WalkStatus> {
                if entering && matches_kind!(arena[node_ref], $kind) && found.is_none() {
                    found = Some(node_ref);
                }
                Ok(WalkStatus::Continue)
            })
            .ok();
            found
        }};
    }

    /// Collect all children of a node that match the given kind.
    fn children_of_kind(arena: &Arena, parent: NodeRef, kind: &str) -> Vec<NodeRef> {
        let mut result = Vec::new();
        let mut child = arena[parent].first_child();
        while let Some(c) = child {
            if arena[c].kind_data().kind_name() == kind {
                result.push(c);
            }
            child = arena[c].next_sibling();
        }
        result
    }

    // ── Unit tests for helpers ────────────────────────────────────────────────

    #[test]
    fn solid_rule_detection() {
        assert!(is_solid_rule(b"---", 0));
        assert!(is_solid_rule(b"----------", 0));
        assert!(is_solid_rule(b"   ---", 0)); // indent 3 ok
        assert!(!is_solid_rule(b"--", 0)); // only 2 dashes
        assert!(!is_solid_rule(b"--- ---", 0)); // has internal space
        assert!(!is_solid_rule(b"    ---", 0)); // indent 4 → rejected
        assert!(!is_solid_rule(b"abc", 0));
        assert!(!is_solid_rule(b"", 0));
    }

    #[test]
    fn column_rule_parsing() {
        let groups = parse_column_rule(b"--- --- ---", 0).unwrap();
        assert_eq!(groups, vec![(0, 3), (4, 7), (8, 11)]);

        let groups = parse_column_rule(b"----------- ------- ---------------", 0).unwrap();
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0], (0, 11));
        assert_eq!(groups[1], (12, 19));
        assert_eq!(groups[2], (20, 35));

        // Solid rule → None (only one group)
        assert!(parse_column_rule(b"----------", 0).is_none());

        // Single group → None (< 2)
        assert!(parse_column_rule(b"---", 0).is_none());

        // Non-dash character → None
        assert!(parse_column_rule(b"--- : ---", 0).is_none());

        // Indent > 3 → None
        assert!(parse_column_rule(b"    --- ---", 0).is_none());
    }

    #[test]
    fn byte_offset_for_col_maps_display_columns_to_utf8_boundaries() {
        // ASCII: display column == byte offset.
        assert_eq!(byte_offset_for_col("hello", 0), 0);
        assert_eq!(byte_offset_for_col("hello", 3), 3);
        assert_eq!(byte_offset_for_col("hello", 5), 5);
        // Past end clamps to len.
        assert_eq!(byte_offset_for_col("hello", 99), 5);

        // Cyrillic: each char is 2 bytes UTF-8 but one display column.
        // "Выровнен" — 8 chars, 16 bytes.
        let s = "Выровнен";
        assert_eq!(byte_offset_for_col(s, 0), 0);
        assert_eq!(byte_offset_for_col(s, 1), 2);
        assert_eq!(byte_offset_for_col(s, 4), 8);
        assert_eq!(byte_offset_for_col(s, 8), 16);
        assert_eq!(byte_offset_for_col(s, 12), 16); // past end

        // Every returned offset must be a valid UTF-8 boundary.
        let s = "  Выровнен    По";
        for col in 0..=20 {
            let off = byte_offset_for_col(s, col);
            assert!(
                s.is_char_boundary(off),
                "col {col} -> byte {off} is not a char boundary in {s:?}"
            );
        }
    }

    #[test]
    fn cyrillic_table_does_not_corrupt_utf8() {
        // Regression for the bug where the parser sliced cell content by
        // *byte* offsets derived from the (ASCII) dash-rule line. With
        // Cyrillic content each Unicode character spans 2 UTF-8 bytes, so the
        // raw byte position landed in the middle of multi-byte characters and
        // produced invalid UTF-8 in the rendered output.
        let input = "\
---------------------------------------------------------------------
  Выровнен    По          Вправо  Влево
  по центру   умолчанию
------------ ----------- ------- ------------------------------------
  Первая      строка       12.0  Пример строки, которая
                                 занимает несколько физических строк.
---------------------------------------------------------------------
";
        let html = render(input);
        // The renderer's internal buffer must remain valid UTF-8.
        assert!(html.is_char_boundary(0));
        assert!(html.contains("<table>"));
        assert!(html.contains("<thead>"));
    }

    #[test]
    fn strip_newline_variants() {
        assert_eq!(strip_newline(b"hello\n"), 5);
        assert_eq!(strip_newline(b"hello\r\n"), 5);
        assert_eq!(strip_newline(b"hello"), 5);
        assert_eq!(strip_newline(b"\n"), 0);
        assert_eq!(strip_newline(b""), 0);
    }

    // ── Integration: parser tests ─────────────────────────────────────────────

    #[test]
    fn with_header_basic_structure() {
        let input = "\
-------------------------------------------------------------
 Centered   Default           Right Left
  Header    Aligned         Aligned Aligned
----------- ------- --------------- -------------------------
   First    row                12.0 Example of a row.

  Second    row                 5.0 Another row.
-------------------------------------------------------------
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table node");

        // Header
        let header = find_first!(&arena, table, TableHeader).expect("no TableHeader");
        let header_rows = children_of_kind(&arena, header, "TableRow");
        assert_eq!(header_rows.len(), 1, "exactly one header row");
        let header_cells = children_of_kind(&arena, header_rows[0], "TableCell");
        assert_eq!(header_cells.len(), 4, "four header cells");

        // Body
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let body_rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(body_rows.len(), 2, "two body rows");
        for row in &body_rows {
            let cells = children_of_kind(&arena, *row, "TableCell");
            assert_eq!(cells.len(), 4, "four body cells per row");
        }
    }

    #[test]
    fn headerless_basic_structure() {
        let input = "\
----------- ------- ---------------
   First    row              12.0

  Second    row               5.0
----------- ------- ---------------
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table node");

        // No header
        assert!(
            find_first!(&arena, table, TableHeader).is_none(),
            "headerless table must not have a TableHeader"
        );

        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 2);
        for row in &rows {
            let cells = children_of_kind(&arena, *row, "TableCell");
            assert_eq!(cells.len(), 3);
        }
    }

    /// Extract the inner content of the `n`-th `<td>` or `<th>` (zero-based,
    /// counting both kinds in document order). Returns the bytes between
    /// the opening tag's `>` and the matching `</td>`/`</th>`. The match
    /// requires the byte immediately after `td` / `th` to be whitespace,
    /// `>`, or `/`, so block-level wrappers like `<thead>` / `<tbody>`
    /// are not mistaken for cells.
    fn nth_cell_content(html: &str, n: usize) -> String {
        fn opens(tail: &str, name: &str) -> bool {
            if !tail.starts_with("<") || !tail[1..].starts_with(name) {
                return false;
            }
            let after = &tail[1 + name.len()..];
            after
                .chars()
                .next()
                .map(|c| c == '>' || c == '/' || c.is_ascii_whitespace())
                .unwrap_or(false)
        }

        let mut idx = 0;
        let mut cursor = 0;
        while let Some(rel) = html[cursor..].find('<') {
            let abs = cursor + rel;
            let tail = &html[abs..];
            let close_tag: &str = if opens(tail, "td") {
                "</td>"
            } else if opens(tail, "th") {
                "</th>"
            } else {
                cursor = abs + 1;
                continue;
            };
            let close_of_open = abs + tail.find('>').expect("malformed cell open tag");
            let content_start = close_of_open + 1;
            let content_end = content_start
                + html[content_start..]
                    .find(close_tag)
                    .expect("missing cell close tag");
            if idx == n {
                return html[content_start..content_end].to_string();
            }
            idx += 1;
            cursor = content_end + close_tag.len();
        }
        panic!("cell index {n} out of range in HTML");
    }

    /// Normalise multi-line cell content for comparison: trim, then
    /// collapse any internal whitespace (spaces and newlines) into a
    /// single space. Mirrors HTML rendering semantics, which treat soft
    /// line breaks (emitted as `\n`) and ASCII spaces equivalently.
    fn cell_text(html: &str, n: usize) -> String {
        let raw = nth_cell_content(html, n);
        raw.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn multiline_body_row_content_joined() {
        // Two physical lines belong to the same body row (no blank line between
        // them).  They must both appear in the same cell, joined by a soft
        // line break (rendered as a space in HTML).
        let input = "\
------------ ------------ ------------
row1col1     row1col2     row1col3
continued1   continued2   continued3
------------ ------------ ------------
";
        let html = render(input);
        // Both physical lines must appear in the output.
        assert!(html.contains("row1col1"), "row1col1 present: {html}");
        assert!(html.contains("continued1"), "continued1 present: {html}");
        // They must be in the SAME cell, not in separate rows.
        let td_open = html.find("<td").expect("no <td");
        let td_close = html.find("</td>").expect("no </td>");
        let first_cell = &html[td_open..td_close + 5];
        assert!(
            first_cell.contains("row1col1") && first_cell.contains("continued1"),
            "both content lines must be in the same first cell: {html}"
        );
    }

    #[test]
    fn multiline_header_content() {
        let input = "\
---------------------
 Header  Multi
  Line   Header
-------- -----------
 cell1   cell2
---------------------
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let header = find_first!(&arena, table, TableHeader).expect("no TableHeader");
        let row = children_of_kind(&arena, header, "TableRow");
        assert_eq!(row.len(), 1);
        let cells = children_of_kind(&arena, row[0], "TableCell");
        assert_eq!(cells.len(), 2, "two header cells");
    }

    #[test]
    fn multiple_body_rows() {
        let input = "\
----- -----
 r1c1  r1c2

 r2c1  r2c2

 r3c1  r3c2
----- -----
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn solid_rule_alone_falls_through_to_thematic_break() {
        // A solid dash rule NOT followed by header + separator + body + bottom
        // must NOT produce a Table (it should become a ThematicBreak).
        let input = "---\n";
        let html = render(input);
        assert!(html.contains("<hr"), "expected <hr>, got: {html}");
        assert!(!html.contains("<table"), "must not produce <table>: {html}");
    }

    #[test]
    fn solid_rule_without_body_falls_through() {
        // Solid top + header + separator, but NO body rows and NO bottom rule.
        let input = "\
----------
 Header
--- ---
";
        // The table parse should fail (no body, no bottom rule), leaving the
        // dashes to become a thematic break and the rest as a paragraph.
        let html = render(input);
        assert!(!html.contains("<table"), "must not produce <table>: {html}");
    }

    #[test]
    fn missing_bottom_rule_falls_through() {
        // Valid top structure but no closing dash rule → fall-through.
        let input = "\
----------
 Header
--- ---
 body text
";
        let html = render(input);
        assert!(!html.contains("<table"), "must not produce <table>: {html}");
    }

    #[test]
    fn alignment_center_right_left() {
        // Column 1: "Ctr" is inset on both sides → Center
        // Column 2: "Right" flush right → Right
        // Column 3: "Left" flush left → Left
        let input = "\
-----------  -------  -------
   Ctr         Right  Left
----------- -------- --------
 a           b        c
-----------  -------  -------
";
        let html = render(input);
        // Center column
        assert!(
            html.contains("text-align: center") || html.contains("align=\"center\""),
            "expected centered column: {html}"
        );
        // Right column
        assert!(
            html.contains("text-align: right") || html.contains("align=\"right\""),
            "expected right-aligned column: {html}"
        );
    }

    #[test]
    fn inline_markdown_in_cells_rendered() {
        // Inline markdown must be parsed inside table cells.
        // Columns are made wide enough to contain the markdown syntax.
        let input = "\
----------- ----- ------
**bold**    *em*  `code`

a           b     c
----------- ----- ------
";
        let html = render(input);
        assert!(html.contains("<strong>"), "bold in cell: {html}");
        assert!(html.contains("<em>"), "em in cell: {html}");
        assert!(html.contains("<code>"), "code in cell: {html}");
    }

    #[test]
    fn html_output_structure() {
        // A with-header table: solid top rule → header → grouped separator → body → solid bottom.
        let input = "\
----------
 A    B
----- -----
 1    2
----------
";
        let html = render(input);
        assert!(html.contains("<table"), "{html}");
        assert!(html.contains("<thead"), "{html}");
        assert!(html.contains("<tbody"), "{html}");
        assert!(html.contains("<th"), "{html}");
        assert!(html.contains("<td"), "{html}");
    }

    #[test]
    fn indented_rule_up_to_three_spaces_accepted() {
        // Up to 3 leading spaces are allowed on the rule lines.
        let input = "\
   ----- -----
   A     B
   ----- -----
   1     2
   ----- -----
";
        let html = render(input);
        assert!(
            html.contains("<table"),
            "expected table with 3-space indent: {html}"
        );
    }

    #[test]
    fn single_row_table() {
        let input = "\
----- -----
 A     B
----- -----
 1     2
----- -----
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn cell_content_extracted_correctly() {
        let input = "\
---------- ----------
 Heading1   Heading2
---------- ----------
 Value1     Value2
---------- ----------
";
        let html = render(input);
        assert!(html.contains("Heading1"), "Heading1 in output: {html}");
        assert!(html.contains("Heading2"), "Heading2 in output: {html}");
        assert!(html.contains("Value1"), "Value1 in output: {html}");
        assert!(html.contains("Value2"), "Value2 in output: {html}");
    }

    #[test]
    fn does_not_consume_paragraph_after_table() {
        let input = "\
----- -----
 A     B
----- -----
 1     2
----- -----

Following paragraph.
";
        let html = render(input);
        assert!(html.contains("<table"), "table present: {html}");
        assert!(
            html.contains("Following paragraph"),
            "paragraph after table: {html}"
        );
    }

    // ── Pandoc-parity regression tests ──────────────────────────────────────
    //
    // Each test below pairs a markdown sample drawn from the docbake
    // user documentation with the **exact** content pandoc 3.x emits for
    // every `<th>` / `<td>` (in document order) when run with
    // `pandoc --wrap=none -f markdown -t html`. The cell content is
    // compared after collapsing all internal whitespace, which is the
    // semantically meaningful unit: HTML treats soft line breaks (`\n`)
    // and ASCII spaces equivalently, so a docbake-emitted
    // `"Centered\nHeader"` is equivalent to pandoc's `"Centered Header"`.

    #[test]
    fn pandoc_parity_with_header_russian() {
        // Sample from doc/ru/markdown/tables-lists.md "Таблица с заголовком".
        let input = "\
---------------------------------------------------------------------
  Выровнен    По          Вправо  Влево
  по центру   умолчанию
------------ ----------- ------- ------------------------------------
  Первая      строка       12.0  Пример строки, которая
                                 занимает несколько физических строк.

  Вторая      строка        5.0  Ещё одна строка. Обратите внимание
                                 на пустую строку между строками.
---------------------------------------------------------------------
";
        let html = render(input);

        // Header row.
        assert_eq!(cell_text(&html, 0), "Выровнен по центру");
        assert_eq!(cell_text(&html, 1), "По умолчанию");
        assert_eq!(cell_text(&html, 2), "Вправо");
        assert_eq!(cell_text(&html, 3), "Влево");

        // Body row 1.
        assert_eq!(cell_text(&html, 4), "Первая");
        assert_eq!(cell_text(&html, 5), "строка");
        assert_eq!(cell_text(&html, 6), "12.0");
        assert_eq!(
            cell_text(&html, 7),
            "Пример строки, которая занимает несколько физических строк."
        );

        // Body row 2.
        assert_eq!(cell_text(&html, 8), "Вторая");
        assert_eq!(cell_text(&html, 9), "строка");
        assert_eq!(cell_text(&html, 10), "5.0");
        assert_eq!(
            cell_text(&html, 11),
            "Ещё одна строка. Обратите внимание на пустую строку между строками."
        );
    }

    #[test]
    fn pandoc_parity_headerless_russian() {
        // Sample from doc/ru/markdown/tables-lists.md "Таблица без заголовка".
        let input = "\
------------ ----------- ------- ------------------------------------
  Первая      строка       12.0  Пример многострочной строки без
                                 заголовка.

  Вторая      строка        5.0  Ещё одна строка.
------------ ----------- ------- ------------------------------------
";
        let html = render(input);

        // No <thead> in a headerless table.
        assert!(
            !html.contains("<thead"),
            "headerless table must not emit <thead>: {html}"
        );

        // Body row 1.
        assert_eq!(cell_text(&html, 0), "Первая");
        assert_eq!(cell_text(&html, 1), "строка");
        assert_eq!(cell_text(&html, 2), "12.0");
        assert_eq!(
            cell_text(&html, 3),
            "Пример многострочной строки без заголовка."
        );

        // Body row 2.
        assert_eq!(cell_text(&html, 4), "Вторая");
        assert_eq!(cell_text(&html, 5), "строка");
        assert_eq!(cell_text(&html, 6), "5.0");
        assert_eq!(cell_text(&html, 7), "Ещё одна строка.");
    }

    #[test]
    fn pandoc_parity_canonical_example() {
        // The canonical sample from the pandoc manual
        // (https://pandoc.org/demo/example33/8.9-tables.html).
        let input = "\
-------------------------------------------------------------
 Centered   Default           Right Left
  Header    Aligned         Aligned Aligned
----------- ------- --------------- -------------------------
   First    row                12.0 Example of a row that
                                    spans multiple lines.

  Second    row                 5.0 Here's another one. Note
                                    the blank line between
                                    rows.
-------------------------------------------------------------
";
        let html = render(input);

        // Header.
        assert_eq!(cell_text(&html, 0), "Centered Header");
        assert_eq!(cell_text(&html, 1), "Default Aligned");
        assert_eq!(cell_text(&html, 2), "Right Aligned");
        assert_eq!(cell_text(&html, 3), "Left Aligned");

        // Body row 1.
        assert_eq!(cell_text(&html, 4), "First");
        assert_eq!(cell_text(&html, 5), "row");
        assert_eq!(cell_text(&html, 6), "12.0");
        assert_eq!(
            cell_text(&html, 7),
            "Example of a row that spans multiple lines."
        );

        // Body row 2.
        assert_eq!(cell_text(&html, 8), "Second");
        assert_eq!(cell_text(&html, 9), "row");
        assert_eq!(cell_text(&html, 10), "5.0");
        assert_eq!(
            cell_text(&html, 11),
            "Here's another one. Note the blank line between rows."
        );
    }

    #[test]
    fn no_column_spillage_across_lines() {
        // Regression guard for the bug where intermediate physical lines
        // of a multi-line cell incorrectly included bytes from later
        // columns. Each cell must contain only its own column slice.
        let input = "\
----------- ----------- -----------
 A-cont      B-cont      C-cont
 A-second    B-second    C-second
----------- ----------- -----------
";
        let html = render(input);

        // A cells: must NOT contain B-* or C-*.
        let a = cell_text(&html, 0);
        assert_eq!(a, "A-cont A-second");
        assert!(!a.contains("B-"), "col A spilled into col B: {a:?}");
        assert!(!a.contains("C-"), "col A spilled into col C: {a:?}");

        // B cells.
        let b = cell_text(&html, 1);
        assert_eq!(b, "B-cont B-second");
        assert!(!b.contains("A-"));
        assert!(!b.contains("C-"));

        // C cells (last column).
        let c = cell_text(&html, 2);
        assert_eq!(c, "C-cont C-second");
        assert!(!c.contains("A-"));
        assert!(!c.contains("B-"));
    }

    #[test]
    fn three_physical_lines_in_one_cell() {
        // Ensures the soft-line-break pattern repeats correctly for a
        // cell that spans three physical lines. Headerless table; body
        // rows are separated by a blank line (a dash rule inside the
        // body would close the table).
        let input = "\
------ -------------------------
 head   one
        two
        three

 a      x y z
        more text
        even more
------ -------------------------
";
        let html = render(input);
        assert_eq!(cell_text(&html, 0), "head");
        assert_eq!(cell_text(&html, 1), "one two three");
        assert_eq!(cell_text(&html, 2), "a");
        assert_eq!(cell_text(&html, 3), "x y z more text even more");
    }
}
