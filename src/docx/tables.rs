use anyhow::Result;
use docx_rs::*;
use rushdown::ast::{KindData, NodeRef};

use super::{DocxRenderer, RenderContext};

impl<'a> DocxRenderer<'a> {
    /// Render a table to a DOCX table.
    pub(super) fn render_table(
        &self,
        mut docx: Docx,
        context: &RenderContext<'_>,
        table_ref: NodeRef,
    ) -> Result<Docx> {
        let arena = context.arena;
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

        let mut rows = Vec::new();
        for row_ref in &header_rows {
            let cells = self.render_table_row(context, *row_ref, true)?;
            rows.push(TableRow::new(cells));
        }
        for row_ref in &body_rows {
            let cells = self.render_table_row(context, *row_ref, false)?;
            rows.push(TableRow::new(cells));
        }

        if !rows.is_empty() {
            let table = Table::new(rows);
            docx = docx.add_table(table).add_paragraph(Paragraph::new());
        }

        Ok(docx)
    }

    /// Render a single table row, returning a Vec of cells.
    fn render_table_row(
        &self,
        context: &RenderContext<'_>,
        row_ref: NodeRef,
        is_header: bool,
    ) -> Result<Vec<TableCell>> {
        let arena = context.arena;
        let mut cells = Vec::new();
        for cell_ref in arena[row_ref].children(arena) {
            if !matches!(arena[cell_ref].kind_data(), KindData::TableCell(_)) {
                continue;
            }
            let mut para = Paragraph::new();
            for child in arena[cell_ref].children(arena) {
                let mut runs = self.render_inline_runs(context, child);
                if is_header {
                    runs = runs.into_iter().map(|r| r.bold()).collect();
                }
                for run in runs {
                    para = para.add_run(run);
                }
            }
            cells.push(TableCell::new().add_paragraph(para));
        }
        Ok(cells)
    }
}
