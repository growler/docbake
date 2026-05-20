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

use core::fmt;

use rushdown::ast::{
    pp_indent, Arena, KindData, NodeKind, NodeRef, NodeType, Paragraph, PrettyPrint, Table,
    TableBody, TableCell, TableCellAlignment, TableHeader, TableRow,
};
use rushdown::parser::{
    AnyBlockParser, BlockParser, Context, NoParserOptions, Parser, ParserExtension,
    ParserExtensionFn, State,
};
use rushdown::text::{self, Reader as _, Segment};
use rushdown::util::{
    indent_position, indent_width, is_blank, is_space, tab_width, trim_left_space_length,
    trim_right_space_length,
};
use rushdown::{as_extension_data_mut, as_type_data_mut};

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

/// Outcome of [`strip_container_indent`].
enum StripIndent {
    /// Indent successfully consumed (or none required, or line is blank/EOF —
    /// in all cases the caller may peek the line and process it normally).
    Ok,
    /// A non-blank line had less leading whitespace than `indent` columns —
    /// the table is no longer aligned, the caller must abort the parse.
    Insufficient,
}

/// Advances `reader` past `indent` display columns of leading whitespace on
/// the current line, mirroring what a *whitespace-stripping* parent
/// container's `cont()` would have done before invoking a fresh child block
/// parser.
///
/// Necessary because [`MultilineTableParser::open`] eagerly consumes every
/// line of the table inside a single call. After our own `advance_line()` the
/// reader sits at the next line's head with the parent container's continuation
/// indent (list-item content offset, def-list body indent, fenced-div
/// padding, …) still attached. Without re-stripping it on every line after
/// the first, `parse_column_rule` / `is_any_dash_rule` see the indent as
/// part of the line content and either reject the bottom rule
/// (`indent_width > 3`) or shift the derived column extents — both of which
/// silently corrupt the parse.
///
/// Blank lines and EOF are returned as [`StripIndent::Ok`] without
/// advancement; pandoc treats them as row separators / table terminators and
/// neither requires an indent prefix.
///
/// This helper handles only *whitespace* continuation. Blockquote markers
/// (`>`) require [`strip_blockquote_layers`]; see [`ParentMode`] for the
/// dispatch.
fn strip_container_indent(reader: &mut text::BasicReader, indent: usize) -> StripIndent {
    if indent == 0 {
        return StripIndent::Ok;
    }
    let Some((line, _)) = reader.peek_line_bytes() else {
        return StripIndent::Ok;
    };
    if is_blank(&line) {
        return StripIndent::Ok;
    }
    match indent_position(&line, reader.line_offset(), indent) {
        Some((pos, padding)) => {
            reader.advance_and_set_padding(pos, padding);
            StripIndent::Ok
        }
        None => StripIndent::Insufficient,
    }
}

/// Advances `reader` past `layers` consecutive blockquote markers on the
/// current line, replicating the byte/column shape of
/// [`BlockquoteParser::process`] (`rushdown-0.18.0/src/parser/blockquote.rs`):
/// for each layer, skip any leading whitespace via [`indent_width`], require
/// a `>`, advance past it, then optionally consume one trailing space/tab
/// (with the tab-padding adjustment).
///
/// Necessary because [`MultilineTableParser::open`] eagerly consumes every
/// line in a single dispatch — the rushdown block dispatcher does **not**
/// re-invoke `BlockquoteParser::cont()` between our internal
/// `advance_line()` calls, so each subsequent peeked line still carries the
/// raw `>` prefix that the parent blockquote would have stripped on a
/// normal top-level line.
///
/// Returns [`StripIndent::Insufficient`] the first time a layer's `>` is
/// missing (including the case of a truly blank `\n`-only line, which
/// terminates the parent blockquote in CommonMark/pandoc). A `>\n` line
/// (blockquote with empty content) succeeds: the `>` is consumed and the
/// reader sits on the trailing `\n`, where the outer table loop's
/// `is_blank` check then catches it as a row separator.
fn strip_blockquote_layers(reader: &mut text::BasicReader, layers: usize) -> StripIndent {
    for _ in 0..layers {
        let Some((line, _)) = reader.peek_line_bytes() else {
            return StripIndent::Ok;
        };
        let line_bytes: &[u8] = line.as_ref();
        let (_, mut pos) = indent_width(line_bytes, reader.line_offset());
        if pos >= line_bytes.len() || line_bytes[pos] != b'>' {
            return StripIndent::Insufficient;
        }
        pos += 1;
        if pos >= line_bytes.len() || line_bytes[pos] == b'\n' {
            // Line ends at the marker — `>\n` (blank-content). Consume the
            // marker; the next blockquote layer (if any) will see the
            // remaining bytes and fail, which correctly aborts the table
            // parse for nested cases where the inner blockquote is not
            // continued.
            reader.advance(pos);
            continue;
        }
        reader.advance(pos);
        let next = line_bytes[pos];
        if next == b' ' || next == b'\t' {
            let padding = if next == b'\t' {
                tab_width(reader.line_offset()) - 1
            } else {
                0
            };
            reader.advance_and_set_padding(1, padding);
        }
    }
    StripIndent::Ok
}

/// Per-line continuation strip tokenised from the raw bytes of the top
/// rule line's prefix (the bytes between the line head and
/// `reader.pos.start` when `open()` is called, i.e. what every ancestor
/// collectively stripped on the top rule line).
///
/// Each `StripOp` corresponds to either a whitespace run (any composition
/// of list-item, def-list term-definition, or other indent-stripping
/// ancestors) or a single blockquote layer. The whole sequence is
/// applied — in order — to each peeked line during the forward-scan in
/// `open()`.
///
/// During `cont()` we don't need the sequence at all: the engine invokes
/// each parent's `cont()` in turn before reaching us, so the line we
/// peek there is already fully stripped. The sequence only exists to
/// validate the forward-scan's view of upcoming lines from raw source.
#[derive(Debug, Clone)]
struct ParentStripSeq(Vec<StripOp>);

/// A single strip operation.
#[derive(Debug, Clone, Copy)]
enum StripOp {
    /// Strip up to `cols` columns of leading whitespace. Blank lines (at
    /// the time this op is applied) pass through unchanged — matches the
    /// engine's tolerance for blank lines inside list-item / def-list-body
    /// continuations.
    Whitespace { cols: usize },
    /// Strip ≤3 cols leading whitespace, then `>`, then optionally one
    /// space/tab. Mirrors `BlockquoteParser::process`.
    Blockquote,
}

