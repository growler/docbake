use anyhow::Result;
use docx_rs::*;
use rushdown::as_kind_data;
use rushdown::ast::{KindData, NodeRef};
use rushdown_definition_list::{DefinitionList, Term, TermDefinition};
use rushdown_fenced_div::FencedDiv;

use crate::caption::{Caption, CaptionText};

use super::numbering::{BULLET_NUMBERING_ID, ORDERED_NUMBERING_ID};
use super::toc::TOC_DEPTH;
use super::{DocxRenderer, RenderContext};

impl<'a> DocxRenderer<'a> {
    pub(super) fn render_block(
        &self,
        mut docx: Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Docx> {
        let arena = context.arena;
        let source = context.source;
        match arena[node_ref].kind_data() {
            KindData::Heading(_) => {
                let heading = as_kind_data!(arena[node_ref], Heading);
                let level = (heading.level() as usize + context.heading_shift).min(6);
                let style = format!("Heading{level}");
                let heading_text = Self::collect_inline_text(arena, source, node_ref);

                let mut para = Paragraph::new().style(&style);

                let mut has_bookmark = false;
                let eligible_for_toc_bookmark = level <= TOC_DEPTH
                    && !heading_text.is_empty()
                    && !crate::html::is_admonition_title_heading(arena, node_ref, source);
                if eligible_for_toc_bookmark {
                    let next_key = self.toc_heading_keys.borrow_mut().pop_front();
                    if let Some(key) = next_key {
                        let id = *self.bookmark_counter.borrow();
                        *self.bookmark_counter.borrow_mut() = id + 1;
                        para = para.add_bookmark_start(id, key);
                        has_bookmark = true;
                    }
                }

                for child in arena[node_ref].children(arena) {
                    let runs = self.render_inline_runs(context, child);
                    for run in runs {
                        para = para.add_run(run);
                    }
                }
                if has_bookmark {
                    let id = *self.bookmark_counter.borrow() - 1;
                    para = para.add_bookmark_end(id);
                }
                docx = docx.add_paragraph(para);
            }
            KindData::Paragraph(_) => {
                let mut para = Paragraph::new();
                for child in arena[node_ref].children(arena) {
                    match arena[child].kind_data() {
                        KindData::Image(_) => {
                            if let Some(pic) = self.resolve_graph_image(context, child) {
                                para = para.add_run(Run::new().add_image(pic));
                            }
                        }
                        _ => {
                            let runs = self.render_inline_runs(context, child);
                            for run in runs {
                                para = para.add_run(run);
                            }
                        }
                    }
                }
                docx = docx.add_paragraph(para);
            }
            KindData::CodeBlock(code_block) => {
                let mut code_text = String::new();
                for line in code_block.value().iter(source) {
                    code_text.push_str(&line);
                }
                docx = self.render_code_block_paragraphs(docx, &code_text);
            }
            KindData::Blockquote(_) => {
                for child in arena[node_ref].children(arena) {
                    docx = self.render_block(docx, context, child)?;
                }
            }
            KindData::List(list) => {
                let is_ordered = list.is_ordered();
                let numbering_id = if is_ordered {
                    ORDERED_NUMBERING_ID
                } else {
                    BULLET_NUMBERING_ID
                };

                docx = self.render_list_items(docx, context, node_ref, numbering_id, 0)?;
            }
            KindData::Table(_) => {
                docx = self.render_table(docx, context, node_ref)?;
            }
            KindData::ThematicBreak(_) => {
                let para = Paragraph::new().add_run(Run::new().add_text(""));
                docx = docx.add_paragraph(para);
            }
            _ => {
                // Handle extension nodes (diagrams, fenced divs, captions, etc.)
                if rushdown::matches_extension_kind!(arena, node_ref, Caption) {
                    docx = self.render_caption(docx, context, node_ref)?;
                } else if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::diagram::Diagram
                ) {
                    docx = self.render_diagram(docx, arena, source, node_ref)?;
                } else if rushdown::matches_extension_kind!(arena, node_ref, FencedDiv) {
                    docx = self.render_fenced_div(docx, context, node_ref)?;
                } else if rushdown::matches_extension_kind!(arena, node_ref, DefinitionList) {
                    docx = self.render_definition_list(docx, context, node_ref)?;
                }
                // FootnoteDefinition nodes are rendered inline at reference sites — skip here.
            }
        }
        Ok(docx)
    }

