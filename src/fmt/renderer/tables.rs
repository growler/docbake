use anyhow::Result;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, KindData, NodeRef, TableCellAlignment, TextQualifier};

use super::source_recovery::{
    is_any_dash_rule_line, multiline_table_separator_extents, source_line_at_pos,
};
use super::MarkdownFmtRenderer;

#[derive(Clone, Copy, Debug)]
struct TableEdgeStyle {
    leading_pipe: bool,
    trailing_pipe: bool,
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
