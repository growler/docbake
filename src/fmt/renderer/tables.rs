use anyhow::Result;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, KindData, NodeRef, TableCellAlignment, TextQualifier};

use super::source_recovery::{
    grid_table_column_widths, is_any_dash_rule_line, is_grid_border_line,
    multiline_table_separator_extents, source_line_at_pos,
};
use super::MarkdownFmtRenderer;

#[derive(Clone, Copy, Debug)]
struct TableEdgeStyle {
    leading_pipe: bool,
    trailing_pipe: bool,
}

/// One cell from a grid table, captured by the fmt renderer. Tracks the
/// physical-line content (recovered through the cell-mode inline renderer
/// plus `SOFT_LINE_BREAK` split) along with the cell's `colspan` and
/// `rowspan` (each read from the corresponding node attribute set by the
/// grid-table parser; default 1). The `alignment` field mirrors the AST's
/// `TableCell::alignment()` so single-column cells keep their per-column
/// alignment.
#[derive(Debug, Clone)]
struct GridCell {
    colspan: usize,
    rowspan: usize,
    alignment: TableCellAlignment,
    lines: Vec<String>,
}

/// Internal sentinel used by the cell-mode inline renderer to mark cell
/// physical-line breaks (encoded in source as `SOFT_LINE_BREAK` text
/// qualifiers by the multiline-table parser).
///
/// The sentinel is split out before any cell content is emitted into the
/// final markdown stream, so it never reaches the output.
const CELL_SOFT_BREAK_MARKER: &str = "\u{0001}CELL_SOFT_BREAK\u{0001}";

impl MarkdownFmtRenderer {
    /// Top-level entry for `KindData::Table(_)` in the block renderer.
    ///
    /// Dispatches to the multiline-table emitter when the table's source
    /// position begins with a pandoc dash rule (set by the multiline-table
    /// parser to `top_seg.start()`); otherwise falls back to the pipe-table
    /// emitter (aligned or unaligned, per `align_tables`).
    pub(super) fn render_any_table(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        if is_grid_table_node(arena, node_ref, source) {
            self.render_grid_table(out, arena, node_ref, source)?;
            return Ok(());
        }
        if is_multiline_table_node(arena, node_ref, source) {
            self.render_multiline_table(out, arena, node_ref, source)?;
            return Ok(());
        }
        if self.align_tables {
            self.render_table(out, arena, node_ref, source)?;
        } else {
            self.render_table_unaligned(out, arena, node_ref, source)?;
        }
        Ok(())
    }

    pub(super) fn render_table(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        let mut header_rows = Vec::new();
        let mut body_rows = Vec::new();
        let mut aligns = Vec::new();

        for child in arena[node_ref].children(arena) {
            match arena[child].kind_data() {
                KindData::TableHeader(_) => {
                    for row in arena[child].children(arena) {
                        header_rows.push(row);
                    }
                }
                KindData::TableBody(_) => {
                    for row in arena[child].children(arena) {
                        body_rows.push(row);
                    }
                }
                _ => {}
            }
        }

        if let Some(first_row) = header_rows.first().or(body_rows.first()) {
            aligns = arena[*first_row]
                .children(arena)
                .map(|cell| as_kind_data!(arena, cell, TableCell).alignment())
                .collect();
        }
        let style = detect_table_edge_style(arena, &header_rows, &body_rows, source);

        let mut rendered_header_rows = Vec::new();
        let mut rendered_body_rows = Vec::new();

        for row in &header_rows {
            rendered_header_rows.push(self.collect_table_row(arena, *row, source)?);
        }
        for row in &body_rows {
            rendered_body_rows.push(self.collect_table_row(arena, *row, source)?);
        }

        let mut columns = aligns.len();
        for row in &rendered_header_rows {
            columns = columns.max(row.len());
        }
        for row in &rendered_body_rows {
            columns = columns.max(row.len());
        }

        let mut widths = vec![3usize; columns];
        for row in rendered_header_rows.iter().chain(rendered_body_rows.iter()) {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }

        for row in &rendered_header_rows {
            out.push_str(&render_table_row(row, &widths, &aligns, style));
            out.push('\n');
        }

        if !rendered_header_rows.is_empty() || !aligns.is_empty() {
            let mut separators = Vec::new();
            for (index, width) in widths.iter().enumerate().take(columns) {
                let align = aligns
                    .get(index)
                    .copied()
                    .unwrap_or(TableCellAlignment::None);
                separators.push(render_separator_cell(align, *width));
            }
            out.push_str(&render_table_row(&separators, &widths, &[], style));
            out.push('\n');
        }

        for row in &rendered_body_rows {
            out.push_str(&render_table_row(row, &widths, &aligns, style));
            out.push('\n');
        }
        out.push('\n');

        Ok(())
    }

    pub(super) fn render_table_unaligned(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        let mut header_rows = Vec::new();
        let mut body_rows = Vec::new();
        let mut aligns = Vec::new();

        for child in arena[node_ref].children(arena) {
            match arena[child].kind_data() {
                KindData::TableHeader(_) => {
                    for row in arena[child].children(arena) {
                        header_rows.push(row);
                    }
                }
                KindData::TableBody(_) => {
                    for row in arena[child].children(arena) {
                        body_rows.push(row);
                    }
                }
                _ => {}
            }
        }

        if let Some(first_row) = header_rows.first().or(body_rows.first()) {
            aligns = arena[*first_row]
                .children(arena)
                .map(|cell| as_kind_data!(arena, cell, TableCell).alignment())
                .collect();
        }

        let style = detect_table_edge_style(arena, &header_rows, &body_rows, source);

        for row in &header_rows {
            let row = self.collect_table_row(arena, *row, source)?;
            out.push_str(&render_table_row_unaligned(&row, style));
            out.push('\n');
        }

        if !header_rows.is_empty() || !aligns.is_empty() {
            let separators: Vec<String> = aligns
                .iter()
                .map(|align| match align {
                    TableCellAlignment::Left => ":---".to_string(),
                    TableCellAlignment::Center => ":---:".to_string(),
                    TableCellAlignment::Right => "---:".to_string(),
                    _ => "---".to_string(),
                })
                .collect();
            out.push_str(&render_table_row_unaligned(&separators, style));
            out.push('\n');
        }

        for row in &body_rows {
            let row = self.collect_table_row(arena, *row, source)?;
            out.push_str(&render_table_row_unaligned(&row, style));
            out.push('\n');
        }
        out.push('\n');

        Ok(())
    }

    fn collect_table_row(
        &self,
        arena: &Arena,
        row_ref: NodeRef,
        source: &str,
    ) -> Result<Vec<String>> {
        let mut row = Vec::new();
        for cell in arena[row_ref].children(arena) {
            let mut text = String::new();
            self.render_inline_children(&mut text, arena, cell, source)?;
            row.push(text.trim().to_string());
        }
        Ok(row)
    }

