use std::io::Write as _;

use anyhow::{bail, Context, Result};
use docx_rs::*;
use memchr::memmem::Finder;
use rushdown::as_kind_data;
use rushdown::ast::{KindData, NodeRef, TableCellAlignment};

use super::stylesheet::TABLE_STYLE;
use super::{DocxRenderer, RenderContext};
use crate::render_typst::{cell_colspan, cell_rowspan};
use crate::table_source::column_weights;

/// The table properties that docx-rs cannot write, for one table of the
/// document part.
pub(super) struct TableLayout {
    /// The number of header rows. These rows repeat on each page.
    header_rows: usize,
    /// The column widths in twips.
    grid: Vec<u32>,
}

/// A vertical merge that starts in a column.
#[derive(Clone, Copy)]
struct Merge {
    /// The index of the first row after the merge.
    end: usize,
    /// The number of columns of the merged cell.
    span: usize,
}

impl<'a> DocxRenderer<'a> {
    /// Render a table to a DOCX table. Records the table properties that
    /// docx-rs cannot write in `self.tables`.
    pub(super) fn render_table(
        &self,
        mut docx: Docx,
        context: &RenderContext<'_>,
        table_ref: NodeRef,
    ) -> Result<Docx> {
        let arena = context.arena;
        let source = context.source;
        let mut header_rows = Vec::new();
        let mut body_rows = Vec::new();

        for child in arena[table_ref].children(arena) {
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

        // The column count is the sum of the column spans in the first row.
        // No merge from a row above covers the first row.
        let column_count = match header_rows.first().or_else(|| body_rows.first()) {
            Some(&row) => table_cells(context, row)
                .map(|cell| cell_colspan(arena, source, cell))
                .sum::<usize>(),
            None => 0,
        };
        if column_count == 0 {
            return Ok(docx);
        }

        let weights = arena[table_ref]
            .pos()
            .and_then(|pos| column_weights(source, pos));
        let grid = grid_widths(self.params.page.text_width(), weights, column_count);

        let mut rows = Vec::with_capacity(header_rows.len() + body_rows.len());
        let mut merges = vec![Merge { end: 0, span: 1 }; column_count];
        for (row_index, &row_ref) in header_rows.iter().chain(&body_rows).enumerate() {
            let style = if row_index < header_rows.len() {
                "TableHeader"
            } else {
                "TableCell"
            };
            let mut cells = Vec::new();
            let mut col = 0;
            for cell_ref in table_cells(context, row_ref) {
                col = add_merged_cells(&mut cells, &merges, &grid, row_index, col, style);
                if col >= column_count {
                    break;
                }
                let span = cell_colspan(arena, source, cell_ref).min(column_count - col);
                let row_span = cell_rowspan(arena, source, cell_ref);
                let mut cell = TableCell::new()
                    .width(span_width(&grid, col, span), WidthType::Dxa)
                    .add_paragraph(self.render_cell_paragraph(context, cell_ref, style)?);
                if span > 1 {
                    cell = cell.grid_span(span);
                }
                if row_span > 1 {
                    cell = cell.vertical_merge(VMergeType::Restart);
                    merges[col] = Merge {
                        end: row_index + row_span,
                        span,
                    };
                }
                cells.push(cell);
                col += span;
            }
            add_merged_cells(&mut cells, &merges, &grid, row_index, col, style);
            rows.push(TableRow::new(cells));
        }

        docx = docx
            .add_table(Table::without_borders(rows))
            .add_paragraph(Paragraph::new());
        self.tables.borrow_mut().push(TableLayout {
            header_rows: header_rows.len(),
            grid,
        });

        Ok(docx)
    }

    /// Render the content of a table cell to one paragraph with the style
    /// `style` and the alignment of the Markdown column.
    fn render_cell_paragraph(
        &self,
        context: &RenderContext<'_>,
        cell_ref: NodeRef,
        style: &str,
    ) -> Result<Paragraph> {
        let arena = context.arena;
        let mut para = Paragraph::new().style(style);
        let alignment = match as_kind_data!(arena, cell_ref, TableCell).alignment() {
            TableCellAlignment::Left => Some(AlignmentType::Left),
            TableCellAlignment::Center => Some(AlignmentType::Center),
            TableCellAlignment::Right => Some(AlignmentType::Right),
            _ => None,
        };
        if let Some(alignment) = alignment {
            para = para.align(alignment);
        }
        for child in arena[cell_ref].children(arena) {
            for run in self.render_inline_runs(context, child)? {
                para = para.add_run(run);
            }
        }
        Ok(para)
    }
}

/// Gives the cells of the table row `row_ref`.
fn table_cells<'c>(
    context: &'c RenderContext<'_>,
    row_ref: NodeRef,
) -> impl Iterator<Item = NodeRef> + 'c {
    let arena = context.arena;
    arena[row_ref]
        .children(arena)
        .filter(move |&cell| matches!(arena[cell].kind_data(), KindData::TableCell(_)))
}

