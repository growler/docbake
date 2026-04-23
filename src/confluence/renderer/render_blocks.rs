use std::fmt::Write as _;

use anyhow::Result;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, KindData, NodeRef, TableCellAlignment, Task};
use rushdown_definition_list::{DefinitionList, Term, TermDefinition};
use rushdown_fenced_div::FencedDiv;
use rushdown_footnote::FootnoteDefinition;

use crate::graph::PreparedDocument;

use super::types::{FootnoteEntry, RenderContext, Scope};
use super::xml::{cdata_escape, xml_escape};
use super::ConfluenceRenderer;

impl<'a> ConfluenceRenderer<'a> {
    pub(super) fn render_graph_document(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        doc_id: usize,
        heading_shift: usize,
        footnotes: &[FootnoteEntry],
        skip_first_rendered_block: &mut bool,
    ) -> Result<()> {
        let doc = prepared.graph().document(doc_id);
        for child in doc.arena[doc.doc_ref].children(&doc.arena) {
            if !crate::conditional::node_is_visible(&doc.arena, child, &doc.source, &self.profile)?
            {
                continue;
            }
            if let Some(entries) = prepared.graph().include_entries(doc_id, child) {
                for entry in entries {
                    self.render_graph_document(
                        out,
                        prepared,
                        entry.doc_id,
                        heading_shift + entry.heading_shift,
                        footnotes,
                        skip_first_rendered_block,
                    )?;
                }
                continue;
            }
            if *skip_first_rendered_block {
                *skip_first_rendered_block = false;
                continue;
            }
            let context = RenderContext {
                prepared,
                scope: Scope { doc_id },
                arena: &doc.arena,
                source: &doc.source,
                heading_shift,
                footnotes,
            };
            self.render_block(out, &context, child)?;
        }
        Ok(())
    }

