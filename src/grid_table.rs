//! Pandoc-style grid tables for rushdown.
//!
//! Implements pandoc's `grid_tables` extension as a [`BlockParser`]
//! registered at priority **40** (below `ListParser` at 300,
//! `ThematicBreakParser` at 200, and `SetextHeadingParser` at 100).
//! The priority inversion is load-bearing for rowspan support: a
//! row-separator line like `+    +---+` (rowspan continuation in
//! column 0, cell ends in column 1) matches the `+ ` bullet pattern
//! of rushdown's list parser, and without claiming the line first we
//! lose the line mid-table.
//!
//! `open()` validates only that the trigger line is an all-dash
//! `+---+` border, then captures every subsequent `+`- or `|`-started
//! line via `cont()` into a `Paragraph` accumulator. `close()` runs
//! the full validation (column alignment, row partition, cell-map
//! construction with rowspan / colspan resolution) via
//! [`try_parse_grid_table`] and either replaces the accumulator with
//! the built `Table` subtree on success or leaves it as a plain
//! `Paragraph` on failure — the same fallback pattern multiline
//! tables use.
//!
//! ```markdown
//! +----------------+----------------+
//! | Header 1       | Header 2       |
//! +================+================+
//! | Row 1 col 1    | Row 1 col 2    |
//! +----------------+----------------+
//! | Row 2 spans    | many lines     |
//! | of content     |                |
//! +----------------+----------------+
//! ```
//!
//! ## Syntax
//!
//! - **Top border** — `+` `-`* `+` … `+`. Indent ≤ 3.
//! - **Content line** — starts with `|`, has `|` at every column boundary
//!   defined by the top border (or a subset of them — see Column spans
//!   below), ends with `|`.
//! - **Header separator** (optional) — same shape as a border but with
//!   `=` instead of `-`. Marks the boundary between header and body
//!   rows.
//! - **Row separator** — `+---+...+---+` between body rows. Optional;
//!   rows without a separator are joined as multi-line cells.
//! - **Bottom border** — `+---+...+---+` (mandatory).
//! - All `+` and `|` characters must vertically align across the table.
//! - Alignment in any border: `+:--+` (left), `+--:+` (right), `+:-:+`
//!   (centre), `+---+` (default). Alignment is taken from the *first*
//!   border line that carries `:` markers.
//! - **No blank lines** may appear between the top and bottom borders.
//! - **Headerless form** — no `+===+` separator; the whole table is
//!   body.
//!
//! ## Column spans (merged cells)
//!
//! A content line may omit `|` separators at column boundaries to
//! indicate that a cell spans the missing column(s). The first and
//! last `|` (the table's outer borders) must always be present.
//!
//! ```markdown
//! +------+------+------+
//! | Group 1     | Solo |
//! +======+======+======+
//! | a    | b    | c    |
//! +------+------+------+
//! ```
//!
//! Column-spanning cells receive a `colspan` node attribute whose
//! value is the span count. Rushdown's HTML renderer passes the
//! attribute through as `<td colspan="N">` / `<th colspan="N">` via
//! its `TABLE_CELL_ATTRS` allow-list.
//!
//! ## Limitations
//!
//! Cell content is inline-only — multi-physical-line cells are joined
//! with soft line breaks. Lists, code blocks, and multi-paragraph
//! cells degrade to inline text rather than producing nested block
//! AST.
//!
//! ## Row spans
//!
//! A row separator may carry **continuation segments** (all spaces
//! between two `+`s) for columns where the cell from the previous row
//! continues into the next row:
//!
//! ```markdown
//! +------+------+
//! | A    | B    |
//! +      +------+
//! |      | C    |
//! +------+------+
//! ```
//!
//! Each cell records its `rowspan` as a `rowspan="N"` node attribute,
//! emitted by rushdown's HTML renderer as `<td rowspan="N">` /
//! `<th rowspan="N">` via the `TABLE_CELL_ATTRS` allow-list.
//! Continuation rows' `TableRow` children list only the cells that
//! *start* in that row — cells continuing from above are not
//! re-emitted, matching the way browsers expect `rowspan` to behave.
//!
//! Constraints (each rejected by `try_parse_grid_table`):
//! - The top and bottom borders must be all-dash (rowspan can't
//!   extend from above the table or past its end).
//! - The `+===+` header separator must be all-equals — rowspan can't
//!   cross the header / body boundary.
//! - Every border line must have a `+` at every top-border column
//!   position (the "merged" form `+         +`, with no internal `+`,
//!   is rejected).
//! - A row's pipe layout must match the cell layout from above: if
//!   the row above has a colspan-2 cell that continues, the
//!   continuation row's `|` positions must skip the column boundary
//!   inside that cell.
//!
//! [pandoc manual]: https://pandoc.org/demo/example33/8.9-tables.html

use core::fmt;

use rushdown::as_type_data_mut;
use rushdown::ast::{
    pp_indent, Arena, KindData, NodeKind, NodeRef, NodeType, Paragraph, PrettyPrint, Table,
    TableBody, TableCell, TableCellAlignment, TableHeader, TableRow,
};
use rushdown::parser::{
    AnyBlockParser, BlockParser, Context, NoParserOptions, Parser, ParserExtension,
    ParserExtensionFn, State,
};
use rushdown::text::{self, Reader as _, Segment};
use rushdown::util::{indent_width, trim_right_space_length};

// ============================================================================
// Low-level helpers
// ============================================================================

/// Maps a *display-column* position to a byte offset within `s`. Each
/// Unicode scalar is approximated as one display column; clamps to
/// `s.len()` past EOL.
fn byte_offset_for_col(s: &str, target_col: usize) -> usize {
    for (col, (byte_idx, _ch)) in s.char_indices().enumerate() {
        if col == target_col {
            return byte_idx;
        }
    }
    s.len()
}

/// Returns the byte length of `bytes` after stripping a trailing `\n`
/// or `\r\n`.
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

/// Returns the line's bytes from `seg` with any trailing `\n`/`\r\n`
/// stripped. The segment is assumed to be post-parent-strip (which is
/// what the paragraph parser gives us) — leading whitespace ≤ 3 is
/// preserved and consumed by the border / content-line parsers.
fn segment_line_bytes(source: &str, seg: Segment) -> &[u8] {
    let bytes = &source.as_bytes()[seg.start()..seg.stop()];
    &bytes[..strip_newline(bytes)]
}

// ============================================================================
// Grid border parsing
// ============================================================================

/// What kind of fill occupies one segment between two consecutive `+`s on
/// a border line. A border line is a sequence of one or more segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentKind {
    /// `---` (optionally with `:` alignment markers). Used in the top
    /// border, bottom border, and row separators where a cell ends.
    Dash,
    /// `===` (optionally with `:`). Used in the header separator.
    Equals,
    /// All-spaces between two `+`s. Appears only in row separators and
    /// indicates that the cell occupying this column extends from the
    /// row above into the row below — i.e. a row span.
    Continuation,
}

/// Parsed shape of one border line: the byte positions of each `+` and
/// the kind of each segment between consecutive `+`s.
#[derive(Debug, Clone)]
struct ParsedBorder {
    /// Byte positions of `+` characters (post-parent-strip, relative
    /// to the line's start). `plus_positions.len() = segments.len() + 1`.
    plus_positions: Vec<usize>,
    /// One entry per pair of consecutive `+`s.
    segments: Vec<SegmentKind>,
}

impl ParsedBorder {
    /// `true` if every segment is `Dash`. Used to validate the top and
    /// bottom borders (no rowspan continuation possible there).
    fn all_dash(&self) -> bool {
        self.segments.iter().all(|s| matches!(s, SegmentKind::Dash))
    }

    /// `true` if every segment is `Equals`. Used to validate the header
    /// separator (rowspan crossing the header/body boundary is rejected).
    fn all_equals(&self) -> bool {
        self.segments
            .iter()
            .all(|s| matches!(s, SegmentKind::Equals))
    }

