use anyhow::Result;
use docx_rs::*;
use rushdown::as_kind_data;
use rushdown::ast::{KindData, NodeRef, Task};
use rushdown_definition_list::{DefinitionList, Term, TermDefinition};
use rushdown_fenced_div::FencedDiv;

use crate::caption::{Caption, CaptionText};

use super::numbering::{ListKind, LIST_LEVELS};
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
                if let Some(style) = context.paragraph_style {
                    para = para.style(style);
                }
                for child in arena[node_ref].children(arena) {
                    let runs = self.render_inline_runs(context, child);
                    for run in runs {
                        para = para.add_run(run);
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
                let context = RenderContext {
                    paragraph_style: Some("Quote"),
                    ..*context
                };
                for child in arena[node_ref].children(arena) {
                    docx = self.render_block(docx, &context, child)?;
                }
            }
            KindData::List(_) => {
                docx = self.render_list(docx, context, node_ref, 0)?;
            }
            KindData::Table(_) => {
                docx = self.render_table(docx, context, node_ref)?;
            }
            KindData::ThematicBreak(_) => {
                let para = Paragraph::new()
                    .style("ThematicBreak")
                    .add_run(Run::new().add_text(""));
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
    /// child heading is rendered as the admonition header paragraph (not a
    /// real heading) to avoid polluting the document outline / TOC.
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
            // Non-admonition fenced div: render children as normal blocks
            // with the paragraph style of the enclosing block.
            for child in arena[node_ref].children(arena) {
                docx = self.render_block(docx, context, child)?;
            }
        }
        Ok(docx)
    }

    /// Render a list at the list level `level` and its nested lists. The
    /// list gets its own `w:num`. An ordered list starts at its start
    /// number. A list below the sixth level uses the sixth level.
    ///
    /// An unordered list with a task item is a task list. A task item
    /// starts with a box character. A list item without a task in a task
    /// list starts with the bullet character of the level.
    ///
    /// The first paragraph of an item has the marker. An item that is
    /// empty or starts with another block gets an empty paragraph with the
    /// marker first. The next paragraphs of the item use a `w:num` of
    /// `TaskList` at the same level, thus they have the indent of the item
    /// and no marker. A task list uses its own `w:num` for them. The other
    /// blocks in an item do not use the paragraph style of the enclosing
    /// block.
    fn render_list(
        &self,
        mut docx: Docx,
        context: &RenderContext<'_>,
        list_ref: NodeRef,
        level: usize,
    ) -> Result<Docx> {
        let context = &RenderContext {
            paragraph_style: None,
            ..*context
        };
        let arena = context.arena;
        let list = as_kind_data!(arena[list_ref], List);
        let items: Vec<_> = arena[list_ref]
            .children(arena)
            .filter_map(|item_ref| match arena[item_ref].kind_data() {
                KindData::ListItem(item) => Some((item_ref, item.task())),
                _ => None,
            })
            .collect();
        let kind = if list.is_ordered() {
            ListKind::Ordered
        } else if items.iter().any(|(_, task)| task.is_some()) {
            ListKind::Task
        } else {
            ListKind::Bullet
        };
        let ilvl = level.min(LIST_LEVELS - 1);
        let start = list.is_ordered().then(|| list.start().max(1) as usize);
        let num_id;
        (docx, num_id) = self.add_list_num(docx, kind, ilvl, start);
        // The `w:num` of the next paragraphs of the items.
        let mut next_num_id = (kind == ListKind::Task).then_some(num_id);
        let list_paragraph = |id: usize, marker: Option<&str>| {
            let para = Paragraph::new()
                .style("ListParagraph")
                .numbering(NumberingId::new(id), IndentLevel::new(ilvl));
            match marker {
                Some(marker) => para.add_run(Run::new().add_text(format!("{marker} "))),
                None => para,
            }
        };

        let lists = &self.params.lists;
        for (item_ref, task) in items {
            let mut marker = match task {
                Some(Task::Completed) => Some(lists.task_list.checked.as_str()),
                Some(Task::Active) => Some(lists.task_list.unchecked.as_str()),
                _ if kind == ListKind::Task => Some(lists.bullet_list[ilvl].as_str()),
                _ => None,
            };
            let starts_with_paragraph = arena[item_ref]
                .children(arena)
                .next()
                .is_some_and(|child| matches!(arena[child].kind_data(), KindData::Paragraph(_)));
            if !starts_with_paragraph {
                docx = docx.add_paragraph(list_paragraph(num_id, marker.take()));
            }
            let mut first = starts_with_paragraph;
            for child in arena[item_ref].children(arena) {
                match arena[child].kind_data() {
                    KindData::Paragraph(_) => {
                        let id = if first {
                            num_id
                        } else if let Some(id) = next_num_id {
                            id
                        } else {
                            let id;
                            (docx, id) = self.add_list_num(docx, ListKind::Task, ilvl, None);
                            next_num_id = Some(id);
                            id
                        };
                        first = false;
                        let mut para = list_paragraph(id, marker.take());
                        for inline in arena[child].children(arena) {
                            let runs = self.render_inline_runs(context, inline);
                            for run in runs {
                                para = para.add_run(run);
                            }
                        }
                        docx = docx.add_paragraph(para);
                    }
                    KindData::List(_) => {
                        docx = self.render_list(docx, context, child, level + 1)?;
                    }
                    _ => {
                        docx = self.render_block(docx, context, child)?;
                    }
                }
            }
        }
        Ok(docx)
    }

    /// Add a `w:num` of the numbering definition `kind` and give its ID.
    /// `start` is the start number of the level `ilvl`.
    fn add_list_num(
        &self,
        docx: Docx,
        kind: ListKind,
        ilvl: usize,
        start: Option<usize>,
    ) -> (Docx, usize) {
        let id = self.list_num_counter.replace_with(|id| *id + 1);
        let mut num = Numbering::new(id, kind.abstract_num_id());
        if let Some(start) = start {
            num = num.add_override(LevelOverride::new(ilvl).start(start));
        }
        (docx.add_numbering(num), id)
    }

    /// Render a definition list. Terms use `DefinitionTerm`. The
    /// paragraphs of a definition use `DefinitionBody`; the other blocks of
    /// a definition keep their own styles.
    fn render_definition_list(
        &self,
        mut docx: Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Docx> {
        let arena = context.arena;
        for child in arena[node_ref].children(arena) {
            if rushdown::matches_extension_kind!(arena, child, Term) {
                let mut para = Paragraph::new().style("DefinitionTerm");
                for inline in arena[child].children(arena) {
                    let runs = self.render_inline_runs(context, inline);
                    for run in runs {
                        para = para.add_run(run);
                    }
                }
                docx = docx.add_paragraph(para);
            } else if rushdown::matches_extension_kind!(arena, child, TermDefinition) {
                let context = RenderContext {
                    paragraph_style: Some("DefinitionBody"),
                    ..*context
                };
                for def_child in arena[child].children(arena) {
                    docx = self.render_block(docx, &context, def_child)?;
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
