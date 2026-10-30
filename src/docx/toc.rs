use std::collections::hash_map::Entry;

use anyhow::{bail, Result};
use docx_rs::*;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, KindData, NodeRef};
use rushdown_footnote::FootnoteDefinition;

use crate::caption::{Caption, CaptionText};
use crate::graph::DocumentGraph;

use super::links::bookmark_name;
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
    /// Also collects the targets of the internal links and resets the
    /// bookmark counter that `render_block` uses for the bookmark of each
    /// heading and caption.
    pub(super) fn add_table_of_contents(
        &self,
        mut docx: Docx,
        graph: &DocumentGraph,
    ) -> Result<Docx> {
        let toc_entries = self.collect_toc_entries(graph)?;
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

    /// Gives the TOC entries, and collects the targets of the internal
    /// links: the visible headings and the captions of visible blocks,
    /// without admonition titles and without the content of footnote
    /// definitions. Two IDs with the same bookmark name are an error.
    fn collect_toc_entries(&self, graph: &DocumentGraph) -> Result<Vec<TocEntry>> {
        let mut entries = Vec::new();
        let mut targets = self.link_targets.borrow_mut();
        targets.clear();

        for (doc_id, node_ref, heading_shift) in
            graph.rendered_nodes_preorder_visible(&self.profile)?
        {
            let doc = graph.document(doc_id);
            let arena = &doc.arena;
            let source = &doc.source;

            let is_heading = matches!(arena[node_ref].kind_data(), KindData::Heading(_));
            let is_target = if is_heading {
                !crate::html::is_admonition_title_heading(arena, node_ref, source)
            } else if rushdown::matches_extension_kind!(arena, node_ref, Caption) {
                let block = arena[node_ref]
                    .children(arena)
                    .find(|&c| !rushdown::matches_extension_kind!(arena, c, CaptionText));
                match block {
                    Some(block) => {
                        crate::conditional::node_is_visible(arena, block, source, &self.profile)?
                    }
                    None => true,
                }
            } else {
                false
            };
            if !is_target || in_footnote_definition(arena, node_ref) {
                continue;
            }
            let Some(id) = arena[node_ref].attributes().get("id") else {
                continue;
            };
            let id = id.str(source);
            let key = bookmark_name(&id);
            match targets.entry(key.clone()) {
                Entry::Occupied(other) if *other.get() != id => {
                    bail!(
                        "the IDs `{}` and `{id}` have the same bookmark name",
                        other.get()
                    )
                }
                Entry::Occupied(_) => {}
                Entry::Vacant(entry) => {
                    entry.insert(id.to_string());
                }
            }
            if !is_heading {
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

            entries.push(TocEntry { key, text, level });
        }

        Ok(entries)
    }
}

/// Tells if `node_ref` is in a footnote definition. The text of a footnote
/// definition is in the footnotes part, thus its headings and captions
/// take no TOC entry and no bookmark.
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