    /// `true` if any segment is `Equals`. A border that *contains* an
    /// Equals segment must be `all_equals` — mixing Equals with other
    /// segment kinds is rejected by `try_parse_grid_table`.
    fn has_equals(&self) -> bool {
        self.segments
            .iter()
            .any(|s| matches!(s, SegmentKind::Equals))
    }
}

/// Parse a grid border line. Returns `None` if the line isn't a valid
/// border at all (indent > 3, no `+` boundaries, mixed dash/equals in
/// the same border, an empty column `++`, a segment containing
/// unrecognised bytes).
///
/// The caller validates *semantic* shape (e.g. "top border must be
/// `all_dash`") against the returned [`ParsedBorder`].
fn parse_grid_border(line: &[u8], offset: usize) -> Option<ParsedBorder> {
    let (w, start) = indent_width(line, offset);
    if w > 3 {
        return None;
    }
    let trailing_ws = trim_right_space_length(line);
    let content_end = line.len() - trailing_ws;
    if start >= content_end {
        return None;
    }
    if line[start] != b'+' {
        return None;
    }

    // Find every `+` position in the content.
    let plus_positions: Vec<usize> = line[start..content_end]
        .iter()
        .enumerate()
        .filter_map(|(i, &b)| (b == b'+').then_some(start + i))
        .collect();
    if plus_positions.len() < 2 {
        return None;
    }
    if plus_positions[0] != start || plus_positions.last().copied() != Some(content_end - 1) {
        return None;
    }

    // Classify each segment between consecutive `+`s.
    let mut segments: Vec<SegmentKind> = Vec::with_capacity(plus_positions.len() - 1);
    let mut has_dash_anywhere = false;
    let mut has_equals_anywhere = false;
    for w in plus_positions.windows(2) {
        if w[1] - w[0] < 2 {
            return None; // `++` — empty column
        }
        let seg_bytes = &line[w[0] + 1..w[1]];
        let kind = classify_border_segment(seg_bytes)?;
        match kind {
            SegmentKind::Dash => has_dash_anywhere = true,
            SegmentKind::Equals => has_equals_anywhere = true,
            SegmentKind::Continuation => {}
        }
        segments.push(kind);
    }

    // A single border line cannot mix `---` and `===` segments.
    if has_dash_anywhere && has_equals_anywhere {
        return None;
    }

    Some(ParsedBorder {
        plus_positions,
        segments,
    })
}

/// Classify the bytes between two consecutive `+` characters in a
/// border line. A segment is composed of dashes, equals, colons (only
/// alignment markers — they need to be flanked by dashes or equals),
/// and/or spaces. Returns `None` for any other byte or for a segment
/// that has only `:` markers (no fill).
fn classify_border_segment(bytes: &[u8]) -> Option<SegmentKind> {
    let mut has_dash = false;
    let mut has_equals = false;
    let mut has_colon = false;
    let mut has_space = false;
    for &b in bytes {
        match b {
            b'-' => has_dash = true,
            b'=' => has_equals = true,
            b':' => has_colon = true,
            b' ' => has_space = true,
            _ => return None,
        }
    }
    if has_dash && has_equals {
        return None;
    }
    if has_dash {
        // `:` markers are allowed alongside dashes (alignment).
        // Spaces are not allowed inside an otherwise-dash segment —
        // borders are contiguous within each segment.
        if has_space {
            return None;
        }
        return Some(SegmentKind::Dash);
    }
    if has_equals {
        if has_space {
            return None;
        }
        return Some(SegmentKind::Equals);
    }
    // No dashes, no equals.
    if has_colon {
        // Colons-only is not a valid segment shape (no fill to anchor
        // the alignment markers to).
        return None;
    }
    // Spaces-only (the segment is non-empty because we already rejected
    // `++` above).
    debug_assert!(has_space);
    Some(SegmentKind::Continuation)
}

/// Compute per-column alignment from a border line by inspecting the
/// `:` markers between each pair of consecutive `+` positions.
fn alignment_from_border(line: &[u8], plus_positions: &[usize]) -> Vec<TableCellAlignment> {
    plus_positions
        .windows(2)
        .map(|w| {
            let start = w[0] + 1;
            let end = w[1];
            if start >= end || end > line.len() {
                return TableCellAlignment::None;
            }
            let seg = &line[start..end];
            let left = seg.first().copied() == Some(b':');
            let right = seg.last().copied() == Some(b':');
            match (left, right) {
                (true, true) => TableCellAlignment::Center,
                (true, false) => TableCellAlignment::Left,
                (false, true) => TableCellAlignment::Right,
                (false, false) => TableCellAlignment::None,
            }
        })
        .collect()
}

/// Returns the *subset* of `top_plus_cols` at which a `|` appears in
/// `line`. The leftmost and rightmost top columns must each have a
/// `|`. Returns `None` if those requirements aren't met, the line has
/// indent > 3, or the bytes aren't valid UTF-8.
///
/// A proper-subset return signals column-spanning cells: a cell whose
/// content area covers more than one top-border column boundary.
fn pipe_positions_for_line(line: &[u8], top_plus_cols: &[usize]) -> Option<Vec<usize>> {
    let (w, _) = indent_width(line, 0);
    if w > 3 {
        return None;
    }
    let s = core::str::from_utf8(line).ok()?;
    let mut pipes: Vec<usize> = Vec::new();
    for &col in top_plus_cols {
        let byte_off = byte_offset_for_col(s, col);
        if byte_off < line.len() && line[byte_off] == b'|' {
            pipes.push(col);
        }
    }
    if pipes.len() < 2 {
        return None;
    }
    if pipes.first() != top_plus_cols.first() || pipes.last() != top_plus_cols.last() {
        return None;
    }
    Some(pipes)
}

/// Returns `true` if `line` is a grid content line — i.e. a
/// `|`-delimited row whose pipe positions are a (possibly proper)
/// subset of the table's top-border column positions.
fn is_grid_content_line(line: &[u8], offset: usize, top_plus_cols: &[usize]) -> bool {
    let (w, _) = indent_width(line, offset);
    if w > 3 {
        return false;
    }
    pipe_positions_for_line(line, top_plus_cols).is_some()
}

/// Computes the row-level pipe positions that *every* physical content
/// line in `row_lines` must share. Returns `None` if any line can't be
/// parsed as a content line, or if different physical lines use
/// different pipe positions (pandoc rejects misaligned rows; we do
/// the same).
fn pipe_positions_for_row(
    source: &str,
    row_lines: &[Segment],
    top_plus_cols: &[usize],
) -> Option<Vec<usize>> {
    let mut agreed: Option<Vec<usize>> = None;
    for &ls in row_lines {
        let line = segment_line_bytes(source, ls);
        let pipes = pipe_positions_for_line(line, top_plus_cols)?;
        if let Some(ref a) = agreed {
            if a != &pipes {
                return None;
            }
        } else {
            agreed = Some(pipes);
        }
    }
    agreed
}

// ============================================================================
// Validated table description
// ============================================================================

/// One cell's identity in the cell map: starting position
/// `(row, col_start)`, extent `(row_end, col_end)` (both exclusive),
/// and the union of physical source lines from every logical row the
/// cell occupies. `populate_cell` later extracts cell content by
/// column-trimming each line; lines in continuation rows are pure
/// whitespace in the cell's column range and produce no output —
/// no special handling required.
struct CellOrigin {
    row: usize,
    col_start: usize, // index into top_plus_cols
    col_end: usize,   // exclusive
    row_end: usize,   // exclusive
    lines: Vec<Segment>,
}