impl ParentStripSeq {
    /// Tokenises `prefix` (the raw bytes the dispatcher stripped on the
    /// top rule line) into a sequence of strip ops, without consulting
    /// the AST. The prefix is decomposed left-to-right as alternating
    /// runs of leading whitespace and `>` blockquote markers — every
    /// container kind whose `cont()` strips a whitespace indent (list
    /// items, def-list term-definitions) or a blockquote marker is
    /// covered by this grammar. Container kinds that strip nothing
    /// (fenced divs, `Document`, `List`, `DefinitionList`) contribute
    /// no prefix bytes and are therefore irrelevant.
    ///
    /// Returns `None` if the prefix contains any byte that is neither
    /// whitespace nor `>` — that would mean the dispatcher consumed
    /// something this parser cannot replay, so we decline the candidate
    /// rather than guess.
    fn build(prefix: &[u8]) -> Option<Self> {
        let mut ops: Vec<StripOp> = Vec::new();
        let mut cursor: usize = 0;
        let mut col: usize = 0;
        while cursor < prefix.len() {
            // Whitespace run.
            let mut ws_cols: usize = 0;
            while let Some(&b) = prefix.get(cursor) {
                if b == b' ' {
                    ws_cols += 1;
                    cursor += 1;
                } else if b == b'\t' {
                    ws_cols += tab_width(col + ws_cols);
                    cursor += 1;
                } else {
                    break;
                }
            }
            if ws_cols > 0 {
                ops.push(StripOp::Whitespace { cols: ws_cols });
                col += ws_cols;
            }
            // Blockquote marker (with optional trailing space/tab).
            match prefix.get(cursor) {
                Some(&b'>') => {
                    cursor += 1;
                    col += 1;
                    match prefix.get(cursor) {
                        Some(&b' ') => {
                            cursor += 1;
                            col += 1;
                        }
                        Some(&b'\t') => {
                            let pad = tab_width(col);
                            cursor += 1;
                            col += pad;
                        }
                        _ => {}
                    }
                    ops.push(StripOp::Blockquote);
                }
                Some(_) => return None, // unrecognised non-whitespace byte
                None => break,
            }
        }
        Some(Self(ops))
    }

    /// Apply the sequence to the current line. Returns
    /// [`StripIndent::Insufficient`] if any op fails on a non-blank line.
    fn apply(&self, reader: &mut text::BasicReader) -> StripIndent {
        for op in &self.0 {
            // Re-peek for each op — blank-line semantics are op-specific
            // and prior ops may have advanced the reader past leading
            // whitespace into a now-blank tail.
            let blank = reader
                .peek_line_bytes()
                .map(|(b, _)| is_blank(b.as_ref()))
                .unwrap_or(true);
            match op {
                StripOp::Whitespace { cols } => {
                    if blank {
                        continue; // list/def-list continue past blank lines
                    }
                    if matches!(
                        strip_container_indent(reader, *cols),
                        StripIndent::Insufficient
                    ) {
                        return StripIndent::Insufficient;
                    }
                }
                StripOp::Blockquote => {
                    if matches!(
                        strip_blockquote_layers(reader, 1),
                        StripIndent::Insufficient
                    ) {
                        return StripIndent::Insufficient;
                    }
                }
            }
        }
        StripIndent::Ok
    }
}

/// Finds the byte position of the start of the line containing byte
/// `pos` in `source`. Used to extract the raw prefix bytes that the
/// dispatcher stripped on the top rule line.
fn find_line_head(source: &[u8], pos: usize) -> usize {
    source[..pos]
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0)
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
                let content = &source[line_seg.start()..line_seg.start() + content_byte_len];
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
// Intermediate AST node — collected during the line-by-line phase, replaced
// in `close()` by a real `Table` (or a `Paragraph` fallback).
// ============================================================================

/// State of the per-line collection state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CandidateState {
    /// With-header form, pre-separator: subsequent lines are header
    /// content until a grouped column rule appears.
    #[default]
    AwaitingHeader,
    /// Body section: content lines accumulate, blank lines separate rows,
    /// a dash rule (solid or grouped) closes the table.
    AwaitingBody,
    /// A bottom rule was consumed in `cont()`; the next `cont()` call
    /// returns `None` to close.
    Complete,
}

/// Classification used when capturing a line, so the close-time transform
/// can partition the captured stream into header / body / row-separator
/// groups without re-parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Header,
    Body,
    /// Blank line — row separator inside the body section.
    Blank,
}

#[derive(Debug, Clone, Copy)]
struct CapturedLine {
    /// Post-parent-strip source segment for the line, including its
    /// trailing newline byte (consistent with what `peek_line_bytes()`
    /// returns).
    seg: Segment,
    kind: LineKind,
}

/// AST extension node holding the table's captured lines between the
/// block-parsing phase (`open` + `cont`) and the in-phase transform
/// (`close`). Marked as a [`NodeType::LeafBlock`] so the engine does not
/// attempt to open any child blocks for it. Its built-in `Block`
/// type-data carries an empty source vector — captured segments live in
/// this struct's own `lines` field, which keeps `parse_block` from
/// inline-parsing them and preserves the segments for the close-time
/// transform. The node is unconditionally replaced (by a `Table` on
/// success or a `Paragraph` on fallback) inside `close()`, so the
/// extension type never reaches the renderer.
#[derive(Debug)]
struct MultilineTableCandidate {
    /// True if the top rule was solid (with-header form).
    with_header: bool,
    /// Top-rule source segment (kept for the fallback paragraph so the
    /// original text is preserved when the candidate cannot be materialised
    /// as a table).
    top_rule_seg: Segment,
    /// Column extents in stripped-line column space. Populated in
    /// `open()` for headerless tables, in `cont()` (on separator) for
    /// with-header tables.
    col_extents: Vec<(usize, usize)>,
    /// Captured (post-parent-strip) source segments for every line the
    /// state machine accepted, in document order.
    lines: Vec<CapturedLine>,
    /// State machine cursor.
    state: CandidateState,
}

impl NodeKind for MultilineTableCandidate {
    fn typ(&self) -> NodeType {
        NodeType::LeafBlock
    }

    fn kind_name(&self) -> &'static str {
        "MultilineTableCandidate"
    }
}

impl PrettyPrint for MultilineTableCandidate {
    fn pretty_print(&self, w: &mut dyn fmt::Write, _source: &str, level: usize) -> fmt::Result {
        writeln!(
            w,
            "{}MultilineTableCandidate (state={:?}, lines={})",
            pp_indent(level),
            self.state,
            self.lines.len()
        )
    }
}

impl From<MultilineTableCandidate> for KindData {
    fn from(c: MultilineTableCandidate) -> Self {
        KindData::Extension(Box::new(c))
    }
}

// ============================================================================
// Block parser
// ============================================================================

