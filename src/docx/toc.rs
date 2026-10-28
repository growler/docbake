use std::collections::VecDeque;

use anyhow::Result;
use docx_rs::*;
use rushdown::as_kind_data;
use rushdown::ast::KindData;

use crate::graph::DocumentGraph;

use super::DocxRenderer;

/// TOC depth: include heading levels 1..=TOC_DEPTH.
pub(super) const TOC_DEPTH: usize = 3;

impl<'a> DocxRenderer<'a> {
    /// Add the field-based table of contents and the page break after it.
    ///
    /// Also resets the heading bookmark keys and the bookmark counter that
    /// `render_block` uses to put a TOC bookmark on each matching heading.
    pub(super) fn add_table_of_contents(
        &self,
        mut docx: Docx,
        graph: &DocumentGraph,
    ) -> Result<Docx> {
        let toc_entries = self.collect_toc_entries(graph)?;
        *self.toc_heading_keys.borrow_mut() = toc_entries
            .iter()
            .map(|entry| entry.key.clone())
            .collect::<VecDeque<_>>();
        *self.bookmark_counter.borrow_mut() = 0;

        // ── Field-based table of contents ───────────────────────────────
        // Keep field instruction and cached entries so Google Docs and Word
        // can display content without manual update.
        let mut toc = TableOfContents::new()
            .heading_styles_range(1, TOC_DEPTH)
            .hyperlink()
            .without_sdt();
        toc.instr = toc
            .instr
            .clone()
            .hide_tab_and_page_numbers_in_webview()
            .use_applied_paragraph_line_level();
        for entry in toc_entries {
            toc = toc.add_item(
                TableOfContentsItem::new()
                    .level(entry.level)
                    .text(entry.text)
                    .toc_key(entry.key)
                    .page_ref("1"),
            );
        }
        docx = docx.add_table_of_contents(toc);

        docx = docx.add_paragraph(
            Paragraph::new()
                .add_run(Run::new().add_break(BreakType::Page))
                .line_spacing(LineSpacing::new().after(0).line(240)),
        );

        Ok(docx)
    }

    fn collect_toc_entries(&self, graph: &DocumentGraph) -> Result<Vec<TocEntry>> {
        let mut entries = Vec::new();
        let mut idx = 0usize;

        for (doc_id, node_ref, heading_shift) in
            graph.rendered_nodes_preorder_visible(&self.profile)?
        {
            let doc = graph.document(doc_id);
            let arena = &doc.arena;
            let source = &doc.source;

            if !matches!(arena[node_ref].kind_data(), KindData::Heading(_)) {
                continue;
            }
            if crate::html::is_admonition_title_heading(arena, node_ref, source) {
                continue;
            }

            let heading = as_kind_data!(arena[node_ref], Heading);
            let level = (heading.level() as usize + heading_shift).min(6);
            if level > TOC_DEPTH {
                continue;
            }

            let text = Self::collect_inline_text(arena, source, node_ref);
            if text.is_empty() {
                continue;
            }

            idx += 1;
            entries.push(TocEntry {
                key: format!("_Toc{idx:09}"),
                text,
                level,
            });
        }

        Ok(entries)
    }

    /// Generate a static table of contents from headings in the graph.
    #[allow(dead_code)]
    fn render_static_toc(&self, mut docx: Docx, graph: &DocumentGraph) -> Docx {
        // "Table of Contents" header — bold, larger, but NOT a heading
        // style to avoid it appearing in the outline.
        let toc_header = Paragraph::new()
            .add_run(Run::new().add_text("Table of Contents").size(32).bold())
            .line_spacing(LineSpacing::new().after(200).line(240));
        docx = docx.add_paragraph(toc_header);

        // Collect headings from the rendered node preorder walk.
        let rendered = graph
            .rendered_nodes_preorder_visible(&self.profile)
            .unwrap_or_default();
        for (doc_id, node_ref, heading_shift) in rendered {
            let doc = graph.document(doc_id);
            let arena = &doc.arena;
            let source = &doc.source;

            if let KindData::Heading(_) = arena[node_ref].kind_data() {
                // Skip admonition title headings.
                if crate::html::is_admonition_title_heading(arena, node_ref, source) {
                    continue;
                }

                let heading = as_kind_data!(arena[node_ref], Heading);
                let level = (heading.level() as usize + heading_shift).min(6);

                if level > TOC_DEPTH {
                    continue;
                }

                let text = Self::collect_inline_text(arena, source, node_ref);
                if text.is_empty() {
                    continue;
                }

                // Indent: 360 twips per level.
                let indent = ((level - 1) as i32) * 360;
                let size = match level {
                    1 => 24, // 12pt bold
                    2 => 22, // 11pt
                    _ => 20, // 10pt
                };
                let mut run = Run::new().add_text(&text).size(size).color("333333");
                if level == 1 {
                    run = run.bold();
                }
                let para = Paragraph::new()
                    .add_run(run)
                    .indent(Some(indent), None, None, None)
                    .line_spacing(LineSpacing::new().after(40).before(0).line(240));
                docx = docx.add_paragraph(para);
            }
        }
        docx
    }
}

struct TocEntry {
    key: String,
    text: String,
    level: usize,
}