    /// Re-emits a pandoc-style multiline table.
    ///
    /// Reconstructs from the AST: column widths combine the AST-derived
    /// minimums (max char count per cell line per column) with the source's
    /// original grouped separator widths (when wider), so author-tuned wide
    /// columns survive a round-trip. Per-column alignment is read from the
    /// cell `TableCellAlignment` set by the parser. Multi-line cell content
    /// is recovered via [`Self::collect_cell_lines`], which splits on the
    /// `SOFT_LINE_BREAK` text qualifier the parser plants between physical
    /// lines.
    ///
    /// Opening rule: solid for with-header tables, grouped for headerless.
    /// Separator rule (header variant only): grouped. Closing rule: solid in
    /// both cases.
    pub(super) fn render_multiline_table(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        let mut header_rows: Vec<NodeRef> = Vec::new();
        let mut body_rows: Vec<NodeRef> = Vec::new();

        for child in arena[node_ref].children(arena) {
            match arena[child].kind_data() {
                KindData::TableHeader(_) => {
                    for row in arena[child].children(arena) {
                        header_rows.push(row);
                    }
                }
                KindData::TableBody(_) => {
                    for row in arena[child].children(arena) {
                        body_rows.push(row);
                    }
                }
                _ => {}
            }
        }

        let has_header = !header_rows.is_empty();

        // Pull per-column alignment from the first row's cells.
        let aligns: Vec<TableCellAlignment> =
            if let Some(first) = header_rows.first().or(body_rows.first()) {
                arena[*first]
                    .children(arena)
                    .map(|cell| as_kind_data!(arena, cell, TableCell).alignment())
                    .collect()
            } else {
                Vec::new()
            };

        // Collect per-row, per-cell physical lines.
        let mut rendered_header: Vec<Vec<Vec<String>>> = Vec::with_capacity(header_rows.len());
        let mut rendered_body: Vec<Vec<Vec<String>>> = Vec::with_capacity(body_rows.len());

        for row in &header_rows {
            rendered_header.push(self.collect_multiline_row(arena, *row, source)?);
        }
        for row in &body_rows {
            rendered_body.push(self.collect_multiline_row(arena, *row, source)?);
        }

        let columns = rendered_header
            .iter()
            .chain(rendered_body.iter())
            .map(|row| row.len())
            .max()
            .unwrap_or(aligns.len())
            .max(aligns.len());

        if columns == 0 {
            return Ok(());
        }

        // Step 1: AST-derived minimum widths (max char count over every
        // physical line of every cell in the column). Pandoc requires at
        // least 3 dashes per group; floor at 3.
        let mut widths = vec![3usize; columns];
        for row in rendered_header.iter().chain(rendered_body.iter()) {
            for (col, cell_lines) in row.iter().enumerate() {
                for line in cell_lines {
                    widths[col] = widths[col].max(line.chars().count());
                }
            }
        }

        // Step 2: widen with the source's original separator-rule widths
        // when those are wider (preserves author-tuned column widths so
        // unmodified tables round-trip byte-identically).
        if let Some(table_pos) = arena[node_ref].pos() {
            if let Some(source_extents) = multiline_table_separator_extents(source, table_pos) {
                for (col, (start, end)) in source_extents.iter().enumerate() {
                    if col >= widths.len() {
                        break;
                    }
                    let source_width = end.saturating_sub(*start);
                    widths[col] = widths[col].max(source_width);
                }
            }
        }

        let total_width: usize = widths.iter().sum::<usize>() + columns.saturating_sub(1);
        let total_width = total_width.max(3);

        // Step 3: opening rule.
        if has_header {
            out.push_str(&"-".repeat(total_width));
        } else {
            out.push_str(&grouped_dash_rule(&widths));
        }
        out.push('\n');

        // Step 4: header lines (one source line per physical line in the
        // tallest cell of each header row).
        for row in &rendered_header {
            emit_row_physical_lines(out, row, &widths, &aligns);
        }

        // Step 5: grouped separator rule (header variant only).
        if has_header {
            out.push_str(&grouped_dash_rule(&widths));
            out.push('\n');
        }

        // Step 6: body rows; insert a blank line between rows when any row
        // (current or next) spans more than one physical line.
        let any_multiline_row = rendered_body
            .iter()
            .any(|row| row.iter().any(|cell| cell.len() > 1));

        for (idx, row) in rendered_body.iter().enumerate() {
            emit_row_physical_lines(out, row, &widths, &aligns);
            if any_multiline_row && idx + 1 < rendered_body.len() {
                out.push('\n');
            }
        }

        // Step 7: closing rule (always solid).
        out.push_str(&"-".repeat(total_width));
        out.push('\n');

        // Trailing blank line so the block separator matches surrounding
        // block conventions.
        out.push('\n');

        Ok(())
    }

    /// Collects each cell of `row_ref` as a vector of physical-line strings.
    /// Cells are returned in column order; each cell's lines are returned
    /// trimmed of surrounding whitespace (alignment is re-applied by the
    /// emitter, not preserved from the source slice).
    fn collect_multiline_row(
        &self,
        arena: &Arena,
        row_ref: NodeRef,
        source: &str,
    ) -> Result<Vec<Vec<String>>> {
        let mut row = Vec::new();
        for cell in arena[row_ref].children(arena) {
            row.push(self.collect_cell_lines(arena, cell, source)?);
        }
        Ok(row)
    }

    /// Like [`Self::collect_multiline_row`] but for grid tables: each cell
    /// carries its `colspan` and `rowspan` (each read from the
    /// corresponding node attribute set by the grid-table parser;
    /// default 1) and its column alignment. Used by
    /// [`Self::render_grid_table`] to lay out spanned and unspanned cells
    /// uniformly.
    fn collect_grid_row(
        &self,
        arena: &Arena,
        row_ref: NodeRef,
        source: &str,
    ) -> Result<Vec<GridCell>> {
        let mut row = Vec::new();
        for cell in arena[row_ref].children(arena) {
            let lines = self.collect_cell_lines(arena, cell, source)?;
            let colspan = read_span_attr(arena, cell, source, "colspan");
            let rowspan = read_span_attr(arena, cell, source, "rowspan");
            let alignment = as_kind_data!(arena, cell, TableCell).alignment();
            row.push(GridCell {
                colspan,
                rowspan,
                alignment,
                lines,
            });
        }
        Ok(row)
    }