/// Adds a continuation cell to `cells` for each vertical merge that covers
/// the row `row` from the column `col`. Gives the next column that no merge
/// covers.
fn add_merged_cells(
    cells: &mut Vec<TableCell>,
    merges: &[Merge],
    grid: &[u32],
    row: usize,
    mut col: usize,
    style: &str,
) -> usize {
    while let Some(merge) = merges.get(col).filter(|merge| merge.end > row) {
        let mut cell = TableCell::new()
            .width(span_width(grid, col, merge.span), WidthType::Dxa)
            .vertical_merge(VMergeType::Continue)
            .add_paragraph(Paragraph::new().style(style));
        if merge.span > 1 {
            cell = cell.grid_span(merge.span);
        }
        cells.push(cell);
        col += merge.span;
    }
    col
}

/// Gives the width in twips of `span` columns from the column `col`.
fn span_width(grid: &[u32], col: usize, span: usize) -> usize {
    grid[col..col + span].iter().map(|&w| w as usize).sum()
}

/// Divides `text_width` into `column_count` column widths in the ratio of
/// `weights`. Divides it equally if `weights` is `None` or has a different
/// number of columns. The last column gets the remainder, thus the sum of
/// the widths is `text_width`.
fn grid_widths(text_width: u32, weights: Option<Vec<usize>>, column_count: usize) -> Vec<u32> {
    let weights = weights
        .filter(|weights| weights.len() == column_count)
        .unwrap_or_else(|| vec![1; column_count]);
    let total: u64 = weights.iter().map(|&w| w as u64).sum();
    let mut grid: Vec<u32> = weights
        .iter()
        .map(|&w| (u64::from(text_width) * w as u64 / total) as u32)
        .collect();
    let used: u32 = grid.iter().sum();
    if let Some(last) = grid.last_mut() {
        *last += text_width - used;
    }
    grid
}

/// The start of each table that docx-rs writes for
/// `Table::without_borders` with no style and no grid.
const DOCX_RS_TABLE_START: &str = r#"<w:tbl><w:tblPr><w:tblW w:w="0" w:type="auto" /><w:jc w:val="left" /><w:tblBorders /></w:tblPr><w:tblGrid />"#;

/// The start tag of a table.
const TABLE_START_TAG: &str = "<w:tbl>";

/// The end tag of a table.
const TABLE_END_TAG: &str = "</w:tbl>";

/// The properties that docx-rs writes for a row with no properties.
const ROW_PROPERTIES: &str = "<w:trPr />";

/// The properties of a header row.
const HEADER_ROW_PROPERTIES: &str = "<w:trPr><w:tblHeader /></w:trPr>";

/// The `w:tblLook` flag for the first-row conditional format.
const LOOK_FIRST_ROW: u16 = 0x0020;

/// The `w:tblLook` flag that turns off the vertical band formats.
const LOOK_NO_VERTICAL_BAND: u16 = 0x0400;

/// An upper limit of the bytes that the table start of one table adds,
/// without the grid columns.
const TABLE_START_GROWTH: usize = 256;

/// An upper limit of the bytes of one grid column.
const GRID_COLUMN_SIZE: usize = 32;

