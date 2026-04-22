use std::fmt::Write as _;

use anyhow::{bail, Result};
use rushdown::ast::{
    Arena, CodeBlockKind, KindData, LinkKind, NodeRef, TableCellAlignment, Task, TextQualifier,
};
use rushdown::{as_extension_data, as_kind_data};
use rushdown_definition_list::{DefinitionList, Term, TermDefinition};
use rushdown_fenced_div::FencedDiv;
use rushdown_footnote::{FootnoteDefinition, FootnoteReference};

type FootnoteLabels = Vec<(NodeRef, String)>;

pub(crate) const HARD_BREAK_SPACES_MARKER: &str = "@@FMT_HARD_BREAK_SPACE@@";

#[derive(Clone, Copy, Debug)]
struct TableEdgeStyle {
    leading_pipe: bool,
    trailing_pipe: bool,
}

pub(crate) struct MarkdownFmtRenderer {
    paragraph_width: usize,
    align_tables: bool,
}

impl MarkdownFmtRenderer {
    pub(crate) fn new(paragraph_width: usize, align_tables: bool) -> Self {
        Self {
            paragraph_width,
            align_tables,
        }
    }

    pub(crate) fn render_document(
        &self,
        arena: &Arena,
        doc_ref: NodeRef,
        source: &str,
    ) -> Result<String> {
        let mut out = String::new();
        let footnote_labels = collect_footnote_labels(arena, doc_ref, source)?;
        for child in arena[doc_ref].children(arena) {
            self.render_block(&mut out, arena, child, source, &footnote_labels)?;
        }
        Ok(out)
    }