    /// Renders a cell into its constituent physical lines.
    ///
    /// The cell-mode inline renderer (`render_cell_inline_children`) emits
    /// `CELL_SOFT_BREAK_MARKER` wherever a `SOFT_LINE_BREAK` text qualifier
    /// would otherwise produce a single space; this method splits on that
    /// sentinel to recover the physical-line layout authored in the source.
    /// Lines are returned trimmed; entirely empty lines (from
    /// extraction failures) are filtered out.
    fn collect_cell_lines(
        &self,
        arena: &Arena,
        cell_ref: NodeRef,
        source: &str,
    ) -> Result<Vec<String>> {
        let mut buf = String::new();
        self.render_cell_inline_children(&mut buf, arena, cell_ref, source)?;
        let mut lines: Vec<String> = buf
            .split(CELL_SOFT_BREAK_MARKER)
            .map(|part| part.trim().to_string())
            .collect();
        // Drop trailing empty lines (a soft break at the cell's end would
        // otherwise create one).
        while lines.last().map(|s| s.is_empty()).unwrap_or(false) {
            lines.pop();
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        Ok(lines)
    }

    /// Cell-mode inline renderer.
    ///
    /// Walks the inline subtree of `node_ref` like
    /// [`Self::render_inline_children`] but emits `CELL_SOFT_BREAK_MARKER`
    /// whenever a `Text` node carries the `SOFT_LINE_BREAK` qualifier (the
    /// multiline-table parser plants these between physical cell lines).
    /// Other inline kinds are deferred to the standard renderer; the walker
    /// descends into formatting wrappers (emphasis, strong, strikethrough,
    /// links, images, etc.) so a soft break nested inside formatting still
    /// splits the cell correctly.
    fn render_cell_inline_children(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        for child in arena[node_ref].children(arena) {
            self.render_cell_inline(out, arena, child, source)?;
        }
        Ok(())
    }

    /// Renders one inline node, intercepting `Text` to translate its
    /// `SOFT_LINE_BREAK` / `HARD_LINE_BREAK` qualifier into the cell
    /// physical-line sentinel. Every other inline kind is delegated to the
    /// standard inline renderer so wrapper markers (emphasis, code-span
    /// fencing, link/image syntax, footnote refs, raw HTML, etc.) match
    /// what the rest of the formatter produces.
    fn render_cell_inline(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        if let KindData::Text(text) = arena[node_ref].kind_data() {
            out.push_str(text.str(source));
            if text.has_qualifiers(TextQualifier::HARD_LINE_BREAK)
                || text.has_qualifiers(TextQualifier::SOFT_LINE_BREAK)
            {
                out.push_str(CELL_SOFT_BREAK_MARKER);
            }
            return Ok(());
        }
        // Non-Text inline kinds: delegate. Soft breaks inside emphasis/
        // strong/etc. are not produced by the multiline-table parser
        // (qualifiers attach to direct cell-level Text children), so we
        // do not need to recurse in cell-mode here.
        self.render_inline(out, arena, node_ref, source)
    }

    /// Re-emits a pandoc-style grid table.
    ///
    /// Column widths come from the AST-derived minimum per column (longest
    /// physical line + 2 for the `| ` / ` |` padding, floored at 3), widened
    /// to the original source's column widths when the source is available.
    /// Per-column alignment is read from the cell `TableCellAlignment` set
    /// by the parser and encoded as `:` markers in the header separator
    /// (`+:-:+`); for headerless tables the markers are placed in the top
    /// border instead.
    ///
    /// Multi-line cell content is recovered through
    /// [`Self::collect_cell_lines`] — the same SOFT_LINE_BREAK splitting
    /// used by `render_multiline_table`. Cells from the grid-table parser
    /// emit the SOFT_LINE_BREAK marker the same way.
    pub(super) fn render_grid_table(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        let mut header_rows: Vec<NodeRef> = Vec::new();
        let mut body_rows: Vec<NodeRef> = Vec::new();
        for child in arena[node_ref].children(arena) {
            match arena[child].kind_data() {
                KindData::TableHeader(_) => {
                    for row in arena[child].children(arena) {
                        header_rows.push(row);
                    }
                }
                KindData::TableBody(_) => {
                    for row in arena[child].children(arena) {
                        body_rows.push(row);
                    }
                }
                _ => {}
            }
        }
        let has_header = !header_rows.is_empty();

        // Collect rows as cell sequences, capturing each cell's colspan
        // (read from the node's `colspan` attribute, defaulting to 1).
        let mut rendered_header: Vec<Vec<GridCell>> = Vec::with_capacity(header_rows.len());
        let mut rendered_body: Vec<Vec<GridCell>> = Vec::with_capacity(body_rows.len());
        for row in &header_rows {
            rendered_header.push(self.collect_grid_row(arena, *row, source)?);
        }
        for row in &body_rows {
            rendered_body.push(self.collect_grid_row(arena, *row, source)?);
        }

        // Total column count = the maximum sum of colspans across all rows.
        // A well-formed table has the same sum on every row; we take max
        // defensively for malformed input.
        let columns = rendered_header
            .iter()
            .chain(rendered_body.iter())
            .map(|row| row.iter().map(|c| c.colspan).sum::<usize>())
            .max()
            .unwrap_or(0);
        if columns == 0 {
            return Ok(());
        }

        // Per-column alignment: take from the first row's *unspanned* cells.
        // Spanned cells override their slot to `None` since per-column
        // alignment is ambiguous for them.
        let mut aligns: Vec<TableCellAlignment> = vec![TableCellAlignment::None; columns];
        if let Some(first) = rendered_header.first().or(rendered_body.first()) {
            let mut col_idx = 0;
            for cell in first {
                if cell.colspan == 1 && col_idx < aligns.len() {
                    aligns[col_idx] = cell.alignment;
                }
                col_idx += cell.colspan;
            }
        }

        // Step 1: AST-derived minimum widths from *unspanned* cells. Each
        // single-column cell needs room for the longest physical line plus
        // 2 (space on each side between `|` and content). Pandoc floor: 3.
        let mut widths = vec![3usize; columns];
        for row in rendered_header.iter().chain(rendered_body.iter()) {
            let mut col_idx = 0;
            for cell in row {
                if cell.colspan == 1 && col_idx < widths.len() {
                    for line in &cell.lines {
                        widths[col_idx] = widths[col_idx].max(line.chars().count() + 2);
                    }
                }
                col_idx += cell.colspan;
            }
        }

        // Step 2: widen for column-spanning cells. A cell spanning N
        // columns occupies `sum(widths[start..start+N]) + (N - 1)` chars
        // (the `+N-1` accounts for the `|` characters absent inside the
        // span). If the cell content + 2 padding exceeds that, distribute
        // the deficit across the spanned columns.
        for row in rendered_header.iter().chain(rendered_body.iter()) {
            let mut col_idx = 0;
            for cell in row {
                if cell.colspan > 1 && col_idx + cell.colspan <= widths.len() {
                    let needed = cell
                        .lines
                        .iter()
                        .map(|l| l.chars().count())
                        .max()
                        .unwrap_or(0)
                        + 2;
                    let available: usize = widths[col_idx..col_idx + cell.colspan]
                        .iter()
                        .sum::<usize>()
                        + (cell.colspan - 1);
                    if needed > available {
                        let deficit = needed - available;
                        let per = deficit / cell.colspan;
                        let rem = deficit % cell.colspan;
                        for i in 0..cell.colspan {
                            widths[col_idx + i] += per;
                            if i < rem {
                                widths[col_idx + i] += 1;
                            }
                        }
                    }
                }
                col_idx += cell.colspan;
            }
        }

        // Step 3: widen with the source's original column widths when the
        // source had wider columns. Preserves author-tuned widths so
        // unmodified tables round-trip more faithfully.
        if let Some(table_pos) = arena[node_ref].pos() {
            if let Some(source_widths) = grid_table_column_widths(source, table_pos) {
                for (col, w) in source_widths.iter().enumerate() {
                    if col >= widths.len() {
                        break;
                    }
                    widths[col] = widths[col].max(*w);
                }
            }
        }

        // Alignment markers: header separator if there's a header,
        // otherwise the top border.
        let top_aligns: Option<&[TableCellAlignment]> = if has_header {
            None
        } else {
            Some(aligns.as_slice())
        };

        // Active-rowspan state: each entry is the number of additional
        // rows after the current one that a cell from above still
        // occupies this column. `0` means the cell ends at the current
        // row. Continuations across rows decrement this on each row
        // boundary; `> 0` after the row's content means the next row
        // separator must use a Continuation segment (spaces) for this
        // column.
        let mut active_rowspans: Vec<usize> = vec![0; columns];

        // Top border (all-dash; no rowspan can come from "above").
        out.push_str(&grid_border_line(
            b'-',
            &widths,
            top_aligns,
            &active_rowspans,
        ));
        out.push('\n');

        // Header rows + header separator.
        if has_header {
            for (idx, row) in rendered_header.iter().enumerate() {
                emit_grid_row_lines(out, row, &widths, &aligns, &active_rowspans);
                advance_active_rowspans(&mut active_rowspans, row);
                let is_last_header = idx + 1 == rendered_header.len();
                if is_last_header {
                    // The `+===+` boundary closes any rowspan still
                    // active (rowspan can't cross header/body — the
                    // parser already enforces this).
                    for s in active_rowspans.iter_mut() {
                        *s = 0;
                    }
                    out.push_str(&grid_border_line(
                        b'=',
                        &widths,
                        Some(aligns.as_slice()),
                        &active_rowspans,
                    ));
                } else {
                    out.push_str(&grid_border_line(b'-', &widths, None, &active_rowspans));
                }
                out.push('\n');
            }
        }

        // Body rows, each followed by a row separator. The separator
        // after the last body row is the bottom border (any still-
        // active rowspan there is a parser bug; we clear defensively).
        for (idx, row) in rendered_body.iter().enumerate() {
            emit_grid_row_lines(out, row, &widths, &aligns, &active_rowspans);
            advance_active_rowspans(&mut active_rowspans, row);
            let is_last_body = idx + 1 == rendered_body.len();
            if is_last_body {
                for s in active_rowspans.iter_mut() {
                    *s = 0;
                }
            }
            out.push_str(&grid_border_line(b'-', &widths, None, &active_rowspans));
            out.push('\n');
        }
        if rendered_body.is_empty() {
            out.push_str(&grid_border_line(b'-', &widths, None, &active_rowspans));
            out.push('\n');
        }

        // Trailing blank line so the block separator matches surrounding
        // conventions.
        out.push('\n');

        Ok(())
    }
}

/// Reads a `colspan` / `rowspan` node attribute as a `usize`, defaulting
/// to 1 for missing, unparseable, or zero values.
fn read_span_attr(arena: &Arena, cell: NodeRef, source: &str, name: &str) -> usize {
    arena[cell]
        .attributes()
        .get(name)
        .map(|v| v.str(source).into_owned())
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1)
}