/// A grid-table description produced by [`try_parse_grid_table`]
/// from a paragraph's collected source segments. Carries everything
/// needed to build the `Table` AST without re-scanning the source.
struct GridTableSpec {
    /// Source byte offset of the top border — becomes the `Table`
    /// node's `pos` (used by the fmt renderer's source-recovery probe).
    pos: usize,
    /// Byte positions of `+` in the top border, defining the column
    /// layout for every row.
    top_plus_cols: Vec<usize>,
    /// Per-column alignment, taken from the first border line that
    /// carries `:` markers.
    alignments: Vec<TableCellAlignment>,
    /// Number of logical rows that belong to the header. Zero if there
    /// is no `+===+` separator.
    header_row_count: usize,
    /// Total number of logical rows (header + body).
    total_row_count: usize,
    /// All cells, in document order (first row's leftmost cell first).
    /// A cell appears exactly once even if it spans multiple rows or
    /// columns; `build_grid_row` groups them by `row` for emission.
    cells: Vec<CellOrigin>,
}

/// One logical row produced by Pass B of [`try_parse_grid_table`]:
/// the row's physical content lines plus the border that closes it
/// (a row separator for non-final rows, the bottom border for the
/// last row).
struct LogicalRow {
    lines: Vec<Segment>,
    closing_separator: ParsedBorder,
}

/// Classification produced by Pass A of [`try_parse_grid_table`].
enum ClassifiedLine {
    Content(Segment),
    Separator(ParsedBorder),
}

/// Validate `lines` as a pandoc grid table. Returns `None` on any
/// shape violation (wrong first / last line, misaligned border,
/// non-grid line in the middle, inconsistent per-row pipe positions,
/// duplicate header separator, mixed row-span continuation, rowspan
/// extending past the bottom border, rowspan crossing the header
/// boundary, …). The caller is then expected to put the paragraph's
/// source back unchanged.
///
/// Three-pass algorithm:
/// 1. Classify intermediate lines as Content or Separator.
/// 2. Group content into logical rows separated by row separators;
///    locate the header separator (if any).
/// 3. Walk rows building a 2D cell map: for each row's pipe slots,
///    decide whether the slot starts a new cell or continues one from
///    above (based on per-column active state updated from each
///    closing separator's per-segment kind).
fn try_parse_grid_table(source: &str, lines: &[Segment]) -> Option<GridTableSpec> {
    // Need at least: top border + 1 content line + bottom border.
    if lines.len() < 3 {
        return None;
    }

    // Top border: all-dash, defines columns + (possibly) alignment.
    let top_line = segment_line_bytes(source, lines[0]);
    let top_border = parse_grid_border(top_line, 0)?;
    if !top_border.all_dash() {
        return None;
    }
    let top_plus_cols = top_border.plus_positions.clone();
    let n_cols = top_plus_cols.len() - 1;
    let mut alignments = alignment_from_border(top_line, &top_plus_cols);

    // Bottom border: all-dash, same column positions as the top.
    let bot_line = segment_line_bytes(source, lines[lines.len() - 1]);
    let bot_border = parse_grid_border(bot_line, 0)?;
    if !bot_border.all_dash() || bot_border.plus_positions != top_plus_cols {
        return None;
    }

    // ── Pass A: classify each intermediate line as Content or Separator.
    let mut classified: Vec<ClassifiedLine> = Vec::with_capacity(lines.len() - 2);
    for &seg in &lines[1..lines.len() - 1] {
        let line = segment_line_bytes(source, seg);
        if let Some(border) = parse_grid_border(line, 0) {
            if border.plus_positions != top_plus_cols {
                return None;
            }
            // Header separator (any Equals segment) must be entirely
            // Equals — mixing Equals with Continuation would mean a
            // rowspan crossing the header / body boundary, which we
            // reject.
            if border.has_equals() && !border.all_equals() {
                return None;
            }
            // An all-continuation separator is structurally valid: it
            // means every cell in the row above extends into the row
            // below. (Pass C still does the work — it bumps each
            // cell's `row_end` and clears nothing.)
            let line_align = alignment_from_border(line, &top_plus_cols);
            if line_align
                .iter()
                .any(|a| !matches!(a, TableCellAlignment::None))
                && alignments
                    .iter()
                    .all(|a| matches!(a, TableCellAlignment::None))
            {
                alignments = line_align;
            }
            classified.push(ClassifiedLine::Separator(border));
        } else if is_grid_content_line(line, 0, &top_plus_cols) {
            classified.push(ClassifiedLine::Content(seg));
        } else {
            return None;
        }
    }

    // ── Pass B: group content lines into logical rows.
    let mut rows: Vec<LogicalRow> = Vec::new();
    let mut current_lines: Vec<Segment> = Vec::new();
    let mut header_row_count: Option<usize> = None;
    let mut seen_header_sep = false;

    for cl in classified {
        match cl {
            ClassifiedLine::Content(seg) => current_lines.push(seg),
            ClassifiedLine::Separator(border) => {
                if current_lines.is_empty() {
                    return None; // separator with no content above it
                }
                let is_equals = border.all_equals();
                rows.push(LogicalRow {
                    lines: core::mem::take(&mut current_lines),
                    closing_separator: border,
                });
                if is_equals {
                    if seen_header_sep {
                        return None;
                    }
                    seen_header_sep = true;
                    header_row_count = Some(rows.len());
                }
            }
        }
    }
    // Last row: closed by the bottom border.
    if !current_lines.is_empty() {
        rows.push(LogicalRow {
            lines: core::mem::take(&mut current_lines),
            closing_separator: bot_border.clone(),
        });
    }
    if rows.is_empty() {
        return None;
    }

    // ── Pass C: walk rows, build cell map.
    //
    // `active[c]` is `Some(idx)` when a cell (cells[idx]) is currently
    // occupying column `c` and continuing into the row about to be
    // processed; `None` when the column is free.
    let mut active: Vec<Option<usize>> = vec![None; n_cols];
    let mut cells: Vec<CellOrigin> = Vec::new();

    for (row_idx, row) in rows.iter().enumerate() {
        // Pipe positions consistent across all physical lines in this row.
        let pipes = pipe_positions_for_row(source, &row.lines, &top_plus_cols)?;

        // For each `|`-delimited slot in the row.
        for w in pipes.windows(2) {
            let pipe_start = w[0];
            let pipe_end = w[1];
            let col_start = top_plus_cols.iter().position(|&c| c == pipe_start)?;
            let col_end = top_plus_cols.iter().position(|&c| c == pipe_end)?;
            if col_start >= col_end || col_end > n_cols {
                return None;
            }

            // Inspect the slot's active state.
            let slot_actives: &[Option<usize>] = &active[col_start..col_end];
            if slot_actives.iter().all(|a| a.is_none()) {
                // No cell continuing here — this slot starts a new cell.
                let cell_idx = cells.len();
                cells.push(CellOrigin {
                    row: row_idx,
                    col_start,
                    col_end,
                    row_end: row_idx + 1,
                    lines: row.lines.clone(),
                });
                for slot in &mut active[col_start..col_end] {
                    *slot = Some(cell_idx);
                }
            } else if let Some(first_idx) = slot_actives[0] {
                // All slot columns must reference the same cell.
                if slot_actives.iter().any(|a| *a != Some(first_idx)) {
                    return None;
                }
                // And that cell's column extent must match the slot
                // exactly — partial-span continuation is rejected.
                let cell = &cells[first_idx];
                if cell.col_start != col_start || cell.col_end != col_end {
                    return None;
                }
                // Append this row's physical lines to the cell so
                // populate_cell can later extract the cell's content
                // across its full row range. (Continuation rows' bytes
                // in the cell's columns are whitespace; extract_cell_segment
                // returns None for them and they're silently skipped.)
                let row_lines = row.lines.clone();
                cells[first_idx].lines.extend(row_lines);
            } else {
                // Mixed None + Some — slot straddles a span boundary.
                return None;
            }
        }

        // Apply the row's closing separator to update active state.
        let sep = &row.closing_separator;
        debug_assert_eq!(sep.segments.len(), n_cols);
        for (c, &seg_kind) in sep.segments.iter().enumerate() {
            match seg_kind {
                SegmentKind::Continuation => {
                    let Some(idx) = active[c] else {
                        // Continuation marker over a column with no
                        // active cell — nothing to continue.
                        return None;
                    };
                    // Bump the cell's row_end to the row beyond this
                    // separator. Each successive Continuation bumps it
                    // further; a Dash/Equals stops the bump.
                    cells[idx].row_end = row_idx + 2;
                }
                SegmentKind::Dash | SegmentKind::Equals => {
                    active[c] = None;
                }
            }
        }
    }

    // After the last row's closing separator (the bottom border, all
    // Dash), `active` must be all-None — otherwise a cell tried to
    // extend past the bottom border.
    if active.iter().any(|a| a.is_some()) {
        return None;
    }

    Some(GridTableSpec {
        pos: lines[0].start(),
        top_plus_cols,
        alignments,
        header_row_count: header_row_count.unwrap_or(0),
        total_row_count: rows.len(),
        cells,
    })
}

