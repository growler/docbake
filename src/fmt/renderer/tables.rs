use anyhow::Result;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, KindData, NodeRef, TableCellAlignment};

use super::source_recovery::source_line_at_pos;
use super::MarkdownFmtRenderer;

#[derive(Clone, Copy, Debug)]
struct TableEdgeStyle {
    leading_pipe: bool,
    trailing_pipe: bool,
}

impl MarkdownFmtRenderer {
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