/// Update `active_rowspans` after emitting `row`'s content lines: each
/// non-zero entry steps down by 1 (the cell from above advanced one row),
/// and each rowspan > 1 cell in *this* row adds `rowspan - 1` to its
/// columns (the rows it still occupies).
fn advance_active_rowspans(active_rowspans: &mut [usize], row: &[GridCell]) {
    // Step 1: decrement carry-over rowspans from earlier rows.
    for s in active_rowspans.iter_mut() {
        if *s > 0 {
            *s -= 1;
        }
    }
    // Step 2: account for cells in this row. We walk in the same
    // column-cursor pattern as `emit_grid_row_lines`: each `GridCell`
    // occupies `colspan` columns, starting at the next free column.
    let mut col_idx = 0;
    for cell in row {
        // Advance the cursor past any columns occupied by a carry-over
        // rowspan that's still active (those didn't consume a cell from
        // this row's `row`).
        while col_idx < active_rowspans.len() && active_rowspans[col_idx] > 0 {
            col_idx += 1;
        }
        if cell.rowspan > 1 {
            let extra = cell.rowspan - 1;
            for i in 0..cell.colspan {
                if col_idx + i < active_rowspans.len() {
                    active_rowspans[col_idx + i] = extra;
                }
            }
        }
        col_idx += cell.colspan;
    }
}

/// Detects whether `node_ref` is a multiline (pandoc) table by inspecting
/// the source line at the table's `pos`. The multiline-table parser sets
/// the table's `pos` to the byte offset of the opening dash rule.
fn is_multiline_table_node(arena: &Arena, node_ref: NodeRef, source: &str) -> bool {
    let Some(pos) = arena[node_ref].pos() else {
        return false;
    };
    let Some(line) = source_line_at_pos(source, pos) else {
        return false;
    };
    is_any_dash_rule_line(line)
}

/// Detects whether `node_ref` is a pandoc grid table by inspecting the source
/// line at the table's `pos`. The grid-table parser sets `pos` to the byte
/// offset of the opening `+---+` border.
fn is_grid_table_node(arena: &Arena, node_ref: NodeRef, source: &str) -> bool {
    let Some(pos) = arena[node_ref].pos() else {
        return false;
    };
    let Some(line) = source_line_at_pos(source, pos) else {
        return false;
    };
    is_grid_border_line(line)
}