/// [`BlockParser`] for pandoc-style multiline tables.
///
/// Cooperates with the rushdown line dispatcher rather than fighting it.
///
/// 1. [`open`][Self::open] runs a *read-only* forward-scan over the source
///    that follows the top rule, applying the parent chain's strip via
///    [`classify_parent_chain`] + [`strip_parent_prefix`] on a cursor
///    reader, and validates that the candidate looks like a well-formed
///    multiline table. Heuristics:
///    - A top rule immediately followed by a blank (post-strip) line is
///      a thematic break, not a table opening.
///    - A solid (with-header) top rule must be followed by no more than
///      `MAX_HEADER_LINES` non-blank lines before the grouped separator.
///    - The bottom dash rule must exist before EOF, and the line
///      immediately preceding it must not be blank (otherwise the rule
///      is a thematic break inside the body).
///
///    The reader is restored to the start of the top rule before
///    returning; only the top-rule line is consumed (`advance_to_eol`)
///    and a [`MultilineTableCandidate`] extension node is opened.
///
/// 2. [`cont`][Self::cont] is invoked by the dispatcher once per
///    subsequent line, *after* every parent container has stripped its
///    own prefix. The state machine pushes the post-strip segment into
///    the candidate's `lines` field, advances on grouped column rules
///    (header → body) and dash rules (body → complete), and returns
///    `None` only once a bottom rule has been seen.
///
/// 3. [`close`][Self::close] replaces the candidate with the real
///    `Table`/`TableHeader`/`TableBody`/`TableRow`/`TableCell` subtree
///    using the existing [`extract_cell_segment`] / [`compute_alignments`]
///    / [`populate_cell`] helpers. Running in `close()` (during the
///    block-parsing phase) means each freshly-emitted `TableCell.source`
///    is later picked up by `parse_block`'s inline-parsing pass — same
///    timing as the built-in `TableParagraphTransformer`. When the state
///    machine never reached [`CandidateState::Complete`] (e.g. parent
///    closed us before we saw the bottom rule), the candidate is replaced
///    with a `Paragraph` whose source is the captured lines, so no text
///    is lost.
///
/// Triggered on `b"-"` at priority **50** — ahead of setext headings
/// (100) and thematic breaks (200). `cont()` never advances the reader
/// (the engine's `advance_line` after each iteration does that); `open()`
/// advances past the top rule with `advance_to_eol`.
#[derive(Debug, Default)]
pub(crate) struct MultilineTableParser;

impl MultilineTableParser {
    pub fn new() -> Self {
        Self
    }
}

/// Maximum number of non-blank header lines tolerated between the solid
/// top rule and the grouped separator.
const MAX_HEADER_LINES: usize = 10;
/// Hard ceiling on body-section lines (incl. blanks and the bottom rule)
/// during the forward-scan. Guards against runaway scans on degenerate
/// input.
const MAX_BODY_LINES: usize = 10_000;

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
        let (saved_line, saved_pos) = reader.position();

        // ── Step 1: classify the top rule line ─────────────────────────────
        let (top_bytes, top_seg) = reader.peek_line_bytes()?;
        if top_seg.padding() > 0 {
            return None; // tab-expanded leading content unsupported
        }
        let top_offset = reader.line_offset();
        let top_bytes_ref: &[u8] = top_bytes.as_ref();
        let top_for_analysis = &top_bytes_ref[..strip_newline(top_bytes_ref)];

        let maybe_grouped = parse_column_rule(top_for_analysis, top_offset);
        let solid = maybe_grouped.is_none() && is_solid_rule(top_for_analysis, top_offset);
        if !solid && maybe_grouped.is_none() {
            return None;
        }
        let with_header = solid;
        let table_pos = top_seg.start();

        // Build the per-line parent strip by tokenising the raw prefix
        // bytes the dispatcher already consumed on the top rule line.
        // Returns None for prefix shapes we can't decode (a byte that's
        // neither whitespace nor `>`); we then decline the candidate
        // rather than guess.
        let source_bytes = reader.source().as_bytes();
        let head = find_line_head(source_bytes, top_seg.start());
        let prefix = &source_bytes[head..top_seg.start()];
        let strip_seq = ParentStripSeq::build(prefix)?;

        // ── Step 2: read-only forward-scan validation ──────────────────────
        // The reader is mutated during the scan; restored to the start of
        // the top rule before returning so cont() picks up from line 2 via
        // the engine's normal dispatch (which handles per-line parent
        // stripping for free).
        reader.advance_line();
        let scan_ok = validate_table_structure(reader, &strip_seq, with_header);
        reader.set_position(saved_line, saved_pos);

        if !scan_ok {
            return None;
        }

        // ── Step 3: commit — consume top rule, open the candidate ──────────
        let candidate = MultilineTableCandidate {
            with_header,
            top_rule_seg: top_seg,
            col_extents: if with_header {
                Vec::new()
            } else {
                maybe_grouped.expect("headerless top rule already parsed")
            },
            lines: Vec::new(),
            state: if with_header {
                CandidateState::AwaitingHeader
            } else {
                CandidateState::AwaitingBody
            },
        };
        let node_ref = arena.new_node(candidate);
        arena[node_ref].set_pos(table_pos);
        reader.advance_to_eol();

        Some((node_ref, State::NO_CHILDREN))
    }

    fn cont(
        &self,
        arena: &mut Arena,
        node_ref: NodeRef,
        reader: &mut text::BasicReader,
        _ctx: &mut Context,
    ) -> Option<State> {
        // Already complete? Close on this line so the dispatcher hands
        // control back to the parent / next block parser.
        {
            let cand = as_extension_data_mut!(arena, node_ref, MultilineTableCandidate);
            if matches!(cand.state, CandidateState::Complete) {
                return None;
            }
        }

        let (lb, ls) = reader.peek_line_bytes()?;
        if ls.padding() > 0 {
            // Tab-padded line — abort. Forward-scan would have caught a
            // tab in the rule lines; a tab in mid-row appearing here means
            // the engine's strip diverged from our forward-scan
            // assumption, so it is safer to close than to corrupt the
            // capture.
            return None;
        }
        let lb_ref: &[u8] = lb.as_ref();
        let line = &lb_ref[..strip_newline(lb_ref)];
        let offset = reader.line_offset();

        let cand = as_extension_data_mut!(arena, node_ref, MultilineTableCandidate);
        match cand.state {
            CandidateState::AwaitingHeader => {
                if let Some(cols) = parse_column_rule(line, offset) {
                    // Header → body transition. Don't capture the rule
                    // itself; col_extents now drives cell extraction.
                    cand.col_extents = cols;
                    cand.state = CandidateState::AwaitingBody;
                    Some(State::NO_CHILDREN)
                } else if is_solid_rule(line, offset) || is_blank(line) {
                    // A solid rule or a blank line before the separator
                    // contradicts the forward-scan; close so the engine
                    // can re-dispatch and the fallback path in close()
                    // preserves whatever we collected.
                    None
                } else {
                    cand.lines.push(CapturedLine {
                        seg: ls,
                        kind: LineKind::Header,
                    });
                    Some(State::NO_CHILDREN)
                }
            }
            CandidateState::AwaitingBody => {
                if is_any_dash_rule(line, offset) {
                    // Bottom rule. Do not capture the rule itself; the
                    // blank-before-bottom heuristic was enforced by the
                    // forward-scan.
                    cand.state = CandidateState::Complete;
                    Some(State::NO_CHILDREN)
                } else if is_blank(line) {
                    cand.lines.push(CapturedLine {
                        seg: ls,
                        kind: LineKind::Blank,
                    });
                    Some(State::NO_CHILDREN)
                } else {
                    cand.lines.push(CapturedLine {
                        seg: ls,
                        kind: LineKind::Body,
                    });
                    Some(State::NO_CHILDREN)
                }
            }
            CandidateState::Complete => None,
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
            return; // already removed by something else
        }
        let Some(parent_ref) = arena[node_ref].parent() else {
            return;
        };

        let source = reader.source();
        let pos = arena[node_ref].pos().unwrap_or(0);

        // Pull captured state out by value so the mutable extension
        // borrow ends before we mutate the arena tree.
        let (with_header, col_extents, captured, state, top_rule_seg) = {
            let cand = as_extension_data_mut!(arena, node_ref, MultilineTableCandidate);
            (
                cand.with_header,
                core::mem::take(&mut cand.col_extents),
                core::mem::take(&mut cand.lines),
                cand.state,
                cand.top_rule_seg,
            )
        };

        // Success criteria:
        //   - state == Complete (bottom rule was seen)
        //   - col_extents populated (separator was seen for with-header;
        //     unconditional for headerless)
        //   - at least one body line captured
        //   - with-header: at least one header line captured
        let has_body = captured.iter().any(|c| c.kind == LineKind::Body);
        let has_header = captured.iter().any(|c| c.kind == LineKind::Header);
        let succeed = matches!(state, CandidateState::Complete)
            && !col_extents.is_empty()
            && has_body
            && (!with_header || has_header);

        if !succeed {
            fallback_paragraph(arena, node_ref, parent_ref, &captured, top_rule_seg, pos);
            return;
        }

        // Partition captured lines into header_segs and body_rows.
        let mut header_segs: Vec<Segment> = Vec::new();
        let mut body_rows: Vec<Vec<Segment>> = Vec::new();
        let mut current_row: Vec<Segment> = Vec::new();
        for cl in &captured {
            match cl.kind {
                LineKind::Header => header_segs.push(cl.seg),
                LineKind::Body => current_row.push(cl.seg),
                LineKind::Blank => {
                    if !current_row.is_empty() {
                        body_rows.push(core::mem::take(&mut current_row));
                    }
                }
            }
        }
        if !current_row.is_empty() {
            body_rows.push(current_row);
        }
        if body_rows.is_empty() {
            fallback_paragraph(arena, node_ref, parent_ref, &captured, top_rule_seg, pos);
            return;
        }

        // Compute alignments and build the Table AST.
        let ref_lines: &[Segment] = if with_header {
            &header_segs
        } else {
            let first = body_rows[0].as_slice();
            &first[..1.min(first.len())]
        };
        let alignments = compute_alignments(source, &col_extents, ref_lines);
        let num_cols = col_extents.len();

        let table_ref = arena.new_node(Table::new());
        arena[table_ref].set_pos(pos);

        if with_header {
            let header_ref = arena.new_node(TableHeader::new());
            arena[header_ref].set_pos(pos);
            let row_ref = arena.new_node(TableRow::new());
            arena[row_ref].set_pos(pos);

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
        arena[body_ref].set_pos(pos);
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

        parent_ref.replace_child(arena, node_ref, table_ref);
    }

    fn can_interrupt_paragraph(&self) -> bool {
        false
    }
}