/// Replaces the table properties and the empty grid that docx-rs writes for
/// each table of the document part `document` with the properties from
/// `tables`: the `DataTable` style, the fixed layout, the `w:tblLook` flags
/// and the grid. Marks the header rows with `w:tblHeader`. `tables` has one
/// item for each table, in document order.
///
/// The text in the document part is escaped. Thus each `<w:tbl>` and each
/// `<w:trPr />` is markup. Tables are not nested.
pub(super) fn splice_tables(document: Vec<u8>, tables: &[TableLayout]) -> Result<Vec<u8>> {
    if tables.is_empty() {
        return Ok(document);
    }
    let table_start = Finder::new(TABLE_START_TAG);
    let table_end = Finder::new(TABLE_END_TAG);
    let row_properties = Finder::new(ROW_PROPERTIES);
    let growth: usize = tables
        .iter()
        .map(|table| {
            TABLE_START_GROWTH
                + table.grid.len() * GRID_COLUMN_SIZE
                + table.header_rows * (HEADER_ROW_PROPERTIES.len() - ROW_PROPERTIES.len())
        })
        .sum();
    let mut out = Vec::with_capacity(document.len() + growth);

    let mut rest = &document[..];
    for (index, table) in tables.iter().enumerate() {
        let start = table_start.find(rest).with_context(|| {
            format!(
                "document part has {index} tables, the renderer made {}",
                tables.len()
            )
        })?;
        if !rest[start..].starts_with(DOCX_RS_TABLE_START.as_bytes()) {
            bail!("table {index} does not start with the docx-rs table properties");
        }
        out.extend_from_slice(&rest[..start]);
        write_table_start(&mut out, table)?;
        rest = &rest[start + DOCX_RS_TABLE_START.len()..];

        let end = table_end
            .find(rest)
            .with_context(|| format!("table {index} has no end tag"))?;
        let (mut body, tail) = rest.split_at(end);
        for row in 0..table.header_rows {
            let at = row_properties
                .find(body)
                .with_context(|| format!("table {index} has no header row {row}"))?;
            out.extend_from_slice(&body[..at]);
            out.extend_from_slice(HEADER_ROW_PROPERTIES.as_bytes());
            body = &body[at + ROW_PROPERTIES.len()..];
        }
        out.extend_from_slice(body);
        rest = tail;
    }
    if table_start.find(rest).is_some() {
        bail!(
            "document part has more tables than the {} that the renderer made",
            tables.len()
        );
    }
    out.extend_from_slice(rest);
    Ok(out)
}