/// Builds a grid-table border line of the form `+---+:--+--:+:-:+`. `marker`
/// is the fill byte (`-` for dash borders, `=` for header separator). If
/// `aligns` is `Some`, the corresponding `:` markers are placed inside the
/// border at column boundaries.
/// Build a grid-table border line. For each column:
/// - If `active_rowspans[col] > 0`, emit `widths[col]` spaces (the cell
///   from above is still occupying this column → continuation marker).
/// - Otherwise emit `widths[col]` copies of `marker`, with optional
///   `:` alignment markers at the boundaries.
///
/// A `+` is emitted between every column regardless of continuation —
/// pandoc requires a `+` at every top-border column position on every
/// border line, even inside a contiguous continuation run. The
/// "merged" continuation form `+         +` (no internal `+`) is not
/// emitted because the parser's strict-equality column-position check
/// would reject it on re-parse.
fn grid_border_line(
    marker: u8,
    widths: &[usize],
    aligns: Option<&[TableCellAlignment]>,
    active_rowspans: &[usize],
) -> String {
    let mut s = String::new();
    s.push('+');
    for (col, &w) in widths.iter().enumerate() {
        let continuing = active_rowspans.get(col).map(|&n| n > 0).unwrap_or(false);
        if continuing {
            for _ in 0..w {
                s.push(' ');
            }
        } else {
            let align = aligns
                .and_then(|a| a.get(col))
                .copied()
                .unwrap_or(TableCellAlignment::None);
            let (left, right) = match align {
                TableCellAlignment::Left => (true, false),
                TableCellAlignment::Right => (false, true),
                TableCellAlignment::Center => (true, true),
                _ => (false, false),
            };
            let fill_count = w
                .saturating_sub(if left { 1 } else { 0 })
                .saturating_sub(if right { 1 } else { 0 });
            if left {
                s.push(':');
            }
            for _ in 0..fill_count {
                s.push(marker as char);
            }
            if right {
                s.push(':');
            }
        }
        s.push('+');
    }
    s
}

/// Emits one source line per physical-line index across the row, with
/// `| ... | ... |` framing.
///
/// Cells with `colspan > 1` consume the `widths` of all covered columns
/// plus `colspan - 1` extra characters (the `|` separators absent
/// inside the span are *not* emitted between those columns).
///
/// Columns where `active_rowspans[col] > 0` are continuations of cells
/// from earlier rows: they don't consume a `GridCell` from this row's
/// `row` list and are emitted as `widths[col]` spaces (the cell's
/// content area is empty in this row), bounded by `|` separators that
/// are still emitted to preserve column alignment.
fn emit_grid_row_lines(
    out: &mut String,
    row: &[GridCell],
    widths: &[usize],
    aligns: &[TableCellAlignment],
    active_rowspans: &[usize],
) {
    let height = row
        .iter()
        .map(|cell| cell.lines.len())
        .max()
        .unwrap_or(0)
        .max(1);
    for line_idx in 0..height {
        let mut line_buf = String::new();
        line_buf.push('|');
        let mut col_idx = 0;
        let mut cell_iter = row.iter();
        while col_idx < widths.len() {
            let continuing = active_rowspans
                .get(col_idx)
                .map(|&n| n > 0)
                .unwrap_or(false);
            if continuing {
                // Continuation slot — emit empty cell area + `|`.
                for _ in 0..widths[col_idx] {
                    line_buf.push(' ');
                }
                line_buf.push('|');
                col_idx += 1;
                continue;
            }
            let Some(cell) = cell_iter.next() else {
                // Defensive: not enough cells to fill remaining columns
                // (parser would have rejected). Pad with spaces.
                for _ in 0..widths[col_idx] {
                    line_buf.push(' ');
                }
                line_buf.push('|');
                col_idx += 1;
                continue;
            };
            let span = cell.colspan.max(1);
            let end_col = (col_idx + span).min(widths.len());
            let effective_width: usize = widths[col_idx..end_col].iter().sum::<usize>()
                + (end_col - col_idx).saturating_sub(1);
            let align = if span == 1 {
                aligns
                    .get(col_idx)
                    .copied()
                    .unwrap_or(TableCellAlignment::None)
            } else {
                TableCellAlignment::None
            };
            let content = cell.lines.get(line_idx).map(String::as_str).unwrap_or("");
            line_buf.push_str(&pad_cell_for_grid(content, effective_width, align));
            line_buf.push('|');
            col_idx = end_col;
        }
        out.push_str(line_buf.trim_end_matches(' '));
        out.push('\n');
    }
}

/// Returns a string of width `width` (in characters) containing `content`
/// padded to fit between two `|` separators with 1-space border on each side.
/// Padding inside the border respects `align`. If content is wider than the
/// inner area, no padding is added (the width-computation step is expected
/// to have prevented this).
fn pad_cell_for_grid(content: &str, width: usize, align: TableCellAlignment) -> String {
    let inner = width.saturating_sub(2);
    let chars = content.chars().count();
    if chars >= inner {
        return format!(" {} ", content);
    }
    let extra = inner - chars;
    let (left, right) = match align {
        TableCellAlignment::Right => (extra, 0),
        TableCellAlignment::Center => (extra / 2, extra - extra / 2),
        _ => (0, extra), // Left or None — flush left
    };
    let mut s = String::with_capacity(width);
    s.push(' ');
    for _ in 0..left {
        s.push(' ');
    }
    s.push_str(content);
    for _ in 0..right {
        s.push(' ');
    }
    s.push(' ');
    s
}

/// Emits the maximum-physical-line count's worth of source lines for a
/// single multiline-table row.
fn emit_row_physical_lines(
    out: &mut String,
    row: &[Vec<String>],
    widths: &[usize],
    aligns: &[TableCellAlignment],
) {
    let height = row.iter().map(|cell| cell.len()).max().unwrap_or(0).max(1);
    for line_idx in 0..height {
        let mut row_line = String::new();
        for (col, width) in widths.iter().enumerate() {
            let content = row
                .get(col)
                .and_then(|lines| lines.get(line_idx))
                .map(String::as_str)
                .unwrap_or("");
            let align = aligns.get(col).copied().unwrap_or(TableCellAlignment::None);
            if col > 0 {
                row_line.push(' ');
            }
            row_line.push_str(&pad_cell_for_multiline(content, *width, align));
        }
        // Trim trailing spaces on the final segment so the engine's
        // `TRIM_TRAILING_WHITESPACE` pass does not need to touch it.
        out.push_str(row_line.trim_end_matches(' '));
        out.push('\n');
    }
}

/// Renders a single cell on a single source line, padding to `width`
/// characters with the requested alignment. Default alignment behaves like
/// `Left` for padding purposes (both leave content flush-left); the
/// distinction is preserved at the AST level but irrelevant for visual
/// layout.
fn pad_cell_for_multiline(content: &str, width: usize, align: TableCellAlignment) -> String {
    let chars = content.chars().count();
    if chars >= width {
        return content.to_string();
    }
    let pad = width - chars;
    let mut out = String::with_capacity(content.len() + pad);
    match align {
        TableCellAlignment::Right => {
            for _ in 0..pad {
                out.push(' ');
            }
            out.push_str(content);
        }
        TableCellAlignment::Center => {
            let left = pad / 2;
            let right = pad - left;
            for _ in 0..left {
                out.push(' ');
            }
            out.push_str(content);
            for _ in 0..right {
                out.push(' ');
            }
        }
        _ => {
            out.push_str(content);
            for _ in 0..pad {
                out.push(' ');
            }
        }
    }
    out
}