/// Read-only forward-scan that validates the table structure starting at
/// the reader's current position (which the caller has advanced past the
/// top rule). The reader is mutated during the scan; the caller is
/// responsible for restoring its position before returning from `open()`.
///
/// Each iteration peeks one logical line, applies the parent-chain strip
/// via [`ParentStripSeq::apply`] once, then classifies. The strip is
/// **not idempotent** for non-zero indent widths — it must be applied
/// exactly once per peeked line.
///
/// Returns `true` if the structure looks like a well-formed multiline
/// table, `false` otherwise.
fn validate_table_structure(
    reader: &mut text::BasicReader,
    strip_seq: &ParentStripSeq,
    with_header: bool,
) -> bool {
    // ── With-header header section: collect up to MAX_HEADER_LINES
    // non-blank lines, then expect a grouped separator. A blank line
    // immediately after the top rule means it was a thematic break.
    if with_header {
        let mut header_count = 0;
        loop {
            if matches!(strip_seq.apply(reader), StripIndent::Insufficient) {
                return false;
            }
            let Some((lb, ls)) = reader.peek_line_bytes() else {
                return false; // EOF before separator
            };
            if ls.padding() > 0 {
                return false;
            }
            let lb_ref: &[u8] = lb.as_ref();
            let la = &lb_ref[..strip_newline(lb_ref)];
            let off = reader.line_offset();

            if parse_column_rule(la, off).is_some() {
                if header_count == 0 {
                    // Solid rule + separator with no header rows.
                    return false;
                }
                reader.advance_line();
                break;
            }
            if is_blank(la) {
                // Blank inside header (incl. immediately after top rule).
                return false;
            }
            if is_solid_rule(la, off) {
                // Solid rule before separator would close the table
                // prematurely.
                return false;
            }
            header_count += 1;
            if header_count > MAX_HEADER_LINES {
                return false;
            }
            reader.advance_line();
        }
    }

    // ── Body: accept content lines and blank row-separators until a dash
    // rule closes the table. The first body line may not be blank
    // (otherwise the top rule was a thematic break — relevant for the
    // headerless form, where this loop is entered directly). The line
    // immediately before the bottom rule may not be blank either, or the
    // rule is a thematic break inside the body.
    let mut body_lines = 0usize;
    let mut body_content_seen = false;
    let mut last_was_blank = false;
    let mut first_body_line = true;
    loop {
        if matches!(strip_seq.apply(reader), StripIndent::Insufficient) {
            return false;
        }
        let Some((lb, ls)) = reader.peek_line_bytes() else {
            return false; // EOF without bottom rule
        };
        if ls.padding() > 0 {
            return false;
        }
        let lb_ref: &[u8] = lb.as_ref();
        let la = &lb_ref[..strip_newline(lb_ref)];
        let off = reader.line_offset();

        if is_any_dash_rule(la, off) {
            if last_was_blank || !body_content_seen {
                return false;
            }
            reader.advance_line();
            return true;
        }
        if is_blank(la) {
            if first_body_line {
                // Blank as the very first body line → top rule is a
                // thematic break, not a table opening.
                return false;
            }
            last_was_blank = true;
        } else {
            body_content_seen = true;
            last_was_blank = false;
        }
        first_body_line = false;
        body_lines += 1;
        if body_lines > MAX_BODY_LINES {
            return false;
        }
        reader.advance_line();
    }
}