    /// Render a fenced div, with special handling for admonition blocks.
    ///
    /// For admonition types (`note`, `warning`, `tip`, etc.) the first
    /// child heading is rendered as bold text (not a real heading) to
    /// avoid polluting the document outline / TOC.
    fn render_fenced_div(
        &self,
        mut docx: Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Docx> {
        let arena = context.arena;
        let source = context.source;
        if !crate::conditional::fenced_div_is_visible(arena, node_ref, source, &self.profile)? {
            return Ok(docx);
        }
        let classes = arena[node_ref]
            .attributes()
            .get("class")
            .map(|v| v.str(source))
            .unwrap_or_default();

        let adm_type = crate::admonition_type(&classes);

        if let Some(adm_type) = adm_type {
            docx = self.render_admonition(docx, context, node_ref, adm_type)?;
        } else {
            // Non-admonition fenced div: render children as normal blocks.
            for child in arena[node_ref].children(arena) {
                docx = self.render_block(docx, context, child)?;
            }
        }
        Ok(docx)
    }

    /// Render list items recursively, handling nested lists.
    fn render_list_items(
        &self,
        mut docx: Docx,
        context: &RenderContext<'_>,
        list_ref: NodeRef,
        numbering_id: usize,
        indent_level: usize,
    ) -> Result<Docx> {
        let arena = context.arena;
        for item_ref in arena[list_ref].children(arena) {
            if !matches!(arena[item_ref].kind_data(), KindData::ListItem(_)) {
                continue;
            }

            for child in arena[item_ref].children(arena) {
                match arena[child].kind_data() {
                    KindData::Paragraph(_) => {
                        let mut para = Paragraph::new().numbering(
                            NumberingId::new(numbering_id),
                            IndentLevel::new(indent_level),
                        );
                        for inline in arena[child].children(arena) {
                            let runs = self.render_inline_runs(context, inline);
                            for run in runs {
                                para = para.add_run(run);
                            }
                        }
                        docx = docx.add_paragraph(para);
                    }
                    KindData::List(nested_list) => {
                        let nested_id = if nested_list.is_ordered() {
                            ORDERED_NUMBERING_ID
                        } else {
                            BULLET_NUMBERING_ID
                        };
                        docx = self.render_list_items(
                            docx,
                            context,
                            child,
                            nested_id,
                            indent_level + 1,
                        )?;
                    }
                    _ => {
                        docx = self.render_block(docx, context, child)?;
                    }
                }
            }
        }
        Ok(docx)
    }

    /// Render a definition list.
    fn render_definition_list(
        &self,
        mut docx: Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Docx> {
        let arena = context.arena;
        for child in arena[node_ref].children(arena) {
            if rushdown::matches_extension_kind!(arena, child, Term) {
                let mut para = Paragraph::new();
                for inline in arena[child].children(arena) {
                    let runs = self.render_inline_runs(context, inline);
                    for run in runs {
                        para = para.add_run(run.bold());
                    }
                }
                docx = docx.add_paragraph(para);
            } else if rushdown::matches_extension_kind!(arena, child, TermDefinition) {
                for def_child in arena[child].children(arena) {
                    if matches!(arena[def_child].kind_data(), KindData::Paragraph(_)) {
                        let mut para = Paragraph::new().indent(Some(720), None, None, None);
                        for inline in arena[def_child].children(arena) {
                            let runs = self.render_inline_runs(context, inline);
                            for run in runs {
                                para = para.add_run(run);
                            }
                        }
                        docx = docx.add_paragraph(para);
                    }
                }
            }
        }
        Ok(docx)
    }

    /// Render a Caption extension node.
    fn render_caption(
        &self,
        mut docx: Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Docx> {
        let arena = context.arena;
        let mut caption_text_ref = None;
        let mut block_ref = None;

        for child in arena[node_ref].children(arena) {
            if rushdown::matches_extension_kind!(arena, child, CaptionText) {
                caption_text_ref = Some(child);
            } else {
                block_ref = Some(child);
            }
        }

        if let Some(ct_ref) = caption_text_ref {
            let mut para = Paragraph::new().line_spacing(LineSpacing::new().after(60).line(240));
            for child in arena[ct_ref].children(arena) {
                let runs = self.render_inline_runs(context, child);
                for run in runs {
                    para = para.add_run(run.italic().color("444444"));
                }
            }
            docx = docx.add_paragraph(para);
        }

        if let Some(br) = block_ref {
            docx = self.render_block(docx, context, br)?;
        }

        Ok(docx)
    }

    /// Render code block lines as `CodeBlock` paragraphs, one for each line.
    /// The style carries the font, fill and border box. Adjacent paragraphs
    /// with identical borders merge into a continuous box in Word and
    /// Google Docs.
    fn render_code_block_paragraphs(&self, mut docx: Docx, code_text: &str) -> Docx {
        for line in code_text.lines() {
            let para = Paragraph::new()
                .style("CodeBlock")
                .add_run(Run::new().add_text(line));
            docx = docx.add_paragraph(para);
        }
        docx
    }
}