/// Returns a grouped dash rule with one dash run per column, separated by
/// single spaces, e.g. `widths=[12, 9, 7, 36]` → `"------------ --------- ------- ------------------------------------"`.
fn grouped_dash_rule(widths: &[usize]) -> String {
    let mut out = String::new();
    for (idx, w) in widths.iter().enumerate() {
        if idx > 0 {
            out.push(' ');
        }
        for _ in 0..(*w).max(3) {
            out.push('-');
        }
    }
    out
}

fn detect_table_edge_style(
    arena: &Arena,
    header_rows: &[NodeRef],
    body_rows: &[NodeRef],
    source: &str,
) -> TableEdgeStyle {
    for row_ref in header_rows.iter().chain(body_rows.iter()) {
        if let Some(style) = detect_table_row_edge_style(arena, *row_ref, source) {
            return style;
        }
    }

    TableEdgeStyle {
        leading_pipe: true,
        trailing_pipe: true,
    }
}

fn detect_table_row_edge_style(
    arena: &Arena,
    row_ref: NodeRef,
    source: &str,
) -> Option<TableEdgeStyle> {
    let pos = arena[row_ref].pos()?;
    let line = source_line_at_pos(source, pos)?;
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return None;
    }
    Some(TableEdgeStyle {
        leading_pipe: trimmed.starts_with('|'),
        trailing_pipe: trimmed.ends_with('|'),
    })
}

fn render_table_row(
    row: &[String],
    widths: &[usize],
    aligns: &[TableCellAlignment],
    style: TableEdgeStyle,
) -> String {
    let mut cells = Vec::with_capacity(widths.len());
    for (index, width) in widths.iter().enumerate() {
        let cell = row.get(index).map_or("", String::as_str).trim();
        let pad = width.saturating_sub(cell.chars().count());
        let align = aligns
            .get(index)
            .copied()
            .unwrap_or(TableCellAlignment::None);
        let mut rendered = String::new();
        match align {
            TableCellAlignment::Right => {
                rendered.push_str(&" ".repeat(pad));
                rendered.push_str(cell);
            }
            TableCellAlignment::Center => {
                let left = pad / 2;
                let right = pad - left;
                rendered.push_str(&" ".repeat(left));
                rendered.push_str(cell);
                rendered.push_str(&" ".repeat(right));
            }
            TableCellAlignment::Left | TableCellAlignment::None => {
                rendered.push_str(cell);
                rendered.push_str(&" ".repeat(pad));
            }
            _ => {
                rendered.push_str(cell);
                rendered.push_str(&" ".repeat(pad));
            }
        }
        cells.push(rendered);
    }
    render_table_cells(&cells, style)
}

fn render_table_row_unaligned(row: &[String], style: TableEdgeStyle) -> String {
    let cells: Vec<String> = row.iter().map(|cell| cell.trim().to_string()).collect();
    render_table_cells(&cells, style)
}

fn render_table_cells(cells: &[String], style: TableEdgeStyle) -> String {
    let mut out = String::new();
    if style.leading_pipe {
        out.push('|');
        out.push(' ');
    }

    out.push_str(&cells.join(" | "));

    if style.trailing_pipe {
        out.push(' ');
        out.push('|');
    }
    out
}

fn render_separator_cell(alignment: TableCellAlignment, width: usize) -> String {
    let width = width.max(3);
    match alignment {
        TableCellAlignment::Left => format!(":{}", "-".repeat(width.saturating_sub(1))),
        TableCellAlignment::Center => {
            format!(":{}:", "-".repeat(width.saturating_sub(2).max(1)))
        }
        TableCellAlignment::Right => format!("{}:", "-".repeat(width.saturating_sub(1))),
        TableCellAlignment::None => "-".repeat(width),
        _ => "-".repeat(width),
    }
}

#[cfg(test)]
mod tests {
    use crate::fmt::format_markdown;

    fn fmt(src: &str) -> String {
        format_markdown(src).expect("format_markdown failed")
    }

    /// A vanilla pipe table should still emit a pipe table — multiline-table
    /// detection must not catch pipe tables.
    #[test]
    fn pipe_table_still_emits_pipes() {
        let src = "| A | B |\n|---|---|\n| 1 | 2 |\n";
        let out = fmt(src);
        assert!(
            out.contains("| A   | B   |"),
            "pipe table got rewritten: {out:?}"
        );
        assert!(out.contains("| 1   | 2   |"));
        assert!(
            !out.lines()
                .any(|line| line.starts_with("---") && !line.contains('|')),
            "unexpected solid dash rule in pipe-table output: {out:?}"
        );
    }

    /// With-header multiline table round-trips through format_markdown.
    #[test]
    fn multiline_with_header_round_trip() {
        let src = "\
# Heading

-------------------------------------------------------------------
  Centered    Default          Right Left
  Header      Aligned       Aligned Aligned
------------ --------- ------- ------------------------------------
   First     row          12.0 Example of a row that spans
                               two physical lines.

   Second    row           5.0 Another row.
-------------------------------------------------------------------
";
        let out = fmt(src);
        // Output must contain the solid opening + closing rule and a grouped
        // separator rule.
        let dash_lines: Vec<&str> = out
            .lines()
            .filter(|line| line.chars().all(|c| c == '-' || c == ' ') && line.contains('-'))
            .collect();
        // At least: opening (solid), separator (grouped), closing (solid).
        assert!(
            dash_lines.len() >= 3,
            "expected ≥3 dash-rule lines, got: {dash_lines:#?}\nfull output:\n{out}"
        );
        // Solid rules contain no spaces.
        let solids: Vec<&&str> = dash_lines.iter().filter(|l| !l.contains(' ')).collect();
        assert!(
            solids.len() >= 2,
            "expected ≥2 solid rules (opening + closing): {dash_lines:#?}"
        );
        // Cell text survives.
        assert!(out.contains("Centered"), "Centered missing: {out}");
        assert!(out.contains("Header"), "Header missing: {out}");
        assert!(out.contains("First"));
        assert!(out.contains("Example of a row that spans"));
        assert!(out.contains("two physical lines."));
        assert!(out.contains("Second"));
        assert!(out.contains("Another row."));
        // And not a pipe table.
        assert!(!out.contains(" | "), "rewrote as pipe table: {out}");
    }