/// Replace the candidate node with a fallback `Paragraph` whose source is
/// the captured-line segments plus the original top-rule line. This
/// preserves the input text when the candidate cannot be materialised as a
/// table — e.g. the engine closed us before `cont()` saw the bottom rule,
/// or a per-line strip discrepancy left the state machine stuck.
fn fallback_paragraph(
    arena: &mut Arena,
    node_ref: NodeRef,
    parent_ref: NodeRef,
    captured: &[CapturedLine],
    top_rule_seg: Segment,
    pos: usize,
) {
    let para_ref = arena.new_node(Paragraph::new());
    arena[para_ref].set_pos(pos);
    as_type_data_mut!(arena, para_ref, Block).append_source_line(top_rule_seg);
    for line in captured {
        if matches!(line.kind, LineKind::Blank) {
            // A paragraph cannot span a blank line — drop separators.
            continue;
        }
        as_type_data_mut!(arena, para_ref, Block).append_source_line(line.seg);
    }
    parent_ref.replace_child(arena, node_ref, para_ref);
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

    // ── Nested-container tests ───────────────────────────────────────────────
    //
    // Multiline tables must work as block-level children of containers that
    // strip a continuation indent on each line (sparse list items, definition
    // list bodies, block quotes, fenced divs). The dispatcher only invokes a
    // parent's `cont()` between *top-level* line boundaries; when our parser
    // consumes multiple lines inside one `open()` it must therefore strip the
    // parent's continuation indent itself on every line after the first.

    /// Parser with definition_list + multiline_table extensions on top of GFM.
    fn make_parser_with_deflist() -> parser::Parser {
        parser::Parser::with_extensions(
            parser::Options {
                attributes: false,
                auto_heading_ids: false,
                ..Default::default()
            },
            parser::gfm(parser::GfmOptions::default())
                .and(rushdown_definition_list::definition_list_parser_extension())
                .and(multiline_table_parser_extension()),
        )
    }

    fn parse_with_deflist(input: &str) -> (Arena, NodeRef) {
        let p = make_parser_with_deflist();
        let mut reader = BasicReader::new(input);
        p.parse(&mut reader)
    }

    /// Returns the first descendant of any of the given kind names.
    fn find_kind(arena: &Arena, root: NodeRef, name: &str) -> Option<NodeRef> {
        let mut found: Option<NodeRef> = None;
        walk(arena, root, &mut |arena: &Arena,
                                node_ref: NodeRef,
                                entering: bool|
         -> Result<WalkStatus> {
            if entering && arena[node_ref].kind_data().kind_name() == name && found.is_none() {
                found = Some(node_ref);
            }
            Ok(WalkStatus::Continue)
        })
        .ok();
        found
    }

    /// Collect non-whitespace text content of a node by walking its inline
    /// descendants. Mirrors `cell_text` semantics but works on the AST.
    fn collect_text(arena: &Arena, root: NodeRef, source: &str) -> String {
        let mut buf = String::new();
        walk(arena, root, &mut |arena: &Arena,
                                node_ref: NodeRef,
                                entering: bool|
         -> Result<WalkStatus> {
            if !entering {
                return Ok(WalkStatus::Continue);
            }
            if arena[node_ref].kind_data().kind_name() == "Text" {
                let text = rushdown::as_kind_data!(arena, node_ref, Text);
                let s = text.str(source);
                if !buf.is_empty() && !buf.ends_with(' ') {
                    buf.push(' ');
                }
                buf.push_str(s);
            }
            Ok(WalkStatus::Continue)
        })
        .ok();
        buf.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn table_inside_sparse_list_item_ascii() {
        // ASCII content keeps the byte/column mapping trivial so any column
        // misalignment shows up as wrong cell content rather than invalid
        // UTF-8. Two body rows separated by a blank line; bottom rule closes
        // the table.
        let input = "\
* item 1

* item 2 contains a table:

  ----- -----
   A     B

   1     2
  ----- -----

* item 3
";
        let html = render(input);
        assert!(html.contains("<ul"), "list present: {html}");
        assert!(html.contains("<table"), "table inside list: {html}");
        // All four body cell values must be intact (two rows × two cols).
        for needle in ["A", "B", "1", "2"] {
            assert!(
                html.contains(&format!(">{needle}<")) || html.contains(&format!("> {needle} <")),
                "body cell `{needle}` missing from {html}"
            );
        }
        // Must not collapse to thematic-break + paragraphs.
        assert!(
            !html.contains("<hr"),
            "table close rule must not degrade to <hr>: {html}"
        );
    }

    #[test]
    fn table_inside_sparse_list_item_cyrillic() {
        // Regression for the case where the parser sliced body-line bytes
        // using column extents derived from the *un-indented* top rule. With
        // Cyrillic content the misaligned byte offset split a 2-byte UTF-8
        // character, producing invalid UTF-8 and dropping the cell text.
        let input = "\
* Элемент 1

* Элемент 2 содержит таблицу:

  -------  -------
  Ячейка   Ячейка
  А        Б
  -------  -------

* Элемент 3
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table node");

        // Headerless table — the dash rule has two groups.
        assert!(
            find_first!(&arena, table, TableHeader).is_none(),
            "headerless table must not have a TableHeader"
        );

        // Both cells must be left-aligned: header text is flush-left in its
        // column extent ("Ячейка" starts at col 2 and ends at col 7, the
        // column extends to col 7 — flush both sides → Default/None, but
        // critically NOT Right.
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 1, "one body row (two physical lines joined)");
        let cells = children_of_kind(&arena, rows[0], "TableCell");
        assert_eq!(cells.len(), 2, "two body cells");

        // Cell contents must contain BOTH physical lines' text. The bug
        // dropped "Ячейка" from cell[0] line 0 entirely.
        let c0 = collect_text(&arena, cells[0], input);
        let c1 = collect_text(&arena, cells[1], input);
        assert!(
            c0.contains("Ячейка") && c0.contains("А"),
            "cell[0] must contain both lines, got {c0:?}"
        );
        assert!(
            c1.contains("Ячейка") && c1.contains("Б"),
            "cell[1] must contain both lines, got {c1:?}"
        );

        // Alignment must NOT be Right (which is what the buggy parser
        // produced — the leading list indent made the column-0 text look
        // flush-right within the un-stripped column extent).
        for cell in &cells {
            let align = rushdown::as_kind_data!(arena, *cell, TableCell).alignment();
            assert!(
                !matches!(align, TableCellAlignment::Right),
                "cell alignment must not be Right (bug), got {align:?}"
            );
        }
    }

    #[test]
    fn table_inside_definition_list_body() {
        // Regression for the case where multiline tables inside a definition
        // list body were not recognised at all: the parser saw the body /
        // bottom rule lines with the 4-column def-list continuation indent
        // still attached, `indent_width > 3` failed `is_any_dash_rule` on
        // the bottom rule, the parser exhausted to EOF and rewound, and the
        // ThematicBreak parser claimed the dashes instead.
        let input = "\
Term

:   First paragraph of the definition.

    Second paragraph of the same definition.

    -------  -------
    Ячейка   Ячейка
    А        Б
    -------  -------
";
        let (arena, doc) = parse_with_deflist(input);
        // A DefinitionList must exist and contain a Table inside its
        // TermDefinition body.
        let dl = find_kind(&arena, doc, "DefinitionList").expect("no DefinitionList");
        let td = find_kind(&arena, dl, "TermDefinition").expect("no TermDefinition");
        let table = find_first!(&arena, td, Table)
            .expect("table must be parsed inside the definition list body");

        // Sanity: it's a headerless table with one body row of two cells.
        assert!(find_first!(&arena, table, TableHeader).is_none());
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 1);
        let cells = children_of_kind(&arena, rows[0], "TableCell");
        assert_eq!(cells.len(), 2);

        let c0 = collect_text(&arena, cells[0], input);
        let c1 = collect_text(&arena, cells[1], input);
        assert!(
            c0.contains("Ячейка") && c0.contains("А"),
            "cell[0] must contain both lines, got {c0:?}"
        );
        assert!(
            c1.contains("Ячейка") && c1.contains("Б"),
            "cell[1] must contain both lines, got {c1:?}"
        );
    }

    #[test]
    fn table_inside_blockquote() {
        // Direct port of the bug reproducer.
        //
        // Before the blockquote-aware strip, the table parser saw each
        // continuation line with the parent's `>` marker still attached.
        // `strip_container_indent` (whitespace-only) returned `Insufficient`
        // at the first `>` byte, the parse aborted, and `ThematicBreak`
        // (priority 200) claimed the dashes — top and bottom rule degraded
        // to `<hr>`, body lines became paragraphs.
        //
        // After the fix, `classify_parent_chain` sees a `Blockquote`-only
        // ancestor chain and `strip_blockquote_layers` replays the marker
        // strip per line, including the `>\n` row-separator line.
        let input = "\
> ----- -----
>  A     B
>
>  1     2
> ----- -----
";
        let (arena, doc) = parse(input);

        // The table must live inside a Blockquote.
        let bq = find_kind(&arena, doc, "Blockquote").expect("no Blockquote");
        let table =
            find_first!(&arena, bq, Table).expect("table must be parsed inside the blockquote");

        // Headerless (grouped top rule, no solid rule above the header).
        assert!(
            find_first!(&arena, table, TableHeader).is_none(),
            "headerless table must not have a TableHeader"
        );

        // Two body rows, each with two cells.
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(
            rows.len(),
            2,
            "two body rows (blank `>` line is the separator)"
        );
        for row in &rows {
            let cells = children_of_kind(&arena, *row, "TableCell");
            assert_eq!(cells.len(), 2, "two cells per row");
        }

        let r0_cells = children_of_kind(&arena, rows[0], "TableCell");
        let r1_cells = children_of_kind(&arena, rows[1], "TableCell");
        assert_eq!(collect_text(&arena, r0_cells[0], input), "A");
        assert_eq!(collect_text(&arena, r0_cells[1], input), "B");
        assert_eq!(collect_text(&arena, r1_cells[0], input), "1");
        assert_eq!(collect_text(&arena, r1_cells[1], input), "2");

        // The dash rules must NOT degrade to thematic breaks.
        assert!(
            find_first!(&arena, doc, ThematicBreak).is_none(),
            "dash rules must not degrade to <hr>"
        );

        // Render-side sanity: <table> is enclosed by <blockquote>.
        let html = render(input);
        let bq_open = html.find("<blockquote").expect("no <blockquote");
        let bq_close = html.find("</blockquote>").expect("no </blockquote>");
        let table_open = html.find("<table").expect("no <table");
        assert!(
            bq_open < table_open && table_open < bq_close,
            "<table> must be inside <blockquote>: {html}"
        );
        assert!(!html.contains("<hr"), "no <hr> in rendered HTML: {html}");
    }

    #[test]
    fn table_inside_blockquote_cyrillic() {
        // Keeps the UTF-8 boundary regression test alive in the blockquote
        // code path. Each Cyrillic character is two UTF-8 bytes; if the
        // parser ever sliced cell content by raw byte offsets derived from
        // the (ASCII) dash rule, this test would produce invalid UTF-8 in
        // the rendered output.
        let input = "\
> ----------- -----------
>  Ячейка1     Ячейка2
>
>  Значение1   Значение2
> ----------- -----------
";
        let (arena, doc) = parse(input);

        let bq = find_kind(&arena, doc, "Blockquote").expect("no Blockquote");
        let table =
            find_first!(&arena, bq, Table).expect("table must be parsed inside the blockquote");

        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 2, "two body rows");

        let r0_cells = children_of_kind(&arena, rows[0], "TableCell");
        let r1_cells = children_of_kind(&arena, rows[1], "TableCell");
        assert_eq!(collect_text(&arena, r0_cells[0], input), "Ячейка1");
        assert_eq!(collect_text(&arena, r0_cells[1], input), "Ячейка2");
        assert_eq!(collect_text(&arena, r1_cells[0], input), "Значение1");
        assert_eq!(collect_text(&arena, r1_cells[1], input), "Значение2");

        // Rendered HTML must be valid UTF-8 and structurally well-formed.
        let html = render(input);
        assert!(
            html.is_char_boundary(0),
            "rendered HTML must be valid UTF-8"
        );
        for needle in ["Ячейка1", "Ячейка2", "Значение1", "Значение2"] {
            assert!(html.contains(needle), "{needle} missing from {html}");
        }
        assert!(!html.contains("<hr"), "no <hr> in rendered HTML: {html}");
    }

    // ── Redesign-specific regression tests ──────────────────────────────────
    //
    // After moving line collection into cont() and the table-AST build into
    // close(), the parser no longer fights parent containers — each line is
    // pre-stripped by the dispatcher. These tests exercise nesting depths
    // and edge cases the old "consume-everything-in-open" design couldn't
    // handle.

    #[test]
    fn two_deep_blockquote_table() {
        // Table inside two levels of `>`. The dispatcher invokes each
        // blockquote's cont() in turn before reaching the table's cont(),
        // so we just see the stripped content.
        let input = "\
> > ----- -----
> >  A     B
> >
> >  1     2
> > ----- -----
";
        let (arena, doc) = parse(input);
        // Two blockquotes.
        let outer = find_kind(&arena, doc, "Blockquote").expect("no outer Blockquote");
        // find_kind returns the root if it matches; walk children manually.
        let inner = children_of_kind(&arena, outer, "Blockquote")
            .into_iter()
            .next()
            .expect("no inner Blockquote");
        let table = find_first!(&arena, inner, Table).expect("no Table in inner blockquote");
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 2, "two body rows: {input}");

        let r0_cells = children_of_kind(&arena, rows[0], "TableCell");
        let r1_cells = children_of_kind(&arena, rows[1], "TableCell");
        assert_eq!(collect_text(&arena, r0_cells[0], input), "A");
        assert_eq!(collect_text(&arena, r0_cells[1], input), "B");
        assert_eq!(collect_text(&arena, r1_cells[0], input), "1");
        assert_eq!(collect_text(&arena, r1_cells[1], input), "2");
    }

    #[test]
    fn three_deep_blockquote_table() {
        // Triple-nested blockquote. Demonstrates that the new design
        // composes naturally with depth: cont() never sees `>` markers,
        // because each blockquote ancestor strips its own layer.
        let input = "\
> > > ----- -----
> > >  A     B
> > >
> > >  1     2
> > > ----- -----
";
        let (arena, doc) = parse(input);
        let table =
            find_first!(&arena, doc, Table).expect("no Table parsed inside three-deep blockquote");
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn table_inside_blockquote_with_header() {
        // Verifies the header-form table works inside a blockquote: a
        // solid top rule, header lines, grouped separator, body rows,
        // and a solid bottom rule — every line carries a `>` marker.
        let input = "\
> ---------
>  Col1 Col2
> ---- ----
>  a    b
> ---------
";
        let (arena, doc) = parse(input);
        let bq = find_kind(&arena, doc, "Blockquote").expect("no Blockquote");
        let table = find_first!(&arena, bq, Table).expect("no Table inside blockquote");
        let header = find_first!(&arena, table, TableHeader).expect("no TableHeader");
        let header_rows = children_of_kind(&arena, header, "TableRow");
        assert_eq!(header_rows.len(), 1);
        let header_cells = children_of_kind(&arena, header_rows[0], "TableCell");
        assert_eq!(header_cells.len(), 2);

        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let body_rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(body_rows.len(), 1);
    }

    #[test]
    fn dash_rule_followed_by_blank_is_thematic_break() {
        // Forward-scan heuristic: a dash rule immediately followed by a
        // blank (post-strip) line is a thematic break, NOT a table
        // opening. The candidate must NOT be allocated; the line falls
        // through to the thematic-break parser.
        let input = "---\n\nsome paragraph\n";
        let html = render(input);
        assert!(
            html.contains("<hr"),
            "single dash rule followed by blank line must be <hr>: {html}"
        );
        assert!(!html.contains("<table"), "must not produce <table>: {html}");
    }

    #[test]
    fn dash_rule_followed_by_blank_inside_blockquote_is_thematic_break() {
        // Same heuristic, but inside a blockquote. The forward-scan
        // applies the blockquote strip on the next line; if the post-
        // strip view is blank, reject.
        let input = "\
> ---
>
> some paragraph
";
        let html = render(input);
        // Either an <hr> inside the blockquote, or fall-through to other
        // parsers. The key is NOT producing a <table>.
        assert!(
            !html.contains("<table"),
            "blockquote `> ---\\n>\\n` must not become a table: {html}"
        );
    }

    #[test]
    fn blank_before_bottom_rule_falls_through() {
        // Forward-scan heuristic: a blank line immediately before what
        // looks like the bottom rule means the rule is a thematic break,
        // not the table's closing rule. Reject the whole thing.
        let input = "\
----- -----
 A     B
 1     2

----- -----
";
        let html = render(input);
        // The blank before `----- -----` makes the second grouped rule
        // a thematic break, so no <table> should be produced.
        assert!(
            !html.contains("<table"),
            "blank before bottom rule must reject the table: {html}"
        );
    }

    #[test]
    fn body_only_blank_lines_falls_through() {
        // Forward-scan heuristic: at least one non-blank body line is
        // required before the bottom rule.
        let input = "\
----- -----

----- -----
";
        let html = render(input);
        assert!(
            !html.contains("<table"),
            "table with no body content must reject: {html}"
        );
    }

    #[test]
    fn ten_header_lines_accepted() {
        // The MAX_HEADER_LINES heuristic accepts exactly 10 non-blank
        // header lines before the grouped separator.
        let mut input = String::from("----------\n");
        for i in 0..10 {
            input.push_str(&format!("h{i}h{i}h{i}h{i}\n"));
        }
        input.push_str("---- -----\n");
        input.push_str(" a    b\n");
        input.push_str("----------\n");
        let html = render(&input);
        assert!(
            html.contains("<table"),
            "10 header lines must be accepted: {html}"
        );
    }

    #[test]
    fn eleven_header_lines_rejected() {
        // 11 non-blank header lines exceed MAX_HEADER_LINES.
        let mut input = String::from("----------\n");
        for i in 0..11 {
            input.push_str(&format!("h{i}h{i}h{i}h{i}\n"));
        }
        input.push_str("---- -----\n");
        input.push_str(" a    b\n");
        input.push_str("----------\n");
        let html = render(&input);
        assert!(
            !html.contains("<table"),
            "11 header lines must be rejected: {html}"
        );
    }

    // ── Mixed parent chain tests (pandoc parity) ────────────────────────────
    //
    // After decoding the parent-strip from the top rule's raw prefix, the
    // forward-scan handles arbitrary nesting of List/Blockquote ancestors.
    // Each case below is verified against pandoc 3.x output.

    #[test]
    fn table_inside_blockquote_inside_list_headerless() {
        // List → Blockquote → headerless table.
        // Pandoc emits: list > blockquote > <p>blockquote starts</p>
        // > headerless table with one row of three right-aligned cells.
        let input = "\
* list item

  > blockquote starts
  >
  > ----- ---- ----
  >     1    2    3
  > ----- ---- ----
";
        let (arena, doc) = parse(input);
        // Find: List → ListItem → Blockquote → Table
        let list = find_kind(&arena, doc, "List").expect("no List");
        let li = find_kind(&arena, list, "ListItem").expect("no ListItem");
        let bq = children_of_kind(&arena, li, "Blockquote")
            .into_iter()
            .next()
            .expect("no Blockquote inside list item");
        let table =
            find_first!(&arena, bq, Table).expect("no Table inside blockquote inside list item");
        assert!(
            find_first!(&arena, table, TableHeader).is_none(),
            "headerless"
        );
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 1, "one body row");
        let cells = children_of_kind(&arena, rows[0], "TableCell");
        assert_eq!(cells.len(), 3, "three cells");
        assert_eq!(collect_text(&arena, cells[0], input), "1");
        assert_eq!(collect_text(&arena, cells[1], input), "2");
        assert_eq!(collect_text(&arena, cells[2], input), "3");
        // Per pandoc parity, all three cells are right-aligned (flush-right
        // within their column extents).
        for cell in &cells {
            let align = rushdown::as_kind_data!(arena, *cell, TableCell).alignment();
            assert!(
                matches!(align, TableCellAlignment::Right),
                "cell must be right-aligned, got {align:?}"
            );
        }
    }

    #[test]
    fn table_inside_blockquote_inside_list_with_header() {
        // List → Blockquote → with-header table. The separator dashes
        // must extend at least as wide as the header text in each column
        // (per pandoc multiline-table column-extent semantics).
        let input = "\
* list item

  > blockquote
  >
  > ------------
  >  Col1 Col2
  > ----- ------
  >  a    b
  > ------------
";
        let (arena, doc) = parse(input);
        let list = find_kind(&arena, doc, "List").expect("no List");
        let li = find_kind(&arena, list, "ListItem").expect("no ListItem");
        let bq = children_of_kind(&arena, li, "Blockquote")
            .into_iter()
            .next()
            .expect("no Blockquote in list item");
        let table = find_first!(&arena, bq, Table).expect("no Table");
        let header = find_first!(&arena, table, TableHeader).expect("no header");
        let body = find_first!(&arena, table, TableBody).expect("no body");
        assert_eq!(children_of_kind(&arena, header, "TableRow").len(), 1);
        assert_eq!(children_of_kind(&arena, body, "TableRow").len(), 1);
        let header_cells = children_of_kind(
            &arena,
            children_of_kind(&arena, header, "TableRow")[0],
            "TableCell",
        );
        assert_eq!(header_cells.len(), 2);
        assert_eq!(collect_text(&arena, header_cells[0], input), "Col1");
        assert_eq!(collect_text(&arena, header_cells[1], input), "Col2");
    }

    #[test]
    fn table_inside_list_inside_blockquote() {
        // Blockquote → List → headerless table (the reverse of the
        // previous case). Pandoc emits two body rows, center-aligned.
        let input = "\
> * list inside blockquote
>
>   ----- -----
>    A     B
>
>    1     2
>   ----- -----
";
        let (arena, doc) = parse(input);
        let bq = find_kind(&arena, doc, "Blockquote").expect("no Blockquote");
        let list = find_kind(&arena, bq, "List").expect("no List inside blockquote");
        let li = find_kind(&arena, list, "ListItem").expect("no ListItem");
        let table = find_first!(&arena, li, Table).expect("no Table inside list inside blockquote");
        assert!(
            find_first!(&arena, table, TableHeader).is_none(),
            "headerless"
        );
        let body = find_first!(&arena, table, TableBody).expect("no body");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 2, "two body rows");
        let r0 = children_of_kind(&arena, rows[0], "TableCell");
        let r1 = children_of_kind(&arena, rows[1], "TableCell");
        assert_eq!(collect_text(&arena, r0[0], input), "A");
        assert_eq!(collect_text(&arena, r0[1], input), "B");
        assert_eq!(collect_text(&arena, r1[0], input), "1");
        assert_eq!(collect_text(&arena, r1[1], input), "2");
    }

    #[test]
    fn table_inside_nested_lists() {
        // Two-level homogeneous list nesting → with-header table. Uses
        // the solid-rule form (our parser's documented header trigger);
        // pandoc accepts grouped-top + grouped-middle as with-header too,
        // but that's a behavioural extension outside the current
        // contract.
        let input = "\
* outer

  * inner

    ----------
     Col1 Col2
    ---- -----
     a    b
    ----------
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table inside nested lists");
        let header = find_first!(&arena, table, TableHeader).expect("no header");
        let body = find_first!(&arena, table, TableBody).expect("no body");
        assert_eq!(children_of_kind(&arena, header, "TableRow").len(), 1);
        assert_eq!(children_of_kind(&arena, body, "TableRow").len(), 1);
    }

    #[test]
    fn mixed_parent_chain_list_then_blockquote_no_intro_paragraph() {
        // List → ListItem → Blockquote → headerless table, with the
        // blockquote opening *directly* on the table's top rule (no
        // introductory paragraph inside the blockquote). The byte-driven
        // [`ParentStripSeq::build`] tokenises the dispatcher's prefix
        // (`  > ` = 2-col list indent + blockquote marker + space) into
        // `[Whitespace { cols: 2 }, Blockquote]` regardless of the AST
        // chain's shape.
        let input = "\
* item

  > ----- -----
  >  A     B
  >
  >  1     2
  > ----- -----
";
        let (arena, doc) = parse(input);
        let list = find_kind(&arena, doc, "List").expect("no List");
        let li = find_kind(&arena, list, "ListItem").expect("no ListItem");
        let bq = find_kind(&arena, li, "Blockquote").expect("no Blockquote in list item");
        let table = find_first!(&arena, bq, Table).expect("no Table inside blockquote");
        assert!(
            find_first!(&arena, table, TableHeader).is_none(),
            "headerless"
        );
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 2, "two body rows separated by `>` blank line");
        let r0 = children_of_kind(&arena, rows[0], "TableCell");
        let r1 = children_of_kind(&arena, rows[1], "TableCell");
        assert_eq!(collect_text(&arena, r0[0], input), "A");
        assert_eq!(collect_text(&arena, r0[1], input), "B");
        assert_eq!(collect_text(&arena, r1[0], input), "1");
        assert_eq!(collect_text(&arena, r1[1], input), "2");
    }

    #[test]
    fn table_after_paragraph_with_blank_line() {
        // Standard separation: paragraph, blank line, table. Must produce
        // both the paragraph and the table.
        let input = "\
Some intro text.

----- -----
 A     B
----- -----
 1     2
----- -----
";
        let html = render(input);
        assert!(html.contains("Some intro text"), "paragraph: {html}");
        assert!(html.contains("<table"), "table: {html}");
    }

    #[test]
    fn table_inside_sparse_list_three_rows() {
        // Multiple body rows inside a list item, separated by blank lines.
        // Each blank line must be recognised as a row separator (not a
        // table terminator) after the dispatcher strips the list's
        // continuation indent.
        let input = "\
* The table:

  ----- -----
   A     B

   C     D

   E     F
  ----- -----
";
        let (arena, doc) = parse(input);
        let table = find_first!(&arena, doc, Table).expect("no Table");
        let body = find_first!(&arena, table, TableBody).expect("no TableBody");
        let rows = children_of_kind(&arena, body, "TableRow");
        assert_eq!(rows.len(), 3, "three body rows: {input}");
    }

    #[test]
    fn def_list_body_table_with_header() {
        // With-header table inside a definition list body (4-col content
        // indent). The dispatcher strips 4 cols on every line; our
        // forward-scan classifies the parent as Whitespace { width: 4 }.
        let input = "\
Term

:   First definition paragraph.

    ----------
     Col1 Col2
    ---- -----
     a    b
    ----------
";
        let (arena, doc) = parse_with_deflist(input);
        let dl = find_kind(&arena, doc, "DefinitionList").expect("no DefinitionList");
        let td = find_kind(&arena, dl, "TermDefinition").expect("no TermDefinition");
        let table = find_first!(&arena, td, Table).expect("no Table in def-list body");
        let header = find_first!(&arena, table, TableHeader).expect("no header");
        let body = find_first!(&arena, table, TableBody).expect("no body");
        assert!(children_of_kind(&arena, header, "TableRow").len() == 1);
        assert!(children_of_kind(&arena, body, "TableRow").len() == 1);
    }
}