    pub(super) fn render_block(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        self.render_anchor_if_node_has_id(out, arena, source, node_ref)?;
        match arena[node_ref].kind_data() {
            KindData::Paragraph(_) => {
                out.push_str("<p>");
                self.render_inline_children(out, context, node_ref)?;
                out.push_str("</p>\n");
            }
            KindData::Heading(heading) => {
                let level = (heading.level() as usize + context.heading_shift).min(6);
                write!(out, "<h{level}>")?;
                self.render_inline_children(out, context, node_ref)?;
                writeln!(out, "</h{level}>")?;
            }
            KindData::ThematicBreak(_) => {
                out.push_str("<hr />\n");
            }
            KindData::CodeBlock(code) => {
                let lang = code.language_str(source).unwrap_or_default().to_string();
                let mut value = String::new();
                for line in code.value().iter(source) {
                    value.push_str(line.as_ref());
                }
                if value.ends_with('\n') {
                    value.pop();
                }
                out.push_str("<ac:structured-macro ac:name=\"code\">");
                if !lang.is_empty() {
                    write!(
                        out,
                        "<ac:parameter ac:name=\"language\">{}</ac:parameter>",
                        xml_escape(&lang)
                    )?;
                }
                write!(
                    out,
                    "<ac:plain-text-body><![CDATA[{}]]></ac:plain-text-body>",
                    cdata_escape(&value)
                )?;
                out.push_str("</ac:structured-macro>\n");
            }
            KindData::Blockquote(_) => {
                out.push_str("<blockquote>");
                for child in arena[node_ref].children(arena) {
                    self.render_block(out, context, child)?;
                }
                out.push_str("</blockquote>\n");
            }
            KindData::List(list) => {
                let is_task_list = !list.is_ordered()
                    && arena[node_ref].children(arena).any(|item| {
                        matches!(arena[item].kind_data(), KindData::ListItem(li) if li.is_task())
                    });
                if list.is_ordered() {
                    out.push_str("<ol>\n");
                } else {
                    out.push_str("<ul>\n");
                }
                for item in arena[node_ref].children(arena) {
                    out.push_str("<li>");
                    if is_task_list {
                        if let KindData::ListItem(li) = arena[item].kind_data() {
                            match li.task() {
                                Some(Task::Checked) => {
                                    out.push_str("<ac:task-list><ac:task><ac:task-status>complete</ac:task-status><ac:task-body>");
                                    for child in arena[item].children(arena) {
                                        self.render_block(out, context, child)?;
                                    }
                                    out.push_str("</ac:task-body></ac:task></ac:task-list>");
                                    out.push_str("</li>\n");
                                    continue;
                                }
                                Some(Task::Unchecked) => {
                                    out.push_str("<ac:task-list><ac:task><ac:task-status>incomplete</ac:task-status><ac:task-body>");
                                    for child in arena[item].children(arena) {
                                        self.render_block(out, context, child)?;
                                    }
                                    out.push_str("</ac:task-body></ac:task></ac:task-list>");
                                    out.push_str("</li>\n");
                                    continue;
                                }
                                _ => {}
                            }
                        }
                    }
                    for child in arena[item].children(arena) {
                        self.render_block(out, context, child)?;
                    }
                    out.push_str("</li>\n");
                }
                if list.is_ordered() {
                    out.push_str("</ol>\n");
                } else {
                    out.push_str("</ul>\n");
                }
            }
            KindData::Table(_) => {
                self.render_table(out, context, node_ref)?;
            }
            KindData::HtmlBlock(html) => {
                // Pass through raw HTML as-is -- Confluence may or may not
                // render it, but stripping it loses information.
                for line in html.value().iter(source) {
                    out.push_str(line.as_ref());
                }
            }
            KindData::RawHtml(_) => {
                // Inline raw HTML -- skip in Confluence output.
            }
            KindData::LinkReferenceDefinition(_) => {}
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, DefinitionList) =>
            {
                self.render_definition_list(out, context, node_ref)?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FencedDiv) =>
            {
                if !crate::conditional::fenced_div_is_visible(
                    arena,
                    node_ref,
                    source,
                    &self.profile,
                )? {
                    return Ok(());
                }
                let classes = arena[node_ref]
                    .attributes()
                    .get("class")
                    .map(|v| v.str(source))
                    .unwrap_or_default();

                if let Some(adm_type) = crate::admonition_type(&classes) {
                    self.render_admonition(out, context, node_ref, adm_type)?;
                } else {
                    for child in arena[node_ref].children(arena) {
                        self.render_block(out, context, child)?;
                    }
                }
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, crate::caption::Caption) =>
            {
                self.render_caption(out, context, node_ref)?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::caption::CaptionText
                ) =>
            {
                self.render_inline_children(out, context, node_ref)?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, crate::diagram::Diagram) =>
            {
                self.render_diagram(out, context, node_ref)?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FootnoteDefinition) =>
            {
                // Rendered separately in the footnotes section.
            }
            KindData::Extension(_) => {}
            _ => {}
        }
        Ok(())
    }

    fn render_table(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        out.push_str("<table><tbody>\n");
        for child in arena[node_ref].children(arena) {
            match arena[child].kind_data() {
                KindData::TableHeader(_) => {
                    for row in arena[child].children(arena) {
                        out.push_str("<tr>");
                        for cell in arena[row].children(arena) {
                            let align = self.table_cell_align(arena, cell);
                            if align.is_empty() {
                                out.push_str("<th>");
                            } else {
                                write!(out, "<th style=\"text-align: {align}\">")?;
                            }
                            self.render_inline_children(out, context, cell)?;
                            out.push_str("</th>");
                        }
                        out.push_str("</tr>\n");
                    }
                }
                KindData::TableBody(_) => {
                    for row in arena[child].children(arena) {
                        out.push_str("<tr>");
                        for cell in arena[row].children(arena) {
                            let align = self.table_cell_align(arena, cell);
                            if align.is_empty() {
                                out.push_str("<td>");
                            } else {
                                write!(out, "<td style=\"text-align: {align}\">")?;
                            }
                            self.render_inline_children(out, context, cell)?;
                            out.push_str("</td>");
                        }
                        out.push_str("</tr>\n");
                    }
                }
                _ => {}
            }
        }
        out.push_str("</tbody></table>\n");
        Ok(())
    }

    fn table_cell_align(&self, arena: &Arena, cell: NodeRef) -> &'static str {
        match as_kind_data!(arena, cell, TableCell).alignment() {
            TableCellAlignment::Left => "left",
            TableCellAlignment::Center => "center",
            TableCellAlignment::Right => "right",
            _ => "",
        }
    }

    fn render_definition_list(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        // Confluence doesn't support <dl>; render as a two-column table.
        out.push_str("<table><tbody>\n");
        let children: Vec<_> = arena[node_ref].children(arena).collect();
        let mut index = 0usize;
        while index < children.len() {
            let mut terms = Vec::new();
            while index < children.len()
                && rushdown::matches_extension_kind!(arena, children[index], Term)
            {
                terms.push(children[index]);
                index += 1;
            }
            let mut definitions = Vec::new();
            while index < children.len()
                && rushdown::matches_extension_kind!(arena, children[index], TermDefinition)
            {
                definitions.push(children[index]);
                index += 1;
            }
            if terms.is_empty() {
                index += 1;
                continue;
            }
            for term_ref in &terms {
                out.push_str("<tr><td><strong>");
                self.render_inline_children(out, context, *term_ref)?;
                out.push_str("</strong></td><td>");
                let definition_context = RenderContext {
                    heading_shift: 0,
                    ..*context
                };
                for def_ref in &definitions {
                    for child in arena[*def_ref].children(arena) {
                        self.render_block(out, &definition_context, child)?;
                    }
                }
                out.push_str("</td></tr>\n");
            }
        }
        out.push_str("</tbody></table>\n");
        Ok(())
    }

    fn render_admonition(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
        adm_type: &str,
    ) -> Result<()> {
        let arena = context.arena;
        let confluence_macro = match adm_type {
            "note" => "note",
            "warning" | "important" | "caution" => "warning",
            "tip" => "tip",
            "related" => "info",
            _ => "info",
        };

        let children: Vec<_> = arena[node_ref].children(arena).collect();
        let (title, skip_first) = if let Some(&first) = children.first() {
            if matches!(arena[first].kind_data(), KindData::Heading(_)) {
                let mut title_buf = String::new();
                self.render_inline_children(&mut title_buf, context, first)?;
                (Some(title_buf), true)
            } else {
                (None, false)
            }
        } else {
            (None, false)
        };

        write!(out, "<ac:structured-macro ac:name=\"{confluence_macro}\">")?;
        if let Some(ref t) = title {
            write!(
                out,
                "<ac:parameter ac:name=\"title\">{}</ac:parameter>",
                xml_escape(t)
            )?;
        }
        out.push_str("<ac:rich-text-body>");
        let body_children = if skip_first {
            &children[1..]
        } else {
            &children[..]
        };
        for &child in body_children {
            self.render_block(out, context, child)?;
        }
        out.push_str("</ac:rich-text-body></ac:structured-macro>\n");
        Ok(())
    }

    fn render_caption(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        let mut block_child = None;
        let mut caption_text_child = None;
        for child in arena[node_ref].children(arena) {
            if rushdown::matches_extension_kind!(arena, child, crate::caption::CaptionText) {
                caption_text_child = Some(child);
            } else {
                block_child = Some(child);
            }
        }
        // Render the block content, then a caption paragraph.
        if let Some(block) = block_child {
            self.render_block(out, context, block)?;
        }
        if let Some(ct) = caption_text_child {
            out.push_str("<p><em>");
            self.render_inline_children(out, context, ct)?;
            out.push_str("</em></p>\n");
        }
        Ok(())
    }

    pub(super) fn render_anchor_if_node_has_id(
        &self,
        out: &mut String,
        arena: &Arena,
        source: &str,
        node_ref: NodeRef,
    ) -> Result<()> {
        if let Some(id) = arena[node_ref]
            .attributes()
            .get("id")
            .map(|v| v.str(source))
        {
            write!(
                out,
                "<ac:structured-macro ac:name=\"anchor\"><ac:parameter ac:name=\"\">{}</ac:parameter></ac:structured-macro>",
                xml_escape(&id)
            )?;
        }
        Ok(())
    }
}