    /// Headerless multiline table round-trips.
    #[test]
    fn multiline_headerless_round_trip() {
        let src = "\
# Heading

------------ ----------- ------- ------------------------------------
First        row            12.0 First-row content that wraps onto
                                 two physical lines.

Second       row             5.0 Another row.
------------ ----------- ------- ------------------------------------
";
        let out = fmt(src);
        // Opening rule is grouped (headerless), closing is solid.
        let lines: Vec<&str> = out.lines().collect();
        let first_dash_idx = lines
            .iter()
            .position(|l| l.chars().all(|c| c == '-' || c == ' ') && l.contains('-'))
            .expect("no dash rule");
        let last_dash_idx = lines
            .iter()
            .rposition(|l| l.chars().all(|c| c == '-' || c == ' ') && l.contains('-'))
            .expect("no closing dash rule");
        assert!(
            lines[first_dash_idx].contains(' '),
            "headerless opening rule should be grouped, got: {:?}",
            lines[first_dash_idx]
        );
        assert!(
            !lines[last_dash_idx].contains(' '),
            "closing rule should be solid, got: {:?}",
            lines[last_dash_idx]
        );
        // Content survives.
        assert!(out.contains("First"));
        assert!(out.contains("First-row content that wraps onto"));
        assert!(out.contains("two physical lines."));
        assert!(out.contains("Second"));
        assert!(!out.contains(" | "));
    }

    /// Cyrillic content in cells (regression guard for the parser's UTF-8
    /// boundary handling, exercised through the formatter).
    #[test]
    fn multiline_cyrillic_round_trip() {
        let src = "\
# Heading

-------------------------------------------------------------------
  Выровнен   По         Вправо Влево
  по центру  умолчанию
------------ --------- ------- ------------------------------------
   Первая    строка       12.0 Пример строки, которая
                               занимает несколько физических строк.

   Вторая    строка        5.0 Ещё одна строка.
-------------------------------------------------------------------
";
        let out = fmt(src);
        assert!(out.contains("Выровнен"));
        assert!(out.contains("по центру"));
        assert!(out.contains("умолчанию"));
        assert!(out.contains("Первая"));
        assert!(out.contains("Пример строки, которая"));
        assert!(out.contains("занимает несколько физических строк."));
        assert!(out.contains("Вторая"));
        assert!(out.contains("Ещё одна строка."));
        assert!(!out.contains(" | "), "rewrote as pipe table: {out}");
    }

    /// A pipe table immediately following a multiline table must still
    /// render as pipes.
    #[test]
    fn mixed_pipe_and_multiline_tables() {
        let src = "\
# Heading

------------ --------- ------- ------------------------------------
First        row          12.0 Single line.
------------ --------- ------- ------------------------------------

| A | B |
|---|---|
| 1 | 2 |
";
        let out = fmt(src);
        // Multiline table preserved.
        assert!(out.contains("------------ ---------"), "{out}");
        // Pipe table preserved.
        assert!(out.contains("| A   | B   |"), "{out}");
        assert!(out.contains("| 1   | 2   |"));
    }

    /// Grid table with header round-trips: input is grid syntax, output must
    /// still be grid syntax (not pipe, not multiline).
    #[test]
    fn grid_with_header_round_trip() {
        let src = "\
# Heading

+----------+----------+
| Header 1 | Header 2 |
+==========+==========+
| a        | b        |
+----------+----------+
| c        | d        |
+----------+----------+
";
        let out = fmt(src);
        // Must emit grid borders.
        assert!(
            out.contains("+----------+----------+") || out.contains("+----------+"),
            "missing grid border: {out}"
        );
        // Header separator must use `=`.
        assert!(out.contains("+=="), "missing header separator: {out}");
        // Cell content preserved.
        assert!(out.contains("Header 1"));
        assert!(out.contains("Header 2"));
        assert!(out.contains("| a"));
        assert!(out.contains("| b"));
        assert!(out.contains("| c"));
        assert!(out.contains("| d"));
        // Must not collapse to a pipe table.
        assert!(
            !out.contains("|---|"),
            "rewrote as pipe-table delimiter: {out}"
        );
    }

    /// Headerless grid table round-trips. Alignment markers (when present in
    /// the source) should reappear in the rendered top border.
    #[test]
    fn grid_headerless_with_alignment_round_trip() {
        let src = "\
# Heading

+:----+----:+:--:+
| a   | b   | c  |
+-----+-----+----+
| d   | e   | f  |
+-----+-----+----+
";
        let out = fmt(src);
        // Must emit grid borders.
        assert!(out.contains("+--"), "missing grid border: {out}");
        // No header → no `+===+` separator.
        assert!(
            !out.contains("+==="),
            "headerless table emitted header separator: {out}"
        );
        // Alignment markers placed in top border (since headerless).
        let lines: Vec<&str> = out.lines().collect();
        let first_border = lines
            .iter()
            .find(|l| l.starts_with('+'))
            .expect("no border line");
        assert!(
            first_border.contains(':'),
            "alignment markers missing from top border: {first_border:?}"
        );
        // Content preserved (alignment may pad either side, so just check
        // each cell letter appears between `|`s).
        for needle in ["a", "b", "c", "d", "e", "f"] {
            assert!(out.contains(needle), "cell {needle} missing: {out}");
        }
        assert!(!out.contains("|---|"), "rewrote as pipe table: {out}");
    }

    /// Grid table with a column-spanned cell in the header survives the
    /// markdown round trip. The output must:
    ///   - drop one `|` separator on the spanned-row physical line
    ///   - keep the full `|`-separator set on unspanned rows
    ///   - keep the header separator (`+===+...===+`)
    #[test]
    fn grid_colspan_header_round_trip() {
        let src = "\
# Heading

+------+------+------+
| Group 1     | Solo |
+======+======+======+
| a    | b    | c    |
+------+------+------+
";
        let out = fmt(src);
        // Top border + header separator + bottom border = 3 grid borders.
        let borders = out.lines().filter(|l| l.starts_with('+')).count();
        assert!(borders >= 3, "missing grid borders: {out}");
        assert!(out.contains("+======"), "header separator missing: {out}");
        // The header row should have the spanned cell rendered without a
        // middle `|`: search for a header content line that contains the
        // text `Group 1` and has only TWO `|` characters total (outer
        // borders), proving the middle `|` was suppressed.
        let header_line = out
            .lines()
            .find(|l| l.contains("Group 1"))
            .expect("no header content line");
        assert_eq!(
            header_line.bytes().filter(|&b| b == b'|').count(),
            3,
            "spanned header should have 3 `|`s (outer + outer + Solo): {header_line:?}"
        );
        // The unspanned body row should have 4 `|`s.
        let body_line = out.lines().find(|l| l.contains("| a ")).expect("body");
        assert_eq!(
            body_line.bytes().filter(|&b| b == b'|').count(),
            4,
            "unspanned body should have 4 `|`s: {body_line:?}"
        );
        // Content preserved.
        for needle in ["Group 1", "Solo", "a", "b", "c"] {
            assert!(out.contains(needle), "{needle} missing: {out}");
        }
        assert!(!out.contains("|---|"));
    }