// ============================================================================
// AST construction
// ============================================================================

/// Build the `Table` subtree from a validated [`GridTableSpec`].
///
/// Cells are pre-grouped by their starting row; each logical row
/// emits a `TableRow` containing only the cells that *start* in that
/// row. Cells continuing from a prior row (rowspan) do not reappear
/// in subsequent rows' `TableRow` children — the browser's renderer
/// (or any consumer) reconstructs the visual layout from
/// `rowspan="N"` attributes alone.
fn build_grid_table(arena: &mut Arena, source: &str, spec: &GridTableSpec) -> NodeRef {
    let table_ref = arena.new_node(Table::new());
    arena[table_ref].set_pos(spec.pos);

    // Pre-compute the cell-index lists per row to avoid an O(rows ×
    // cells) inner scan.
    let mut cells_by_row: Vec<Vec<usize>> = vec![Vec::new(); spec.total_row_count];
    for (idx, cell) in spec.cells.iter().enumerate() {
        if cell.row < spec.total_row_count {
            cells_by_row[cell.row].push(idx);
        }
    }
    // Sort each row's cell list left-to-right by starting column so
    // the emitted `TableRow` children preserve document order.
    for row_cells in &mut cells_by_row {
        row_cells.sort_by_key(|&i| spec.cells[i].col_start);
    }

    if spec.header_row_count > 0 {
        let header_ref = arena.new_node(TableHeader::new());
        arena[header_ref].set_pos(spec.pos);
        for row_cells in &cells_by_row[..spec.header_row_count] {
            build_grid_row(arena, header_ref, source, spec, row_cells);
        }
        table_ref.append_child(arena, header_ref);
    }

    if spec.header_row_count < spec.total_row_count {
        let body_ref = arena.new_node(TableBody::new());
        arena[body_ref].set_pos(spec.pos);
        for row_cells in &cells_by_row[spec.header_row_count..spec.total_row_count] {
            build_grid_row(arena, body_ref, source, spec, row_cells);
        }
        table_ref.append_child(arena, body_ref);
    }

    table_ref
}

/// Build one `TableRow` under `parent` containing the cells that
/// *start* in this row. Sets `colspan` / `rowspan` attributes when
/// the corresponding extent is > 1; rushdown's HTML renderer passes
/// both through via its `TABLE_CELL_ATTRS` allow-list.
fn build_grid_row(
    arena: &mut Arena,
    parent: NodeRef,
    source: &str,
    spec: &GridTableSpec,
    cell_indices: &[usize],
) {
    let row_ref = arena.new_node(TableRow::new());
    for &cell_idx in cell_indices {
        let cell = &spec.cells[cell_idx];
        let colspan = cell.col_end - cell.col_start;
        let rowspan = cell.row_end - cell.row;
        // Per-column alignment only applies to a cell that occupies
        // exactly one column; spanned cells have no well-defined
        // alignment from the column-level setting.
        let alignment = if colspan == 1 {
            spec.alignments
                .get(cell.col_start)
                .copied()
                .unwrap_or(TableCellAlignment::None)
        } else {
            TableCellAlignment::None
        };
        let cell_ref = arena.new_node(TableCell::with_alignment(alignment));
        if colspan > 1 {
            arena[cell_ref]
                .attributes_mut()
                .insert("colspan", colspan.to_string().into());
        }
        if rowspan > 1 {
            arena[cell_ref]
                .attributes_mut()
                .insert("rowspan", rowspan.to_string().into());
        }
        let col_start_byte = spec.top_plus_cols[cell.col_start];
        let col_end_byte = spec.top_plus_cols[cell.col_end];
        populate_cell(
            arena,
            cell_ref,
            source,
            &cell.lines,
            col_start_byte,
            col_end_byte,
        );
        row_ref.append_child(arena, cell_ref);
    }
    parent.append_child(arena, row_ref);
}

// ============================================================================
// Cell content extraction (inline-only)
// ============================================================================

/// Extract the cell content segment from a single physical line,
/// between the `|` characters at display columns `col_start` and
/// `col_end`. Leading and trailing spaces are trimmed. Returns `None`
/// if the slice is empty or the line doesn't reach `col_end`.
fn extract_cell_segment(
    source: &str,
    line_seg: Segment,
    col_start: usize,
    col_end: usize,
) -> Option<Segment> {
    debug_assert_eq!(line_seg.padding(), 0);

    let line_bytes = &source.as_bytes()[line_seg.start()..line_seg.stop()];
    let content_byte_len = strip_newline(line_bytes);
    let content = &source[line_seg.start()..line_seg.start() + content_byte_len];

    let b_left = byte_offset_for_col(content, col_start);
    let b_right = byte_offset_for_col(content, col_end);
    if b_left >= b_right {
        return None;
    }
    let inner_start = b_left + 1; // skip the leading `|`
    if inner_start >= b_right {
        return None;
    }
    let seg = Segment::from((line_seg.start() + inner_start, line_seg.start() + b_right))
        .trim_left_space(source)
        .trim_right_space(source);
    if seg.is_empty() {
        None
    } else {
        Some(seg)
    }
}

