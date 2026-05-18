use std::fmt::Write as _;

use anyhow::Result;
use rushdown::as_extension_data;
use rushdown::ast::{Arena, CodeBlockKind, KindData, NodeRef, Task};
use rushdown_definition_list::{DefinitionList, Term, TermDefinition};
use rushdown_fenced_div::FencedDiv;
use rushdown_footnote::FootnoteDefinition;

use super::source_recovery::{
    caption_prefix_from_source, closing_fence_from_opening, definition_child_indent_from_source,
    definition_prefix_from_source, fenced_div_closing_and_trailing_blank_lines, fenced_div_opening,
    opening_fence_from_source, render_node_attrs, verbatim_fenced_code_block_from_source,
};
use super::{FootnoteLabels, MarkdownFmtRenderer, PRESERVE_BLANK_RUN_MARKER};

impl MarkdownFmtRenderer {
    pub(super) fn render_block(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnote_labels: &FootnoteLabels,
        heading_shift: usize,
    ) -> Result<()> {
        match arena[node_ref].kind_data() {
            KindData::Paragraph(_) => {
                let mut inline = String::new();
                self.render_inline_children(&mut inline, arena, node_ref, source)?;
                let paragraph = self.wrap_paragraph(&inline);
                out.push_str(&paragraph);
                out.push_str("\n\n");
            }
            KindData::Heading(heading) => {
                let level = (heading.level() as usize + heading_shift).min(6);
                out.push_str(&"#".repeat(level));
                out.push(' ');
                self.render_inline_children(out, arena, node_ref, source)?;
                if let Some(attrs) = render_node_attrs(arena, node_ref, source) {
                    out.push(' ');
                    out.push_str(&attrs);
                }
                out.push_str("\n\n");
            }
            KindData::ThematicBreak(_) => out.push_str("---\n\n"),
            KindData::CodeBlock(code) => match code.code_block_kind() {
                CodeBlockKind::Indented => {
                    for line in code.value().iter(source) {
                        let raw = line.as_ref();
                        let raw = raw
                            .strip_suffix("\r\n")
                            .or_else(|| raw.strip_suffix('\n'))
                            .unwrap_or(raw);
                        out.push_str("    ");
                        out.push_str(raw);
                        out.push('\n');
                    }
                    out.push('\n');
                }
                CodeBlockKind::Fenced => {
                    let opening = opening_fence_from_source(arena, node_ref, source)
                        .unwrap_or_else(|| {
                            let mut line = String::from("```");
                            line.push_str(code.info_str(source).unwrap_or(""));
                            line
                        });
                    let closing =
                        closing_fence_from_opening(&opening).unwrap_or_else(|| "```".to_string());

                    out.push_str(&opening);
                    out.push('\n');
                    for line in code.value().iter(source) {
                        out.push_str(line.as_ref());
                    }
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&closing);
                    out.push_str("\n\n");
                }
                _ => {
                    out.push_str("```");
                    out.push_str(code.info_str(source).unwrap_or(""));
                    out.push('\n');
                    for line in code.value().iter(source) {
                        out.push_str(line.as_ref());
                    }
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("```\n\n");
                }
            },
            KindData::Blockquote(_) => {
                let mut inner = String::new();
                for child in arena[node_ref].children(arena) {
                    self.render_block(
                        &mut inner,
                        arena,
                        child,
                        source,
                        footnote_labels,
                        heading_shift,
                    )?;
                }
                let inner = inner.trim_end_matches('\n');
                for line in inner.lines() {
                    if line.is_empty() {
                        out.push_str(">\n");
                    } else {
                        out.push_str("> ");
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                out.push('\n');
            }
            KindData::List(list) => {
                let tight = list.is_tight();
                let ordered = list.is_ordered();
                let list_marker = list.marker() as char;
                let unordered_marker = if ordered { '-' } else { list_marker };
                let ordered_marker = list_marker;
                let mut index = list.start().max(1) as usize;
                for item in arena[node_ref].children(arena) {
                    let marker = if ordered {
                        format!("{index}{ordered_marker} ")
                    } else {
                        match arena[item].kind_data() {
                            KindData::ListItem(li) => match li.task() {
                                Some(Task::Completed) => format!("{unordered_marker} [x] "),
                                Some(Task::Active) => format!("{unordered_marker} [ ] "),
                                _ => format!("{unordered_marker} "),
                            },
                            _ => format!("{unordered_marker} "),
                        }
                    };
                    let indent = " ".repeat(marker.len());
                    let children: Vec<NodeRef> = arena[item].children(arena).collect();
                    let mut first_output_line = true;
                    for (child_index, child) in children.iter().enumerate() {
                        let mut rendered = if !tight {
                            verbatim_fenced_code_block_from_source(arena, *child, source)
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };
                        let verbatim = !rendered.is_empty();
                        if !verbatim {
                            if tight && matches!(arena[*child].kind_data(), KindData::Paragraph(_))
                            {
                                self.render_inline_children(&mut rendered, arena, *child, source)?;
                                rendered = self.wrap_paragraph(&rendered);
                            } else {
                                self.render_block(
                                    &mut rendered,
                                    arena,
                                    *child,
                                    source,
                                    footnote_labels,
                                    heading_shift,
                                )?;
                            }
                        }
                        let rendered = rendered.trim_end_matches('\n');

                        for line in rendered.lines() {
                            if first_output_line {
                                out.push_str(&marker);
                                out.push_str(line);
                                out.push('\n');
                                first_output_line = false;
                            } else if line.is_empty() {
                                out.push('\n');
                            } else if verbatim {
                                out.push_str(line);
                                out.push('\n');
                            } else {
                                out.push_str(&indent);
                                out.push_str(line);
                                out.push('\n');
                            }
                        }

                        if !tight && child_index + 1 < children.len() {
                            out.push('\n');
                        }
                    }
                    if !tight {
                        out.push('\n');
                    }
                    index += 1;
                }
                if tight {
                    out.push('\n');
                }
            }
            KindData::Table(_) => {
                self.render_any_table(out, arena, node_ref, source)?;
            }
            KindData::HtmlBlock(html) => {
                for line in html.value().iter(source) {
                    out.push_str(line.as_ref());
                }
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            KindData::LinkReferenceDefinition(def) => {
                if self.suppress_link_reference_definitions {
                    return Ok(());
                }
                write!(
                    out,
                    "[{}]: {}",
                    def.label().str(source),
                    def.destination_str(source)
                )?;
                if let Some(title) = def.title() {
                    write!(
                        out,
                        " \"{}\"",
                        super::source_recovery::escape_md_title(&title.str(source))
                    )?;
                }
                out.push_str("\n\n");
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, DefinitionList) =>
            {
                self.render_definition_list(
                    out,
                    arena,
                    node_ref,
                    source,
                    footnote_labels,
                    heading_shift,
                )?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FencedDiv) =>
            {
                if let Some(profile) = self.graph_profile.as_ref() {
                    if !crate::conditional::fenced_div_is_visible(arena, node_ref, source, profile)?
                    {
                        return Ok(());
                    }
                }
                if self.suppress_include_fenced_divs {
                    let classes = arena[node_ref]
                        .attributes()
                        .get("class")
                        .map(|value| value.str(source))
                        .unwrap_or_default();
                    if classes
                        .split_whitespace()
                        .any(|class_name| class_name == "include")
                    {
                        return Ok(());
                    }
                }
                self.render_fenced_div(
                    out,
                    arena,
                    node_ref,
                    source,
                    footnote_labels,
                    heading_shift,
                )?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, crate::caption::Caption) =>
            {
                self.render_caption(out, arena, node_ref, source, footnote_labels, heading_shift)?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::caption::CaptionText
                ) =>
            {
                self.render_inline_children(out, arena, node_ref, source)?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, crate::diagram::Diagram) =>
            {
                self.render_diagram_block(out, arena, node_ref, source)?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FootnoteDefinition) =>
            {
                if self.suppress_footnote_definitions {
                    return Ok(());
                }
                self.render_footnote_definition(
                    out,
                    arena,
                    node_ref,
                    source,
                    footnote_labels,
                    heading_shift,
                )?;
            }
            KindData::Extension(_) => {}
            _ => {}
        }
        Ok(())
    }

    fn render_diagram_block(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        let diagram = as_extension_data!(arena, node_ref, crate::diagram::Diagram);
        let opening = opening_fence_from_source(arena, node_ref, source).unwrap_or_else(|| {
            let lang = match diagram.diagram_type() {
                crate::diagram::DiagramType::Mermaid => "mermaid",
                crate::diagram::DiagramType::PlantUml => "plantuml",
                crate::diagram::DiagramType::Typst => "typst",
                crate::diagram::DiagramType::DrawIo => "drawio",
                crate::diagram::DiagramType::D2 => "d2",
            };
            let mut line = String::from("```");
            line.push_str(lang);
            line
        });
        let closing = closing_fence_from_opening(&opening).unwrap_or_else(|| "```".to_string());

        out.push_str(&opening);
        out.push('\n');
        let body = crate::diagram::collect_diagram_source(diagram, source);
        out.push_str(&body);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&closing);
        out.push_str("\n\n");
        Ok(())
    }

    fn render_caption(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnote_labels: &FootnoteLabels,
        heading_shift: usize,
    ) -> Result<()> {
        let caption = as_extension_data!(arena, node_ref, crate::caption::Caption);

        let mut block_child = None;
        let mut caption_text_child = None;
        for child in arena[node_ref].children(arena) {
            if rushdown::matches_extension_kind!(arena, child, crate::caption::CaptionText) {
                caption_text_child = Some(child);
            } else {
                block_child = Some(child);
            }
        }

        let prefix = caption_prefix_from_source(arena, node_ref, block_child, source)
            .unwrap_or_else(|| match caption.kind() {
                crate::caption::CaptionKind::Table => "Table:".to_string(),
                crate::caption::CaptionKind::Figure => "Figure:".to_string(),
                crate::caption::CaptionKind::Listing => "Listing:".to_string(),
            });

        let mut caption_line = String::new();
        caption_line.push_str(&prefix);
        if let Some(ct) = caption_text_child {
            let mut caption_text = String::new();
            self.render_inline_children(&mut caption_text, arena, ct, source)?;
            let caption_text = caption_text.trim_start();
            if !caption_text.is_empty() {
                caption_line.push(' ');
                caption_line.push_str(caption_text);
            }
        }
        if let Some(attrs) = render_node_attrs(arena, node_ref, source) {
            if !caption_line.ends_with(' ') {
                caption_line.push(' ');
            }
            caption_line.push_str(&attrs);
        }

        match caption.position() {
            crate::caption::CaptionPosition::Before => {
                out.push_str(caption_line.trim_end());
                out.push_str("\n\n");
                if let Some(block) = block_child {
                    self.render_block(out, arena, block, source, footnote_labels, heading_shift)?;
                }
            }
            crate::caption::CaptionPosition::After => {
                if let Some(block) = block_child {
                    self.render_block(out, arena, block, source, footnote_labels, heading_shift)?;
                }
                out.push_str(caption_line.trim_end());
                out.push_str("\n\n");
            }
        }
        Ok(())
    }

    fn render_fenced_div(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnote_labels: &FootnoteLabels,
        heading_shift: usize,
    ) -> Result<()> {
        let opening = fenced_div_opening(arena, node_ref, source);
        out.push_str(&opening);
        out.push('\n');

        let children: Vec<NodeRef> = arena[node_ref].children(arena).collect();
        for (index, child) in children.iter().enumerate() {
            let mut rendered = String::new();
            self.render_block(
                &mut rendered,
                arena,
                *child,
                source,
                footnote_labels,
                heading_shift,
            )?;
            out.push_str(rendered.trim_end_matches('\n'));

            if index + 1 < children.len() {
                let next = children[index + 1];
                let separator = if matches!(arena[*child].kind_data(), KindData::Heading(_))
                    && matches!(arena[next].kind_data(), KindData::Heading(_))
                {
                    "\n"
                } else {
                    "\n\n"
                };
                out.push_str(separator);
            } else {
                out.push('\n');
            }
        }

        let (closing, trailing_blank_lines) = fenced_div_closing_and_trailing_blank_lines(
            arena, node_ref, source,
        )
        .unwrap_or_else(|| {
            let closing = closing_fence_from_opening(&opening).unwrap_or_else(|| ":::".to_string());
            (closing, 1)
        });

        out.push_str(&closing);
        out.push('\n');
        out.push_str(PRESERVE_BLANK_RUN_MARKER);
        out.push('\n');
        for _ in 0..trailing_blank_lines.max(1) {
            out.push('\n');
        }
        Ok(())
    }

    fn render_definition_list(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnote_labels: &FootnoteLabels,
        heading_shift: usize,
    ) -> Result<()> {
        let tight = as_extension_data!(arena, node_ref, DefinitionList).is_tight();
        let children: Vec<NodeRef> = arena[node_ref].children(arena).collect();
        let mut index = 0usize;

        while index < children.len() {
            if !rushdown::matches_extension_kind!(arena, children[index], Term) {
                index += 1;
                continue;
            }
            let term_ref = children[index];
            index += 1;

            let mut definitions = Vec::new();
            while index < children.len()
                && rushdown::matches_extension_kind!(arena, children[index], TermDefinition)
            {
                definitions.push(children[index]);
                index += 1;
            }

            self.render_inline_children(out, arena, term_ref, source)?;
            out.push('\n');

            if !tight && !definitions.is_empty() {
                out.push('\n');
            }

            for def_ref in definitions {
                let definition_prefix = definition_prefix_from_source(arena, def_ref, source)
                    .unwrap_or_else(|| ":  ".to_string());
                let wrapped_indent = " ".repeat(definition_prefix.chars().count());
                let children: Vec<NodeRef> = arena[def_ref].children(arena).collect();
                let Some((first_child, rest)) = children.split_first() else {
                    out.push_str(":\n");
                    continue;
                };

                let mut first_rendered = String::new();
                self.render_block(
                    &mut first_rendered,
                    arena,
                    *first_child,
                    source,
                    footnote_labels,
                    heading_shift,
                )?;
                let first_rendered = first_rendered.trim_end_matches('\n');
                let mut first_line = true;
                for line in first_rendered.lines() {
                    if first_line {
                        out.push_str(&definition_prefix);
                        out.push_str(line);
                        out.push('\n');
                        first_line = false;
                    } else if line.is_empty() {
                        out.push('\n');
                    } else {
                        out.push_str(&wrapped_indent);
                        out.push_str(line);
                        out.push('\n');
                    }
                }

                for child in rest {
                    out.push('\n');
                    let mut rendered = String::new();
                    self.render_block(
                        &mut rendered,
                        arena,
                        *child,
                        source,
                        footnote_labels,
                        heading_shift,
                    )?;
                    let rendered = rendered.trim_end_matches('\n');
                    let child_indent =
                        definition_child_indent_from_source(arena, def_ref, *child, source)
                            .unwrap_or_else(|| wrapped_indent.clone());
                    for line in rendered.lines() {
                        if line.is_empty() {
                            out.push('\n');
                        } else {
                            out.push_str(&child_indent);
                            out.push_str(line);
                            out.push('\n');
                        }
                    }
                }
            }
            out.push('\n');
        }

        Ok(())
    }
}