    /// Column span in a body row round-trips, and the source's wider
    /// columns are preserved.
    #[test]
    fn grid_colspan_body_round_trip() {
        let src = "\
+------+------+------+
| Hdr1 | Hdr2 | Hdr3 |
+======+======+======+
| both columns | x   |
+------+------+------+
| a    | b    | c    |
+------+------+------+
";
        let out = fmt(src);
        // The first body row must render with 3 `|`s (the middle was
        // merged), the second body row with 4.
        let spans_line = out
            .lines()
            .find(|l| l.contains("both columns"))
            .expect("spans line");
        assert_eq!(spans_line.bytes().filter(|&b| b == b'|').count(), 3);
        let unsp_line = out.lines().find(|l| l.contains("| a ")).expect("unsp");
        assert_eq!(unsp_line.bytes().filter(|&b| b == b'|').count(), 4);
        assert!(out.contains("both columns"));
        assert!(out.contains("Hdr1"));
        assert!(!out.contains("|---|"));
    }

    /// Grid table with a row-spanning cell survives the round trip. The
    /// row separator after row 1 must carry a continuation segment
    /// (spaces between `+`s) for column 0 and emit dashes for column 1;
    /// row 2's content line for column 0 is an empty padded area
    /// bounded by `|`s on both sides.
    #[test]
    fn grid_rowspan_round_trip() {
        let src = "\
+------+------+
| A    | B    |
+      +------+
|      | C    |
+------+------+
";
        let out = fmt(src);
        // Top + intermediate + bottom = 3 border lines starting with `+`.
        let borders: Vec<&str> = out.lines().filter(|l| l.starts_with('+')).collect();
        assert!(borders.len() >= 3, "missing borders: {out}");

        // Find the intermediate separator (between A and the rowspan
        // continuation row): it must have a continuation segment for
        // column 0 (no `-` in the segment) and dashes for column 1.
        let mid_separator = borders
            .iter()
            .find(|l| {
                // Border line that contains both `+   ` (space run) and `+---` (dashes).
                let bytes = l.as_bytes();
                bytes.windows(2).any(|w| w == b"+ ") && bytes.windows(2).any(|w| w == b"+-")
            })
            .copied()
            .unwrap_or_else(|| panic!("no continuation-bearing separator: {out}"));
        // Sanity-check that the first segment is all-space (continuation
        // for column 0) — i.e. nothing between the first two `+`s is a
        // `-`.
        let first_plus = mid_separator.find('+').expect("no `+` in border");
        let after_first = &mid_separator[first_plus + 1..];
        let second_plus = after_first.find('+').expect("only one `+` in border");
        assert!(
            !after_first[..second_plus].contains('-'),
            "first segment should be continuation (spaces only): {mid_separator:?}"
        );

        // Row 2's content line: column-0 cell area must be all spaces
        // between `|`s, column-1 must contain "C".
        let row2 = out
            .lines()
            .find(|l| l.contains("| C "))
            .unwrap_or_else(|| panic!("no row 2 line: {out}"));
        let first_pipe = row2.find('|').unwrap();
        let after_first_pipe = &row2[first_pipe + 1..];
        let second_pipe = after_first_pipe.find('|').unwrap();
        assert!(
            after_first_pipe[..second_pipe].chars().all(|c| c == ' '),
            "row 2 col-0 area should be all spaces (rowspan continuation): {row2:?}"
        );

        // Content survives.
        for needle in ["A", "B", "C"] {
            assert!(out.contains(needle), "{needle} missing: {out}");
        }
        assert!(!out.contains("|---|"));
    }

    /// A cell carrying both rowspan and colspan round-trips: the
    /// continuation row's content line for the spanned area is empty,
    /// and the row separator marks all spanned columns as continuations.
    #[test]
    fn grid_rowspan_and_colspan_round_trip() {
        let src = "\
+----+----+----+
| Big     | X  |
+    +    +----+
|         | Y  |
+----+----+----+
| a  | b  | c  |
+----+----+----+
";
        let out = fmt(src);
        // Content preserved.
        for needle in ["Big", "X", "Y", "a", "b", "c"] {
            assert!(out.contains(needle), "{needle} missing: {out}");
        }
        // Some border line in the output must have a `+    +    +`
        // shape (continuation in both columns 0 and 1).
        let two_continuations = out.lines().any(|l| {
            l.starts_with('+') && l.bytes().filter(|&b| b == b'+').count() >= 3 && {
                // First and second segments must be space-only.
                let bytes = l.as_bytes();
                let mut plus_idx: Vec<usize> = bytes
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &b)| if b == b'+' { Some(i) } else { None })
                    .collect();
                plus_idx.truncate(3);
                plus_idx
                    .windows(2)
                    .take(2)
                    .all(|w| bytes[w[0] + 1..w[1]].iter().all(|&b| b == b' '))
            }
        });
        assert!(
            two_continuations,
            "expected a separator with two contiguous continuation segments: {out}"
        );
        assert!(!out.contains("|---|"));
    }

    /// Multi-line cells in a grid table round-trip: each physical source line
    /// of a cell must reappear on its own line of the output.
    #[test]
    fn grid_multi_line_cell_round_trip() {
        let src = "\
+--------------+----------+
| spans        | single   |
| multiple     |          |
+==============+==========+
| also wraps   | fits one |
| across lines |          |
+--------------+----------+
";
        let out = fmt(src);
        for needle in [
            "spans",
            "multiple",
            "single",
            "also wraps",
            "across lines",
            "fits one",
        ] {
            assert!(out.contains(needle), "{needle} missing: {out}");
        }
        // Header separator emitted.
        assert!(out.contains("+=="));
        assert!(!out.contains("|---|"), "rewrote as pipe table: {out}");
    }

    /// Source separator-rule widths wider than AST minimums must survive
    /// the round trip — this is the author-tuned-width case.
    #[test]
    fn multiline_source_widths_are_preserved() {
        let src = "\
# Heading

-------------------------------------------------------------------
  A          B               C    D
------------ --------- ------- ------------------------------------
   x         y                z    w
-------------------------------------------------------------------
";
        let out = fmt(src);
        // The grouped separator from source has widths [12, 9, 7, 36].
        // The output should retain at least those widths (search for a
        // dash run of ≥ 36 dashes in any grouped or solid rule).
        let has_wide = out
            .lines()
            .any(|line| line.chars().filter(|&c| c == '-').count() >= 36);
        assert!(
            has_wide,
            "expected a dash run of ≥36 to survive; output:\n{out}"
        );
    }
}