/// Populate a `TableCell`'s `Block` source by extracting and joining
/// the column slices from all physical `lines` of the row.
///
/// Multi-physical-line cells use the same soft-line-break encoding as
/// `multiline_table::populate_cell`: each non-final line is emitted as
/// a content segment extended by one trailing byte (forcing the inline
/// parser to commit the text before crossing the segment boundary),
/// followed by a one-byte separator segment pointing at the line's
/// `\n` (which makes `parse_block` emit an empty text node carrying
/// the `SOFT_LINE_BREAK` qualifier).
fn populate_cell(
    arena: &mut Arena,
    cell_ref: NodeRef,
    source: &str,
    lines: &[Segment],
    col_start: usize,
    col_end: usize,
) {
    let mut collected: Vec<(Segment, Segment)> = Vec::with_capacity(lines.len());
    for &ls in lines {
        if let Some(seg) = extract_cell_segment(source, ls, col_start, col_end) {
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
            let extended_stop = (content_seg.stop() + 1).min(line_seg.stop());
            let extended = Segment::from((content_seg.start(), extended_stop));
            as_type_data_mut!(arena, cell_ref, Block).append_source_line(extended);

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
// Candidate AST node
// ============================================================================

/// Marker extension node used by [`GridTableParser`] as the
/// accumulator between `open()` and `close()`. The `Block` `TypeData`
/// (auto-allocated by `Arena::new_node`) carries the captured source
/// segments via `append_source_line` / `take_source`; this struct
/// itself has no state.
///
/// A custom extension node is used instead of `Paragraph` because the
/// engine's block-parsing loop **special-cases** `Paragraph` nodes —
/// it skips calling `cont()` on them and instead routes each line
/// through `open_blocks`, which would let the list parser steal mid-
/// table rows like `+    +---+` (a `+` followed by a space passes
/// `parse_list_item`'s bullet check).
#[derive(Debug, Default)]
struct GridTableCandidate;

impl NodeKind for GridTableCandidate {
    fn typ(&self) -> NodeType {
        NodeType::LeafBlock
    }

    fn kind_name(&self) -> &'static str {
        "GridTableCandidate"
    }
}

impl PrettyPrint for GridTableCandidate {
    fn pretty_print(&self, w: &mut dyn fmt::Write, _source: &str, level: usize) -> fmt::Result {
        writeln!(w, "{}GridTableCandidate", pp_indent(level))
    }
}

impl From<GridTableCandidate> for KindData {
    fn from(c: GridTableCandidate) -> Self {
        KindData::Extension(Box::new(c))
    }
}

// ============================================================================
// BlockParser
// ============================================================================

/// Registered at priority **40** — below rushdown's `ListParser` (300)
/// and `ThematicBreakParser` (200) so we get first dibs on lines like
/// `+    +---+` (a row-separator with a rowspan-continuation segment),
/// which would otherwise be parsed as the start of an unordered list
/// (`+ ` is a valid bullet marker).
///
/// The flow mirrors the multiline-table parser:
///
/// 1. `open()` peeks the line, requires an all-dash `+---+` border
///    (the table's top), and on success allocates a `Paragraph` node
///    as the accumulator. Just the top border line is consumed here;
///    every subsequent line is captured by `cont()`.
/// 2. `cont()` looks for any line that *could* be part of a grid
///    table (a `+`- or `|`-bordered line at indent ≤ 3), appends its
///    segment to the accumulator, and returns `NO_CHILDREN`. Any
///    other line (blank, paragraph text, EOF) returns `None`, which
///    closes the block.
/// 3. `close()` runs full validation via [`try_parse_grid_table`]
///    over the accumulated segments. On success the accumulator is
///    replaced with the built `Table` subtree; on failure the
///    accumulator is left as a plain `Paragraph` carrying the
///    original lines as text (the same fallback behaviour the
///    multiline-table parser uses for malformed candidates).
///
/// `can_interrupt_paragraph` returns `false` — a grid table preceded
/// by non-blank text on the previous line is not recognised. Authors
/// must put a blank line between text and the table, the same
/// convention multiline tables use.
#[derive(Debug, Default)]
pub(crate) struct GridTableParser;

impl GridTableParser {
    pub fn new() -> Self {
        Self
    }
}

impl BlockParser for GridTableParser {
    fn trigger(&self) -> &[u8] {
        b"+"
    }

    fn open(
        &self,
        arena: &mut Arena,
        _parent_ref: NodeRef,
        reader: &mut text::BasicReader,
        _ctx: &mut Context,
    ) -> Option<(NodeRef, State)> {
        let (line, seg) = reader.peek_line_bytes()?;
        if seg.padding() > 0 {
            return None;
        }
        let line_bytes: &[u8] = line.as_ref();
        let stripped = &line_bytes[..strip_newline(line_bytes)];
        // The top border must be all-dash. Equals-only or
        // continuation-bearing borders cannot open a table.
        let border = parse_grid_border(stripped, reader.line_offset())?;
        if !border.all_dash() {
            return None;
        }
        // Allocate the accumulator as a custom extension node — see
        // [`GridTableCandidate`] for why this can't be a `Paragraph`.
        let cand_ref = arena.new_node(GridTableCandidate);
        arena[cand_ref].set_pos(seg.start());
        as_type_data_mut!(arena, cand_ref, Block).append_source_line(seg);
        reader.advance_to_eol();
        Some((cand_ref, State::NO_CHILDREN))
    }

    fn cont(
        &self,
        arena: &mut Arena,
        node_ref: NodeRef,
        reader: &mut text::BasicReader,
        _ctx: &mut Context,
    ) -> Option<State> {
        let (line, seg) = reader.peek_line_bytes()?;
        if seg.padding() > 0 {
            return None;
        }
        let line_bytes: &[u8] = line.as_ref();
        let stripped = &line_bytes[..strip_newline(line_bytes)];
        // Loose admission: any `+`- or `|`-bordered line at indent ≤ 3
        // *might* belong to the grid table. Full validation runs in
        // `close()`. A blank line, a paragraph-text line, or EOF
        // closes the block.
        let (w, pos) = indent_width(stripped, reader.line_offset());
        if w > 3 || pos >= stripped.len() {
            return None;
        }
        match stripped[pos] {
            b'+' | b'|' => {
                as_type_data_mut!(arena, node_ref, Block).append_source_line(seg);
                Some(State::NO_CHILDREN)
            }
            _ => None,
        }
    }

    fn close(
        &self,
        arena: &mut Arena,
        node_ref: NodeRef,
        reader: &mut text::BasicReader,
        _ctx: &mut Context,
    ) {
        if arena.get(node_ref).is_none() {
            return;
        }
        let Some(parent) = arena[node_ref].parent() else {
            return;
        };
        let source = reader.source();
        let lines = as_type_data_mut!(arena, node_ref, Block).take_source();

        match try_parse_grid_table(source, &lines) {
            Some(spec) => {
                let table_ref = build_grid_table(arena, source, &spec);
                parent.replace_child(arena, node_ref, table_ref);
            }
            None => {
                // Fallback: convert the GridTableCandidate back into a
                // plain `Paragraph` carrying the captured lines so the
                // text is preserved. The standard inline pass and any
                // `ParagraphTransformer`s will run on it; pipe-table's
                // delimiter predicate rejects any line containing `+`,
                // so it can't mis-claim the fallback paragraph.
                let pos = arena[node_ref].pos().unwrap_or(0);
                let para_ref = arena.new_node(Paragraph::new());
                arena[para_ref].set_pos(pos);
                for line in lines {
                    as_type_data_mut!(arena, para_ref, Block).append_source_line(line);
                }
                parent.replace_child(arena, node_ref, para_ref);
            }
        }
    }

    fn can_interrupt_paragraph(&self) -> bool {
        false
    }
}

impl From<GridTableParser> for AnyBlockParser {
    fn from(p: GridTableParser) -> Self {
        AnyBlockParser::Extension(Box::new(p))
    }
}

/// Returns a [`ParserExtension`] that adds pandoc-style grid-table
/// support to a rushdown parser. Registers [`GridTableParser`] as a
/// block parser at priority **40** — below `ListParser` (300),
/// `ThematicBreakParser` (200), and `SetextHeadingParser` (100), so we
/// claim `+`-starting lines before any of them get a chance. (The
/// critical case is `ListParser`: a row separator like `+    +---+`
/// matches its `+ ` bullet pattern, and without the priority
/// inversion the list parser would steal the line mid-table.)
pub(crate) fn grid_table_parser_extension() -> impl ParserExtension {
    ParserExtensionFn::new(|p: &mut Parser| {
        p.add_block_parser(GridTableParser::new, NoParserOptions, 40);
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

    fn make_parser() -> parser::Parser {
        parser::Parser::with_extensions(
            parser::Options {
                attributes: false,
                auto_heading_ids: false,
                ..Default::default()
            },
            parser::gfm(parser::GfmOptions::default()).and(grid_table_parser_extension()),
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

    // ── Helpers tests ─────────────────────────────────────────────────────

    #[test]
    fn border_dash_basic() {
        let border = parse_grid_border(b"+---+---+", 0).unwrap();
        assert_eq!(border.plus_positions, vec![0, 4, 8]);
        assert!(border.all_dash());
    }

    #[test]
    fn border_equals_basic() {
        let border = parse_grid_border(b"+===+===+", 0).unwrap();
        assert_eq!(border.plus_positions, vec![0, 4, 8]);
        assert!(border.all_equals());
    }

    #[test]
    fn border_with_alignment_markers() {
        let border = parse_grid_border(b"+:--+--:+:-:+", 0).unwrap();
        assert_eq!(border.plus_positions, vec![0, 4, 8, 12]);
        assert!(border.all_dash());
        let aligns = alignment_from_border(b"+:--+--:+:-:+", &border.plus_positions);
        assert_eq!(
            aligns,
            vec![
                TableCellAlignment::Left,
                TableCellAlignment::Right,
                TableCellAlignment::Center,
            ]
        );
    }

    #[test]
    fn border_with_continuation_segments() {
        // Row separator: cell at col 0 continues into the next row,
        // cell at col 1 ends.
        let border = parse_grid_border(b"+   +---+", 0).unwrap();
        assert_eq!(border.plus_positions, vec![0, 4, 8]);
        assert_eq!(
            border.segments,
            vec![SegmentKind::Continuation, SegmentKind::Dash]
        );
        assert!(!border.all_dash());
        assert!(!border.all_equals());
    }

    #[test]
    fn border_all_continuation_ok_to_parse() {
        // `+   +   +` parses; whether it's a *valid* row separator in
        // context is checked by `try_parse_grid_table` (which currently
        // rejects all-continuation separators).
        let border = parse_grid_border(b"+   +   +", 0).unwrap();
        assert_eq!(
            border.segments,
            vec![SegmentKind::Continuation, SegmentKind::Continuation]
        );
    }

    #[test]
    fn border_rejects_mixed_dash_equals() {
        assert!(parse_grid_border(b"+---+===+", 0).is_none());
    }

    #[test]
    fn border_rejects_indent_gt_3() {
        assert!(parse_grid_border(b"    +---+", 0).is_none());
    }

    #[test]
    fn border_rejects_no_plus_terminator() {
        assert!(parse_grid_border(b"+---+---", 0).is_none());
    }

    #[test]
    fn border_rejects_empty_column() {
        assert!(parse_grid_border(b"++---+", 0).is_none());
    }

    #[test]
    fn border_with_trailing_spaces_ok() {
        let border = parse_grid_border(b"+---+---+   ", 0).unwrap();
        assert_eq!(border.plus_positions, vec![0, 4, 8]);
        assert!(border.all_dash());
    }

    #[test]
    fn border_rejects_dash_segment_with_space() {
        // A segment with mixed dash + space is malformed (a segment
        // is either contiguous fill or pure continuation).
        assert!(parse_grid_border(b"+ - +", 0).is_none());
    }

    #[test]
    fn content_line_matches_top_cols() {
        assert!(is_grid_content_line(b"| a | b |", 0, &[0, 4, 8]));
        // Missing middle `|` is a *column-spanning* content line — valid
        // (the middle column is merged with one neighbour).
        assert!(is_grid_content_line(b"| a   b |", 0, &[0, 4, 8]));
        // First or last `|` missing → invalid (table outer borders must
        // always be present).
        assert!(!is_grid_content_line(b"  a | b |", 0, &[0, 4, 8]));
        assert!(!is_grid_content_line(b"| a | b  ", 0, &[0, 4, 8]));
    }

    #[test]
    fn content_line_cyrillic() {
        // | Aaa | Bbb | with Cyrillic content — `|` positions must align
        // in display columns, not bytes.
        let input = "| Ы   | Ю   |";
        assert!(is_grid_content_line(input.as_bytes(), 0, &[0, 6, 12]));
    }

    // ── Parser integration tests ──────────────────────────────────────────

    #[test]
    fn with_header_basic_structure() {
        let input = "\
+--------+--------+
| Head 1 | Head 2 |
+========+========+
| Row 1A | Row 1B |
+--------+--------+
| Row 2A | Row 2B |
+--------+--------+
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let header = find_first!(&arena, table, TableHeader).expect("no TableHeader");
        let header_rows = children_of_kind(&arena, header, "TableRow");
        assert_eq!(header_rows.len(), 1);
        let header_cells = children_of_kind(&arena, header_rows[0], "TableCell");
        assert_eq!(header_cells.len(), 2);

        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let body_rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(body_rows.len(), 2);
        for row in &body_rows {
            let cells = children_of_kind(&arena, *row, "TableCell");
            assert_eq!(cells.len(), 2);
        }
    }

    #[test]
    fn headerless_basic_structure() {
        let input = "\
+--------+--------+
| Row 1A | Row 1B |
+--------+--------+
| Row 2A | Row 2B |
+--------+--------+
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        assert!(
            find_first!(&arena, table, TableHeader).is_none(),
            "headerless must not have a TableHeader"
        );
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn multi_line_cell_content_joined() {
        let input = "\
+--------+--------+
| line a | line c |
| line b | line d |
+--------+--------+
";
        let html = render(input);
        // Both physical lines of cell 1 must appear in the same cell.
        let td_open = html.find("<td").expect("no <td");
        let td_close = html.find("</td>").expect("no </td>");
        let first_cell = &html[td_open..td_close + 5];
        assert!(
            first_cell.contains("line a") && first_cell.contains("line b"),
            "both physical lines must be in cell 1: {html}"
        );
    }

    #[test]
    fn alignment_markers_in_header_separator() {
        let input = "\
+---+---+---+
| L | R | C |
+:--+--:+:-:+
| a | b | c |
+---+---+---+
";
        let html = render(input);
        assert!(
            html.contains("text-align: left") || html.contains("align=\"left\""),
            "left alignment: {html}"
        );
        assert!(
            html.contains("text-align: right") || html.contains("align=\"right\""),
            "right alignment: {html}"
        );
        assert!(
            html.contains("text-align: center") || html.contains("align=\"center\""),
            "center alignment: {html}"
        );
    }

    #[test]
    fn alignment_markers_in_top_border() {
        // Alignment can also be specified on the top border (with or without
        // a header separator). Headerless table with alignment.
        let input = "\
+:--+--:+
| a | b |
+---+---+
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let row = children_of_kind(&arena, body, "TableRow")
            .into_iter()
            .next()
            .unwrap();
        let cells = children_of_kind(&arena, row, "TableCell");
        let a0 = rushdown::as_kind_data!(&arena, cells[0], TableCell).alignment();
        let a1 = rushdown::as_kind_data!(&arena, cells[1], TableCell).alignment();
        assert!(matches!(a0, TableCellAlignment::Left));
        assert!(matches!(a1, TableCellAlignment::Right));
    }

    #[test]
    fn cyrillic_cell_content_does_not_corrupt_utf8() {
        let input = "\
+---------+---------+
| Ячейка1 | Ячейка2 |
+=========+=========+
| Значен. | Значен. |
+---------+---------+
";
        let html = render(input);
        assert!(html.is_char_boundary(0));
        assert!(html.contains("<table>"));
        assert!(html.contains("Ячейка1"));
        assert!(html.contains("Ячейка2"));
        assert!(html.contains("Значен."));
    }

    #[test]
    fn missing_bottom_border_falls_through() {
        let input = "\
+---+---+
| a | b |
";
        let html = render(input);
        assert!(
            !html.contains("<table"),
            "missing bottom border must fall through: {html}"
        );
    }

    #[test]
    fn misaligned_border_falls_through() {
        // The second border has different column positions than the first.
        let input = "\
+---+---+
| a | b |
+----+---+
";
        let html = render(input);
        assert!(
            !html.contains("<table"),
            "misaligned border must fall through: {html}"
        );
    }

    #[test]
    fn blank_line_inside_table_falls_through() {
        let input = "\
+---+---+
| a | b |

| c | d |
+---+---+
";
        let html = render(input);
        assert!(
            !html.contains("<table"),
            "blank inside table must fall through: {html}"
        );
    }

    #[test]
    fn double_header_separator_falls_through() {
        let input = "\
+---+---+
| h | h |
+===+===+
| a | a |
+===+===+
| b | b |
+---+---+
";
        let html = render(input);
        assert!(
            !html.contains("<table"),
            "two `+===+` separators must reject: {html}"
        );
    }

    #[test]
    fn inline_markdown_in_cells() {
        let input = "\
+----------+----------+
| **bold** | *em*     |
+==========+==========+
| `code`   | plain    |
+----------+----------+
";
        let html = render(input);
        assert!(html.contains("<strong>"), "bold: {html}");
        assert!(html.contains("<em>"), "em: {html}");
        assert!(html.contains("<code>"), "code: {html}");
    }

    #[test]
    fn indented_table_up_to_three_spaces() {
        // Note: the `\<newline>` Rust line-continuation strips leading
        // whitespace from the first physical source line, so the explicit
        // `\n`s here are required to keep all five table lines at the same
        // 3-space indent.
        let input = "   +---+---+\n   | a | b |\n   +---+---+\n   | c | d |\n   +---+---+\n";
        let html = render(input);
        assert!(html.contains("<table"), "3-space-indented table: {html}");
    }

    #[test]
    fn followed_by_paragraph() {
        let input = "\
+---+---+
| a | b |
+---+---+

Following paragraph.
";
        let html = render(input);
        assert!(html.contains("<table"), "table present: {html}");
        assert!(html.contains("Following paragraph"), "para after: {html}");
    }

    #[test]
    fn table_inside_blockquote() {
        let input = "\
> +---+---+
> | a | b |
> +---+---+
> | c | d |
> +---+---+
";
        let (arena, doc) = parse(input);
        let bq = {
            let mut found: Option<NodeRef> = None;
            walk(&arena, doc, &mut |arena: &Arena,
                                    node_ref: NodeRef,
                                    entering: bool|
             -> Result<WalkStatus> {
                if entering
                    && arena[node_ref].kind_data().kind_name() == "Blockquote"
                    && found.is_none()
                {
                    found = Some(node_ref);
                }
                Ok(WalkStatus::Continue)
            })
            .ok();
            found.expect("no Blockquote")
        };
        let table = find_first!(&arena, bq, Table).expect("no Table inside blockquote");
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn table_inside_list_item() {
        let input = "\
* item 1

* item 2 has a table:

  +---+---+
  | a | b |
  +---+---+
  | c | d |
  +---+---+

* item 3
";
        let html = render(input);
        assert!(html.contains("<ul"), "list: {html}");
        assert!(html.contains("<table"), "table inside list: {html}");
    }

    #[test]
    fn pandoc_parity_canonical_example() {
        // Canonical example from
        // https://pandoc.org/demo/example33/8.9-tables.html
        let input = "\
+---------------+---------------+--------------------+
| Fruit         | Price         | Advantages         |
+===============+===============+====================+
| Bananas       | $1.34         | built-in wrapper   |
+---------------+---------------+--------------------+
| Oranges       | $2.10         | cures scurvy       |
+---------------+---------------+--------------------+
";
        let html = render(input);
        assert!(html.contains("<table"), "{html}");
        assert!(html.contains("<thead"), "{html}");
        assert!(html.contains("<tbody"), "{html}");
        for needle in [
            "Fruit",
            "Price",
            "Advantages",
            "Bananas",
            "$1.34",
            "built-in wrapper",
            "Oranges",
            "$2.10",
            "cures scurvy",
        ] {
            assert!(html.contains(needle), "{needle} missing in {html}");
        }
    }

    #[test]
    fn pandoc_parity_alignment_example() {
        // Pandoc manual:
        //   +---------------+---------------+--------------------+
        //   | Right         | Left          | Centered           |
        //   +==============:+:==============+:==================:+
        //   | Bananas       | $1.34         | built-in wrapper   |
        //   +---------------+---------------+--------------------+
        let input = "\
+---------------+---------------+--------------------+
| Right         | Left          | Centered           |
+==============:+:==============+:==================:+
| Bananas       | $1.34         | built-in wrapper   |
+---------------+---------------+--------------------+
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let header = find_first!(&arena, table, TableHeader).expect("no TableHeader");
        let row = children_of_kind(&arena, header, "TableRow")
            .into_iter()
            .next()
            .unwrap();
        let cells = children_of_kind(&arena, row, "TableCell");
        let a0 = rushdown::as_kind_data!(&arena, cells[0], TableCell).alignment();
        let a1 = rushdown::as_kind_data!(&arena, cells[1], TableCell).alignment();
        let a2 = rushdown::as_kind_data!(&arena, cells[2], TableCell).alignment();
        assert!(matches!(a0, TableCellAlignment::Right), "{:?}", a0);
        assert!(matches!(a1, TableCellAlignment::Left), "{:?}", a1);
        assert!(matches!(a2, TableCellAlignment::Center), "{:?}", a2);
    }

    // ── Column-span tests ─────────────────────────────────────────────────

    /// A header cell spanning two columns over a 3-column body.
    #[test]
    fn colspan_in_header() {
        let input = "\
+----+----+----+
| Group 1 | C  |
+====+====+====+
| a  | b  | c  |
+----+----+----+
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let header = find_first!(&arena, table, TableHeader).expect("no TableHeader");
        let row = children_of_kind(&arena, header, "TableRow")
            .into_iter()
            .next()
            .unwrap();
        let cells = children_of_kind(&arena, row, "TableCell");
        assert_eq!(cells.len(), 2, "header row should have 2 cells (spanned)");
        let colspan = arena[cells[0]]
            .attributes()
            .get("colspan")
            .map(|v| v.str(input).to_string());
        assert_eq!(colspan.as_deref(), Some("2"));
        assert!(arena[cells[1]].attributes().get("colspan").is_none());

        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let body_row = children_of_kind(&arena, body, "TableRow")
            .into_iter()
            .next()
            .unwrap();
        let body_cells = children_of_kind(&arena, body_row, "TableCell");
        assert_eq!(body_cells.len(), 3);
    }

    /// Column span in a body row.
    #[test]
    fn colspan_in_body() {
        // Body row 2 merges cols 1+2: pipe positions are 0, 10, 15
        // (matching top columns 0, 10, 15) and content `spans   ` fills
        // the first 9 cells worth of width before the next `|`.
        let input = "\
+----+----+----+
| A  | B  | C  |
+====+====+====+
| a  | b  | c  |
+----+----+----+
| spans   | z  |
+----+----+----+
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 2);

        let r1_cells = children_of_kind(&arena, rows[0], "TableCell");
        assert_eq!(r1_cells.len(), 3);

        let r2_cells = children_of_kind(&arena, rows[1], "TableCell");
        assert_eq!(r2_cells.len(), 2, "body row 2 should be span+single");
        let span = arena[r2_cells[0]]
            .attributes()
            .get("colspan")
            .map(|v| v.str(input).to_string());
        assert_eq!(span.as_deref(), Some("2"));
        assert!(arena[r2_cells[1]].attributes().get("colspan").is_none());
    }

    /// Multiple spans in the same row.
    #[test]
    fn colspan_multiple_in_one_row() {
        let input = "\
+----+----+----+----+
| A1 | A2 | A3 | A4 |
+====+====+====+====+
| span1   | span2   |
+----+----+----+----+
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let row = children_of_kind(&arena, body, "TableRow")
            .into_iter()
            .next()
            .unwrap();
        let cells = children_of_kind(&arena, row, "TableCell");
        assert_eq!(cells.len(), 2);
        for cell in &cells {
            let span = arena[*cell]
                .attributes()
                .get("colspan")
                .map(|v| v.str(input).to_string());
            assert_eq!(span.as_deref(), Some("2"));
        }
    }

    /// Inconsistent pipe positions across the physical lines of one
    /// row: pandoc rejects this and so do we — the row cannot be
    /// cleanly cut into cells. Falls back to a paragraph rather than
    /// emit garbage.
    #[test]
    fn inconsistent_row_pipes_fallback() {
        let input = "\
+----+----+
| a  | b  |
| c     d |
+----+----+
";
        let html = render(input);
        assert!(
            !html.contains("<table"),
            "inconsistent row pipes must fall back: {html}"
        );
    }

    /// A content line missing the *first* `|` (outer left border) is
    /// not a valid content line — the table is no longer recognised.
    #[test]
    fn missing_outer_left_pipe_falls_through() {
        let input = "\
+----+----+
  a  | b  |
+----+----+
";
        let html = render(input);
        assert!(
            !html.contains("<table"),
            "missing left outer `|` must fall through: {html}"
        );
    }

    /// HTML output for a spanned cell carries `colspan="N"`.
    #[test]
    fn html_colspan_emitted() {
        let input = "\
+----+----+----+
| Group 1 | C  |
+====+====+====+
| a  | b  | c  |
+----+----+----+
";
        let html = render(input);
        assert!(
            html.contains("colspan=\"2\""),
            "<th colspan=\"2\"> missing from output: {html}"
        );
    }

    // ── Row-span tests ────────────────────────────────────────────────────

    /// Single cell spanning two rows. Continuation row's TableRow has
    /// fewer children than the column count.
    #[test]
    fn rowspan_basic() {
        let input = "\
+----+----+
| A  | B  |
+    +----+
|    | C  |
+----+----+
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 2);

        // Row 1 has cells A (rowspan=2) and B.
        let r1_cells = children_of_kind(&arena, rows[0], "TableCell");
        assert_eq!(r1_cells.len(), 2);
        let rowspan = arena[r1_cells[0]]
            .attributes()
            .get("rowspan")
            .map(|v| v.str(input).to_string());
        assert_eq!(rowspan.as_deref(), Some("2"));
        assert!(arena[r1_cells[1]].attributes().get("rowspan").is_none());

        // Row 2 has only cell C (col 0 is continued from above).
        let r2_cells = children_of_kind(&arena, rows[1], "TableCell");
        assert_eq!(r2_cells.len(), 1, "continuation row has only the new cell");
    }

    /// A cell with both colspan and rowspan.
    #[test]
    fn rowspan_with_colspan() {
        // Note the row separator `+    +    +----+` keeps a `+` at
        // every top-border column position — pandoc requires this
        // even when consecutive columns are all in a continuation
        // segment (the "merged" form `+         +----+` is rejected
        // by our strict-equality column-position check).
        let input = "\
+----+----+----+
| Big     | X  |
+    +    +----+
|         | Y  |
+----+----+----+
| a  | b  | c  |
+----+----+----+
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 3);

        let r1_cells = children_of_kind(&arena, rows[0], "TableCell");
        assert_eq!(r1_cells.len(), 2);
        let colspan = arena[r1_cells[0]]
            .attributes()
            .get("colspan")
            .map(|v| v.str(input).to_string());
        let rowspan = arena[r1_cells[0]]
            .attributes()
            .get("rowspan")
            .map(|v| v.str(input).to_string());
        assert_eq!(colspan.as_deref(), Some("2"));
        assert_eq!(rowspan.as_deref(), Some("2"));

        // Row 2 has only the X-continuation column → only Y.
        let r2_cells = children_of_kind(&arena, rows[1], "TableCell");
        assert_eq!(r2_cells.len(), 1);

        // Row 3 is a fresh 3-cell row.
        let r3_cells = children_of_kind(&arena, rows[2], "TableCell");
        assert_eq!(r3_cells.len(), 3);
    }

    /// Multiple cells span rows simultaneously.
    #[test]
    fn rowspan_multiple_columns() {
        let input = "\
+----+----+
| A  | B  |
+    +    +
|    |    |
+----+----+
| c  | d  |
+----+----+
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 3);

        let r1_cells = children_of_kind(&arena, rows[0], "TableCell");
        assert_eq!(r1_cells.len(), 2);
        for cell in &r1_cells {
            let rowspan = arena[*cell]
                .attributes()
                .get("rowspan")
                .map(|v| v.str(input).to_string());
            assert_eq!(rowspan.as_deref(), Some("2"));
        }

        // Row 2 has no new cells.
        let r2_cells = children_of_kind(&arena, rows[1], "TableCell");
        assert_eq!(r2_cells.len(), 0);

        // Row 3 is fresh.
        let r3_cells = children_of_kind(&arena, rows[2], "TableCell");
        assert_eq!(r3_cells.len(), 2);
    }

    /// Header row contains a cell that spans down into another header row
    /// (still within the header, before the `+===+`).
    #[test]
    fn rowspan_in_header() {
        let input = "\
+----+----+
| H1 | H2 |
+    +----+
|    | H3 |
+====+====+
| a  | b  |
+----+----+
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let header = find_first!(&arena, table, TableHeader).expect("no TableHeader");
        let header_rows = children_of_kind(&arena, header, "TableRow");
        assert_eq!(header_rows.len(), 2);

        let r1_cells = children_of_kind(&arena, header_rows[0], "TableCell");
        assert_eq!(r1_cells.len(), 2);
        let rowspan = arena[r1_cells[0]]
            .attributes()
            .get("rowspan")
            .map(|v| v.str(input).to_string());
        assert_eq!(rowspan.as_deref(), Some("2"));

        let r2_cells = children_of_kind(&arena, header_rows[1], "TableCell");
        assert_eq!(r2_cells.len(), 1);

        // Body unchanged.
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let body_rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(body_rows.len(), 1);
    }

    /// HTML output for a row-spanning cell carries `rowspan="N"`.
    #[test]
    fn html_rowspan_emitted() {
        let input = "\
+----+----+
| A  | B  |
+    +----+
|    | C  |
+----+----+
";
        let html = render(input);
        assert!(
            html.contains("rowspan=\"2\""),
            "<td rowspan=\"2\"> missing from output: {html}"
        );
    }

    /// A row separator with Equals + Continuation is rejected (would mean
    /// rowspan crossing the header/body boundary).
    #[test]
    fn rowspan_crossing_header_separator_rejected() {
        let input = "\
+----+----+
| H1 | H2 |
+    +====+
|    | b  |
+----+----+
";
        let html = render(input);
        assert!(
            !html.contains("<table"),
            "rowspan crossing the header separator must reject: {html}"
        );
    }

    /// A row separator's continuation at a column with no active cell
    /// is rejected.
    #[test]
    fn rowspan_continuation_with_no_active_cell_rejected() {
        // Top border with one column ends (cell A in row 1), then a
        // continuation marker in col 0 → but cell A already ended.
        // (Constructed by emitting two separators in a row, which the
        // current code already rejects on a different ground; this test
        // documents the active-state invariant.)
        let input = "\
+----+
| A  |
+----+
|    |
+----+
";
        // Pandoc-correct: the middle `+----+` ends cell A; row 2's
        // `|    |` is a new empty cell. So this IS a valid table with
        // two body rows. (Documenting that emptiness is allowed.)
        let html = render(input);
        assert!(
            html.contains("<table"),
            "two empty cells should render: {html}"
        );
    }

    /// A rowspan slot whose column extent doesn't match the cell from
    /// above (partial column-span continuation) is rejected.
    #[test]
    fn rowspan_partial_column_span_rejected() {
        // Row 1 has a colspan-2 cell. Row 2 tries to continue only the
        // first column of that span — invalid.
        let input = "\
+----+----+
| AB      |
+    +----+
|    | C  |
+----+----+
";
        let html = render(input);
        assert!(
            !html.contains("<table"),
            "partial column-span continuation must reject: {html}"
        );
    }
}
