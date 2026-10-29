use std::collections::VecDeque;

use anyhow::Result;
use docx_rs::*;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, KindData, NodeRef};
use rushdown_footnote::FootnoteDefinition;

use crate::graph::DocumentGraph;

use super::DocxRenderer;

/// TOC depth: include heading levels 1..=TOC_DEPTH.
pub(super) const TOC_DEPTH: usize = 3;

/// The paragraph style of a TOC entry for each heading level.
const TOC_STYLES: [&str; 9] = [
    "TOC1", "TOC2", "TOC3", "TOC4", "TOC5", "TOC6", "TOC7", "TOC8", "TOC9",
];

impl<'a> DocxRenderer<'a> {
    /// Add the field-based table of contents and the page break after it.
    /// A document without TOC entries gets neither.
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

        let Some(last) = toc_entries.last() else {
            return Ok(docx);
        };

        // ── Field-based table of contents ───────────────────────────────
        // Keep field instruction and cached entries so Google Docs and Word
        // can display content without manual update.
        let instr = InstrText::TOC(
            InstrToC::new()
                .heading_styles_range(1, TOC_DEPTH)
                .hyperlink()
                .hide_tab_and_page_numbers_in_webview()
                .use_applied_paragraph_line_level(),
        );
        let end_style = TOC_STYLES[last.level - 1];
        let tab = Tab::new()
            .val(TabValueType::Right)
            .leader(TabLeaderType::Dot)
            .pos(self.params.page.text_width() as usize);
        let mut sdt = StructuredDataTag::new();
        let mut instr = Some(instr);
        for entry in toc_entries {
            let mut p = Paragraph::new()
                .style(TOC_STYLES[entry.level - 1])
                .add_tab(tab.clone());
            if let Some(instr) = instr.take() {
                p = p.add_run(
                    Run::new()
                        .add_field_char(FieldCharType::Begin, false)
                        .add_instr_text(instr)
                        .add_field_char(FieldCharType::Separate, false),
                );
            }
            let page_ref = Run::new()
                .add_field_char(FieldCharType::Begin, false)
                .add_instr_text(InstrText::PAGEREF(
                    InstrPAGEREF::new(&entry.key).hyperlink(),
                ))
                .add_field_char(FieldCharType::Separate, false)
                .add_text("1")
                .add_field_char(FieldCharType::End, false);
            p = p.add_hyperlink(
                Hyperlink::new(entry.key, HyperlinkType::Anchor)
                    .add_run(Run::new().add_text(entry.text))
                    .add_run(Run::new().add_tab())
                    .add_run(page_ref),
            );
            sdt = sdt.add_paragraph(p);
        }
        sdt = sdt.add_paragraph(
            Paragraph::new()
                .style(end_style)
                .add_run(Run::new().add_field_char(FieldCharType::End, false)),
        );
        docx = docx
            .add_structured_data_tag(sdt)
            .add_paragraph(Paragraph::new().add_run(Run::new().add_break(BreakType::Page)));

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
            if crate::html::is_admonition_title_heading(arena, node_ref, source)
                || in_footnote_definition(arena, node_ref)
            {
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
}

/// Tells if `node_ref` is in a footnote definition. The text of a footnote
/// definition is in the footnotes part, thus its headings take no TOC
/// entry.
fn in_footnote_definition(arena: &Arena, node_ref: NodeRef) -> bool {
    let mut parent = arena[node_ref].parent();
    while let Some(p) = parent {
        if rushdown::matches_extension_kind!(arena, p, FootnoteDefinition) {
            return true;
        }
        parent = arena[p].parent();
    }
    false
}

struct TocEntry {
    key: String,
    text: String,
    level: usize,
}