    fn render_block(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnote_labels: &FootnoteLabels,
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
                out.push_str(&"#".repeat(heading.level() as usize));
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
                    self.render_block(&mut inner, arena, child, source, footnote_labels)?;
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
                let unordered_marker = if ordered { b'-' } else { list.marker() } as char;
                let mut index = list.start().max(1) as usize;
                for item in arena[node_ref].children(arena) {
                    let marker = if ordered {
                        format!("{}. ", index)
                    } else {
                        match arena[item].kind_data() {
                            KindData::ListItem(li) => match li.task() {
                                Some(Task::Checked) => format!("{unordered_marker} [x] "),
                                Some(Task::Unchecked) => format!("{unordered_marker} [ ] "),
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
                if self.align_tables {
                    self.render_table(out, arena, node_ref, source)?;
                } else {
                    self.render_table_unaligned(out, arena, node_ref, source)?;
                }
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
                write!(
                    out,
                    "[{}]: {}",
                    def.label().str(source),
                    def.destination_str(source)
                )?;
                if let Some(title) = def.title() {
                    write!(out, " \"{}\"", escape_md_title(&title.str(source)))?;
                }
                out.push_str("\n\n");
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, DefinitionList) =>
            {
                self.render_definition_list(out, arena, node_ref, source, footnote_labels)?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FencedDiv) =>
            {
                self.render_fenced_div(out, arena, node_ref, source, footnote_labels)?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, crate::caption::Caption) =>
            {
                self.render_caption(out, arena, node_ref, source, footnote_labels)?;
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
                self.render_footnote_definition(out, arena, node_ref, source, footnote_labels)?;
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

    fn render_footnote_definition(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnote_labels: &FootnoteLabels,
    ) -> Result<()> {
        let _footnote = as_extension_data!(arena, node_ref, FootnoteDefinition);
        let label = footnote_labels
            .iter()
            .find_map(|(nr, label)| (*nr == node_ref).then_some(label.as_str()))
            .unwrap_or("footnote");
        let inline_first_line = footnote_has_inline_body(arena, node_ref, source).unwrap_or(true);
        let mut body = String::new();
        for child in arena[node_ref].children(arena) {
            self.render_block(&mut body, arena, child, source, footnote_labels)?;
        }
        let trimmed = body.trim_end();

        if !inline_first_line {
            writeln!(out, "[^{label}]:")?;
            for line in trimmed.lines() {
                if line.is_empty() {
                    out.push('\n');
                } else {
                    writeln!(out, "    {line}")?;
                }
            }
            out.push('\n');
            return Ok(());
        }

        let mut first_line = true;
        for line in trimmed.lines() {
            if first_line {
                writeln!(out, "[^{label}]: {line}")?;
                first_line = false;
            } else if line.is_empty() {
                out.push('\n');
            } else {
                writeln!(out, "    {line}")?;
            }
        }
        if first_line {
            writeln!(out, "[^{label}]:")?;
        }
        out.push('\n');
        Ok(())
    }

    fn render_caption(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnote_labels: &FootnoteLabels,
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
                    self.render_block(out, arena, block, source, footnote_labels)?;
                }
            }
            crate::caption::CaptionPosition::After => {
                if let Some(block) = block_child {
                    self.render_block(out, arena, block, source, footnote_labels)?;
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
    ) -> Result<()> {
        out.push_str(&fenced_div_opening(arena, node_ref, source));
        out.push('\n');

        let children: Vec<NodeRef> = arena[node_ref].children(arena).collect();
        for (index, child) in children.iter().enumerate() {
            let mut rendered = String::new();
            self.render_block(&mut rendered, arena, *child, source, footnote_labels)?;
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

        out.push_str(":::\n\n");
        Ok(())
    }

    fn render_definition_list(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnote_labels: &FootnoteLabels,
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
                    self.render_block(&mut rendered, arena, *child, source, footnote_labels)?;
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

    fn render_table(
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
            for index in 0..columns {
                let align = aligns
                    .get(index)
                    .copied()
                    .unwrap_or(TableCellAlignment::None);
                separators.push(render_separator_cell(align, widths[index]));
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

    fn render_table_unaligned(
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

    fn render_inline_children(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        for child in arena[node_ref].children(arena) {
            self.render_inline(out, arena, child, source)?;
        }
        Ok(())
    }

    fn render_inline(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        match arena[node_ref].kind_data() {
            KindData::Text(text) => {
                out.push_str(text.str(source));
                if text.has_qualifiers(TextQualifier::HARD_LINE_BREAK) {
                    out.push_str(hard_line_break_marker(arena, node_ref, source));
                } else if text.has_qualifiers(TextQualifier::SOFT_LINE_BREAK) {
                    out.push(' ');
                }
            }
            KindData::CodeSpan(cs) => {
                let text = cs.value_str(source).to_string();
                let fence = backtick_fence(&text);
                let preserve_padding = code_span_has_explicit_padding(arena, node_ref, source);
                if preserve_padding || text.starts_with('`') || text.ends_with('`') {
                    write!(out, "{fence} {text} {fence}")?;
                } else {
                    write!(out, "{fence}{text}{fence}")?;
                }
            }
            KindData::Emphasis(emphasis) if emphasis.level() == 1 => {
                let marker = emphasis_marker_from_source(arena, node_ref, source);
                out.push(marker);
                self.render_inline_children(out, arena, node_ref, source)?;
                out.push(marker);
            }
            KindData::Emphasis(_) => {
                let marker = emphasis_marker_from_source(arena, node_ref, source);
                out.push_str(&marker.to_string().repeat(2));
                self.render_inline_children(out, arena, node_ref, source)?;
                out.push_str(&marker.to_string().repeat(2));
            }
            KindData::Strikethrough(_) => {
                out.push_str("~~");
                self.render_inline_children(out, arena, node_ref, source)?;
                out.push_str("~~");
            }
            KindData::Link(link) => match link.link_kind() {
                LinkKind::Auto(auto) => out.push_str(auto.text_str(source)),
                LinkKind::Reference(_) => {
                    if let Some(raw) = original_reference_link(arena, node_ref, source) {
                        out.push_str(&raw);
                    } else {
                        out.push('[');
                        self.render_inline_children(out, arena, node_ref, source)?;
                        out.push(']');
                    }
                }
                _ => {
                    out.push('[');
                    self.render_inline_children(out, arena, node_ref, source)?;
                    out.push(']');
                    out.push('(');
                    out.push_str(link.destination_str(source));
                    if let Some(title) = link.title() {
                        write!(out, " \"{}\"", escape_md_title(&title.str(source)))?;
                    }
                    out.push(')');
                }
            },
            KindData::Image(image) => {
                out.push_str("![");
                self.render_image_alt_text(out, arena, node_ref, source)?;
                out.push(']');
                out.push('(');
                out.push_str(image.destination_str(source));
                if let Some(title) = image.title() {
                    write!(out, " \"{}\"", escape_md_title(&title.str(source)))?;
                }
                out.push(')');
                if let Some(attrs) = render_node_attrs(arena, node_ref, source) {
                    out.push_str(&attrs);
                }
            }
            KindData::RawHtml(html) => out.push_str(&html.str(source)),
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FootnoteReference) =>
            {
                let footnote = as_extension_data!(arena, node_ref, FootnoteReference);
                write!(out, "[^{}]", footnote.label().str(source))?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::bracketed_span::BracketedSpan
                ) =>
            {
                out.push('[');
                self.render_inline_children(out, arena, node_ref, source)?;
                out.push(']');
                if let Some(attrs) = render_node_attrs(arena, node_ref, source) {
                    out.push_str(&attrs);
                }
            }
            KindData::Extension(_) => {}
            _ => {}
        }
        Ok(())
    }

    fn wrap_paragraph(&self, paragraph: &str) -> String {
        let text = paragraph.trim();
        if text.is_empty() {
            return String::new();
        }
        if text.contains('\n') {
            return text.to_string();
        }

        let mut out = Vec::new();
        let mut current = String::new();

        for word in text.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
                continue;
            }

            let next_len = current.chars().count() + 1 + word.chars().count();
            if next_len <= self.paragraph_width {
                current.push(' ');
                current.push_str(word);
            } else {
                out.push(current);
                current = word.to_string();
            }
        }

        if !current.is_empty() {
            out.push(current);
        }

        out.join("\n")
    }

    fn render_image_alt_text(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        for child in arena[node_ref].children(arena) {
            match arena[child].kind_data() {
                KindData::Text(text) => out.push_str(text.str(source)),
                KindData::CodeSpan(cs) => out.push_str(&cs.value_str(source)),
                _ => self.render_inline(out, arena, child, source)?,
            }
        }
        Ok(())
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
        TableCellAlignment::Center => format!(":{}:", "-".repeat(width.saturating_sub(2).max(1))),
        TableCellAlignment::Right => format!("{}:", "-".repeat(width.saturating_sub(1))),
        TableCellAlignment::None => "-".repeat(width),
        _ => "-".repeat(width),
    }
}

fn escape_md_title(title: &str) -> String {
    title.replace('\\', "\\\\").replace('"', "\\\"")
}

fn backtick_fence(content: &str) -> String {
    let mut max_run = 0usize;
    let mut current_run = 0usize;
    for c in content.chars() {
        if c == '`' {
            current_run += 1;
            if current_run > max_run {
                max_run = current_run;
            }
        } else {
            current_run = 0;
        }
    }
    "`".repeat(max_run + 1)
}

fn code_span_has_explicit_padding(arena: &Arena, node_ref: NodeRef, source: &str) -> bool {
    let Some(pos) = arena[node_ref].pos() else {
        return false;
    };
    let bytes = source.as_bytes();
    if pos >= bytes.len() || bytes[pos] != b'`' {
        return false;
    }

    let mut open_end = pos;
    while open_end < bytes.len() && bytes[open_end] == b'`' {
        open_end += 1;
    }
    let open_len = open_end - pos;
    if open_len == 0 {
        return false;
    }

    let Some(close_start) = find_backtick_closing_run(bytes, open_end, open_len) else {
        return false;
    };
    let raw_content = &source[open_end..close_start];
    raw_content.starts_with(' ')
        && raw_content.ends_with(' ')
        && raw_content.chars().any(|c| !c.is_whitespace())
}

fn find_backtick_closing_run(bytes: &[u8], mut from: usize, open_len: usize) -> Option<usize> {
    while from < bytes.len() {
        if bytes[from] != b'`' {
            from += 1;
            continue;
        }

        let mut run_end = from;
        while run_end < bytes.len() && bytes[run_end] == b'`' {
            run_end += 1;
        }
        let run_len = run_end - from;

        if run_len == open_len {
            let prev_is_backtick = from > 0 && bytes[from - 1] == b'`';
            let next_is_backtick = run_end < bytes.len() && bytes[run_end] == b'`';
            if !prev_is_backtick && !next_is_backtick {
                return Some(from);
            }
        }

        from = run_end;
    }

    None
}

fn render_node_attrs(arena: &Arena, node_ref: NodeRef, source: &str) -> Option<String> {
    let attrs = arena[node_ref].attributes();
    if attrs.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push('{');
    let mut first = true;

    if let Some(id) = attrs.get("id") {
        write!(out, "#{}", id.str(source)).ok()?;
        first = false;
    }
    if let Some(class) = attrs.get("class") {
        for cls in class.str(source).split_whitespace() {
            if !first {
                out.push(' ');
            }
            write!(out, ".{cls}").ok()?;
            first = false;
        }
    }

    for (key, value) in attrs.iter() {
        if key == "id" || key == "class" {
            continue;
        }
        if !first {
            out.push(' ');
        }
        write!(out, "{}=\"{}\"", key, value.str(source)).ok()?;
        first = false;
    }

    out.push('}');
    Some(out)
}

fn fenced_div_opening(arena: &Arena, node_ref: NodeRef, source: &str) -> String {
    if let Some(opening) = fenced_div_opening_from_source(arena, node_ref, source) {
        return opening;
    }

    let attrs = arena[node_ref].attributes();
    if attrs.is_empty() {
        return ":::".to_string();
    }

    let id = attrs.get("id").map(|v| v.str(source));
    let class = attrs
        .get("class")
        .map(|v| v.str(source))
        .unwrap_or_default();
    let class_items: Vec<&str> = class.split_whitespace().collect();
    let has_other_attrs = attrs.iter().any(|(k, _)| k != "id" && k != "class");

    if id.is_none() && !has_other_attrs && class_items.len() == 1 {
        return format!(":::{}", class_items[0]);
    }

    let attrs_text = render_node_attrs(arena, node_ref, source).unwrap_or_default();
    format!(":::{attrs_text}")
}

fn fenced_div_opening_from_source(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Option<String> {
    let pos = arena[node_ref].pos()?;
    let line = source_line_at_pos(source, pos)?;
    let trimmed = line.trim_start();
    trimmed.starts_with(":::").then(|| line.to_string())
}

fn opening_fence_from_source(arena: &Arena, node_ref: NodeRef, source: &str) -> Option<String> {
    let pos = arena[node_ref].pos()?;
    let line = source_tail_at_pos(source, pos)?;
    parse_opening_fence(line).map(|_| line.to_string())
}

fn verbatim_fenced_code_block_from_source(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Option<String> {
    let KindData::CodeBlock(code) = arena[node_ref].kind_data() else {
        return None;
    };
    if !matches!(code.code_block_kind(), CodeBlockKind::Fenced) {
        return None;
    }

    let pos = arena[node_ref].pos()?;
    let start = source[..pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let opening = source_line_at_pos(source, pos)?;
    let (_, marker, run) = parse_opening_fence_any_indent(opening)?;

    let mut line_start = start;
    while line_start < source.len() {
        let line_end = source[line_start..]
            .find('\n')
            .map(|idx| line_start + idx)
            .unwrap_or(source.len());
        let line = source[line_start..line_end].trim_end_matches('\r');
        if line_start > start && is_fence_close_any_indent(line, marker, run) {
            let block_end = if line_end < source.len() {
                line_end + 1
            } else {
                line_end
            };
            return Some(source[start..block_end].to_string());
        }
        if line_end == source.len() {
            break;
        }
        line_start = line_end + 1;
    }

    None
}

fn closing_fence_from_opening(opening: &str) -> Option<String> {
    let (indent, marker, run) = parse_opening_fence(opening)?;
    Some(format!(
        "{}{}",
        " ".repeat(indent),
        marker.to_string().repeat(run)
    ))
}

fn source_line_at_pos(source: &str, pos: usize) -> Option<&str> {
    if pos > source.len() {
        return None;
    }
    let start = source[..pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let end = source[start..]
        .find('\n')
        .map(|idx| start + idx)
        .unwrap_or(source.len());
    Some(source[start..end].trim_end_matches('\r'))
}

fn source_tail_at_pos(source: &str, pos: usize) -> Option<&str> {
    if pos > source.len() {
        return None;
    }
    let end = source[pos..]
        .find('\n')
        .map(|idx| pos + idx)
        .unwrap_or(source.len());
    Some(source[pos..end].trim_end_matches('\r'))
}

fn caption_prefix_from_source(
    arena: &Arena,
    node_ref: NodeRef,
    block_child: Option<NodeRef>,
    source: &str,
) -> Option<String> {
    let pos = block_child
        .and_then(|child| arena[child].pos())
        .or_else(|| arena[node_ref].pos())?;

    let start = source[..pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let preceding = source[..start].trim_end_matches(['\n', '\r']);
    let line_start = preceding.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let line = preceding.get(line_start..)?;
    let trimmed = line.trim_start();
    let colon = trimmed.find(':')?;
    let prefix = &trimmed[..=colon];
    match prefix.to_ascii_lowercase().as_str() {
        "table:" | "figure:" | "listing:" => Some(prefix.to_string()),
        _ => None,
    }
}

fn emphasis_marker_from_source(arena: &Arena, node_ref: NodeRef, source: &str) -> char {
    let Some(pos) = arena[node_ref].pos() else {
        return '*';
    };
    match source.as_bytes().get(pos).copied() {
        Some(b'_') => '_',
        Some(b'*') => '*',
        _ => '*',
    }
}

fn hard_line_break_marker(arena: &Arena, node_ref: NodeRef, source: &str) -> &'static str {
    let Some(pos) = arena[node_ref].pos() else {
        return "\\\n";
    };
    let Some(line) = source_line_at_pos(source, pos) else {
        return "\\\n";
    };
    if line.ends_with('\\') {
        "\\\n"
    } else if line.ends_with("  ") {
        "@@FMT_HARD_BREAK_SPACE@@\n"
    } else {
        "\\\n"
    }
}

fn original_reference_link(arena: &Arena, node_ref: NodeRef, source: &str) -> Option<String> {
    let start = arena[node_ref].pos()?;
    let first_end = parse_link_bracket(source, start)?;
    let rest = source.get(first_end..)?;

    if rest.starts_with('(') {
        return None;
    }

    if rest.starts_with('[') {
        let second_end = parse_link_bracket(source, first_end)?;
        return source.get(start..second_end).map(ToString::to_string);
    }

    source.get(start..first_end).map(ToString::to_string)
}

fn parse_link_bracket(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(start) != Some(&b'[') {
        return None;
    }

    let mut depth = 0usize;
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                index = (index + 2).min(bytes.len());
                continue;
            }
            b'[' => depth += 1,
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn parse_opening_fence(line: &str) -> Option<(usize, char, usize)> {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    if indent > 3 {
        return None;
    }
    let trimmed = &line[indent..];
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|c| *c == marker).count();
    (run >= 3).then_some((indent, marker, run))
}

fn parse_opening_fence_any_indent(line: &str) -> Option<(usize, char, usize)> {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    let trimmed = &line[indent..];
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|c| *c == marker).count();
    (run >= 3).then_some((indent, marker, run))
}

fn is_fence_close_any_indent(line: &str, marker: char, open_len: usize) -> bool {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    let trimmed = &line[indent..];
    let run = trimmed.chars().take_while(|c| *c == marker).count();
    if run < open_len {
        return false;
    }
    trimmed[run..].chars().all(|c| c == ' ' || c == '\t')
}

fn definition_prefix_from_source(arena: &Arena, node_ref: NodeRef, source: &str) -> Option<String> {
    let pos = arena[node_ref].pos()?;
    let tail = source_tail_at_pos(source, pos)?;
    let colon = tail.find(':')?;
    if colon > 3 {
        return None;
    }

    let whitespace_len = tail[colon + 1..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .count();
    if whitespace_len == 0 {
        return Some(tail[..=colon].to_string());
    }

    Some(tail[..colon + 1 + whitespace_len].to_string())
}

fn definition_child_indent_from_source(
    arena: &Arena,
    def_ref: NodeRef,
    child_ref: NodeRef,
    source: &str,
) -> Option<String> {
    let def_pos = arena[def_ref].pos()?;
    let def_line_start = source[..def_pos]
        .rfind('\n')
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let def_indent = source[def_line_start..def_pos].chars().count();

    let child_pos = arena[child_ref].pos()?;
    let child_line = source_line_at_pos(source, child_pos)?;
    let child_indent = child_line.chars().take_while(|c| *c == ' ').count();

    Some(" ".repeat(child_indent.saturating_sub(def_indent)))
}

fn footnote_has_inline_body(arena: &Arena, node_ref: NodeRef, source: &str) -> Option<bool> {
    let pos = arena[node_ref].pos()?;
    let tail = source_tail_at_pos(source, pos)?;
    let colon = tail.find(':')?;
    Some(!tail[colon + 1..].trim().is_empty())
}

fn collect_footnote_labels(
    arena: &Arena,
    doc_ref: NodeRef,
    source: &str,
) -> Result<FootnoteLabels> {
    let mut nodes = Vec::new();
    collect_footnote_nodes(arena, doc_ref, &mut nodes);

    let mut map = FootnoteLabels::new();
    for node_ref in nodes {
        let Some(label) = footnote_label_from_source_at_node(arena, node_ref, source) else {
            let pos = arena[node_ref].pos().unwrap_or_default();
            bail!("failed to read footnote label from source at byte offset {pos}");
        };
        map.push((node_ref, label));
    }
    Ok(map)
}

fn footnote_label_from_source_at_node(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Option<String> {
    let pos = arena[node_ref].pos()?;
    let tail = source_tail_at_pos(source, pos)?;
    parse_footnote_label_from_line(tail)
}

fn parse_footnote_label_from_line(line: &str) -> Option<String> {
    let line = line.trim_start_matches([' ', '\t']);
    let rest = line.strip_prefix("[^")?;
    let end = rest.find(']')?;
    if end == 0 {
        return None;
    }
    let label = &rest[..end];
    let tail = rest[end + 1..].trim_start_matches([' ', '\t']);
    if !tail.starts_with(':') {
        return None;
    }
    Some(label.to_string())
}

fn collect_footnote_nodes(arena: &Arena, node_ref: NodeRef, out: &mut Vec<NodeRef>) {
    if rushdown::matches_extension_kind!(arena, node_ref, FootnoteDefinition) {
        out.push(node_ref);
        return;
    }
    for child in arena[node_ref].children(arena) {
        collect_footnote_nodes(arena, child, out);
    }
}