/// Writes the start of the table `table` to `out`: the table properties
/// and the grid.
fn write_table_start(out: &mut Vec<u8>, table: &TableLayout) -> std::io::Result<()> {
    let first_row = table.header_rows > 0;
    let look = if first_row { LOOK_FIRST_ROW } else { 0 } | LOOK_NO_VERTICAL_BAND;
    write!(
        out,
        concat!(
            r#"<w:tbl><w:tblPr><w:tblStyle w:val="{style}" />"#,
            r#"<w:tblW w:w="0" w:type="auto" /><w:tblLayout w:type="fixed" />"#,
            r#"<w:tblLook w:val="{look:04X}" w:firstRow="{first_row}" w:lastRow="0" "#,
            r#"w:firstColumn="0" w:lastColumn="0" w:noHBand="0" w:noVBand="1" />"#,
            r#"</w:tblPr><w:tblGrid>"#,
        ),
        style = TABLE_STYLE,
        look = look,
        first_row = u8::from(first_row),
    )?;
    for width in &table.grid {
        write!(out, r#"<w:gridCol w:w="{width}" />"#)?;
    }
    out.extend_from_slice(b"</w:tblGrid>");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(header_rows: usize, grid: &[u32]) -> TableLayout {
        TableLayout {
            header_rows,
            grid: grid.to_vec(),
        }
    }

    /// A table as docx-rs writes it, with `rows` rows of one cell.
    fn docx_rs_table(rows: usize) -> String {
        let rows = (0..rows)
            .map(|_| TableRow::new(vec![TableCell::new()]))
            .collect();
        String::from_utf8(Table::without_borders(rows).build()).unwrap()
    }

    #[test]
    fn docx_rs_table_has_expected_start_and_rows() {
        let xml = docx_rs_table(1);
        assert!(xml.starts_with(DOCX_RS_TABLE_START), "{xml}");
        assert!(xml.contains(ROW_PROPERTIES), "{xml}");
        assert!(xml.ends_with(TABLE_END_TAG), "{xml}");
    }

    #[test]
    fn grid_widths_fill_text_width() {
        assert_eq!(grid_widths(10205, None, 2), [5102, 5103]);
        assert_eq!(grid_widths(10205, Some(vec![4, 8]), 2), [3401, 6804]);
        assert_eq!(grid_widths(10205, Some(vec![4, 8]), 3), [3401, 3401, 3403]);
    }

    #[test]
    fn splice_without_tables_keeps_document() {
        let document = b"<w:body><w:p /></w:body>".to_vec();
        let ptr = document.as_ptr();
        let out = splice_tables(document, &[]).unwrap();
        assert_eq!(out, b"<w:body><w:p /></w:body>");
        assert_eq!(out.as_ptr(), ptr);
    }

    #[test]
    fn splice_writes_properties_and_header_rows() {
        let document = format!(
            "<w:body>{}<w:p />{}</w:body>",
            docx_rs_table(3),
            docx_rs_table(2)
        );
        let out = splice_tables(
            document.into_bytes(),
            &[layout(2, &[100, 200]), layout(0, &[300])],
        )
        .unwrap();
        let out = String::from_utf8(out).unwrap();
        let tables: Vec<&str> = out.split(TABLE_START_TAG).skip(1).collect();
        assert_eq!(tables.len(), 2, "{out}");

        assert!(
            tables[0].starts_with(concat!(
                r#"<w:tblPr><w:tblStyle w:val="DataTable" />"#,
                r#"<w:tblW w:w="0" w:type="auto" /><w:tblLayout w:type="fixed" />"#,
                r#"<w:tblLook w:val="0420" w:firstRow="1" w:lastRow="0" "#,
                r#"w:firstColumn="0" w:lastColumn="0" w:noHBand="0" w:noVBand="1" />"#,
                r#"</w:tblPr><w:tblGrid><w:gridCol w:w="100" /><w:gridCol w:w="200" />"#,
                r#"</w:tblGrid>"#,
            )),
            "{out}"
        );
        assert_eq!(tables[0].matches(HEADER_ROW_PROPERTIES).count(), 2, "{out}");
        assert_eq!(tables[0].matches(ROW_PROPERTIES).count(), 1, "{out}");
        let rows: Vec<&str> = tables[0].split("<w:tr>").skip(1).collect();
        assert!(rows[0].starts_with(HEADER_ROW_PROPERTIES), "{out}");
        assert!(rows[1].starts_with(HEADER_ROW_PROPERTIES), "{out}");
        assert!(rows[2].starts_with(ROW_PROPERTIES), "{out}");

        assert!(
            tables[1].starts_with(concat!(
                r#"<w:tblPr><w:tblStyle w:val="DataTable" />"#,
                r#"<w:tblW w:w="0" w:type="auto" /><w:tblLayout w:type="fixed" />"#,
                r#"<w:tblLook w:val="0400" w:firstRow="0" "#,
            )),
            "{out}"
        );
        assert!(!tables[1].contains("w:tblHeader"), "{out}");
        assert_eq!(tables[1].matches(ROW_PROPERTIES).count(), 2, "{out}");
        assert!(!out.contains("w:tblBorders"), "{out}");
        assert!(out.ends_with("</w:tbl></w:body>"), "{out}");
    }

    #[test]
    fn splice_rejects_fewer_tables_than_layouts() {
        let document = format!("<w:body>{}</w:body>", docx_rs_table(1));
        let err =
            splice_tables(document.into_bytes(), &[layout(1, &[1]), layout(1, &[1])]).unwrap_err();
        assert!(err.to_string().contains("has 1 tables"), "{err}");
    }

    #[test]
    fn splice_rejects_more_tables_than_layouts() {
        let document = format!("<w:body>{0}{0}</w:body>", docx_rs_table(1));
        let err = splice_tables(document.into_bytes(), &[layout(1, &[1])]).unwrap_err();
        assert!(err.to_string().contains("more tables"), "{err}");
    }

    #[test]
    fn splice_rejects_more_header_rows_than_rows() {
        let document = format!("<w:body>{}</w:body>", docx_rs_table(1));
        let err = splice_tables(document.into_bytes(), &[layout(2, &[1])]).unwrap_err();
        assert!(err.to_string().contains("no header row 1"), "{err}");
    }
}
