use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use rushdown::as_extension_data;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, KindData, LinkKind, NodeRef, TableCellAlignment, TextQualifier};
use rushdown_definition_list::{DefinitionList, Term, TermDefinition};
use rushdown_fenced_div::FencedDiv;
use rushdown_footnote::{FootnoteDefinition, FootnoteReference};

use crate::graph::PreparedDocument;

pub struct MarkdownRenderer<'a> {
    base_dir: &'a Path,
    output_dir: &'a Path,
}

struct RenderScope {
    doc_id: usize,
}

/// Collected footnote for deferred rendering at document end.
#[derive(Clone)]
struct FootnoteEntry {
    label: String,
    doc_id: usize,
    node_ref: NodeRef,
}

impl<'a> MarkdownRenderer<'a> {
    pub fn new(base_dir: &'a Path, output_dir: &'a Path) -> Self {
        Self {
            base_dir,
            output_dir,
        }
    }

    pub fn render_document(&self, prepared: &PreparedDocument) -> Result<String> {
        let mut out = String::new();
        let mut footnotes = Vec::new();
        let mut seen_footnotes = HashSet::<(usize, String)>::new();
        self.render_graph_document(
            &mut out,
            prepared,
            prepared.graph().root_id,
            0,
            &mut footnotes,
            &mut seen_footnotes,
        )?;
        self.render_footnote_definitions(&mut out, prepared, &footnotes)?;
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_graph_document(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        doc_id: usize,
        heading_shift: usize,
        footnotes: &mut Vec<FootnoteEntry>,
        seen_footnotes: &mut HashSet<(usize, String)>,
    ) -> Result<()> {
        let doc = prepared.graph().document(doc_id);
        for child in doc.arena[doc.doc_ref].children(&doc.arena) {
            if let Some(entries) = prepared.graph().include_entries(doc_id, child) {
                for entry in entries {
                    self.render_graph_document(
                        out,
                        prepared,
                        entry.doc_id,
                        heading_shift + entry.heading_shift,
                        footnotes,
                        seen_footnotes,
                    )?;
                }
                continue;
            }
            self.render_block(
                out,
                prepared,
                &RenderScope { doc_id },
                &doc.arena,
                child,
                &doc.source,
                heading_shift,
                footnotes,
                seen_footnotes,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_block(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &RenderScope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        heading_shift: usize,
        footnotes: &mut Vec<FootnoteEntry>,
        seen_footnotes: &mut HashSet<(usize, String)>,
    ) -> Result<()> {
        match arena[node_ref].kind_data() {
            KindData::Paragraph(_) => {
                self.render_inline_children(
                    out,
                    prepared,
                    scope,
                    arena,
                    node_ref,
                    source,
                    footnotes,
                    seen_footnotes,
                )?;
                out.push_str("\n\n");
            }
            KindData::Heading(heading) => {
                let level = (heading.level() as usize + heading_shift).min(6);
                out.push_str(&"#".repeat(level));
                out.push(' ');
                self.render_inline_children(
                    out,
                    prepared,
                    scope,
                    arena,
                    node_ref,
                    source,
                    footnotes,
                    seen_footnotes,
                )?;
                if let Some(id) = arena[node_ref].attributes().get("id") {
                    write!(out, " {{#{}}}", id.str(source))?;
                }
                out.push_str("\n\n");
            }
            KindData::ThematicBreak(_) => out.push_str("---\n\n"),
            KindData::CodeBlock(code) => {
                let lang = code.language_str(source).unwrap_or("");
                out.push_str("```");
                out.push_str(lang);
                let attrs = arena[node_ref].attributes();
                if !attrs.is_empty() {
                    out.push_str(" {");
                    let mut first = true;
                    if let Some(id) = attrs.get("id") {
                        write!(out, "#{}", id.str(source))?;
                        first = false;
                    }
                    if let Some(class) = attrs.get("class") {
                        for cls in class.str(source).split_whitespace() {
                            if !first {
                                out.push(' ');
                            }
                            write!(out, ".{cls}")?;
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
                        write!(out, "{}=\"{}\"", key, value.str(source))?;
                        first = false;
                    }
                    out.push('}');
                }
                out.push('\n');
                for line in code.value().iter(source) {
                    out.push_str(line.as_ref());
                }
                // Code value lines from rushdown include trailing newlines,
                // but ensure the closing fence is on its own line.
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```\n\n");
            }
            KindData::Blockquote(_) => {
                let mut inner = String::new();
                for child in arena[node_ref].children(arena) {
                    self.render_block(
                        &mut inner,
                        prepared,
                        scope,
                        arena,
                        child,
                        source,
                        heading_shift,
                        footnotes,
                        seen_footnotes,
                    )?;
                }
                // Prefix each line with `> `.
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
                let ordered = list.is_ordered();
                let mut index = list.start().max(1) as usize;
                for item in arena[node_ref].children(arena) {
                    let marker = if ordered {
                        format!("{}. ", index)
                    } else {
                        "- ".to_string()
                    };
                    let indent = " ".repeat(marker.len());
                    let mut item_content = String::new();
                    for child in arena[item].children(arena) {
                        self.render_block(
                            &mut item_content,
                            prepared,
                            scope,
                            arena,
                            child,
                            source,
                            heading_shift,
                            footnotes,
                            seen_footnotes,
                        )?;
                    }
                    let mut first_line = true;
                    for line in item_content.lines() {
                        if first_line {
                            out.push_str(&marker);
                            out.push_str(line);
                            out.push('\n');
                            first_line = false;
                        } else if line.is_empty() {
                            out.push('\n');
                        } else {
                            out.push_str(&indent);
                            out.push_str(line);
                            out.push('\n');
                        }
                    }
                    index += 1;
                }
                out.push('\n');
            }
            KindData::Table(_) => {
                self.render_table(
                    out,
                    prepared,
                    scope,
                    arena,
                    node_ref,
                    source,
                    footnotes,
                    seen_footnotes,
                )?;
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
            KindData::LinkReferenceDefinition(_) => {
                // Links are already resolved; skip definitions.
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, DefinitionList) =>
            {
                self.render_definition_list(
                    out,
                    prepared,
                    scope,
                    arena,
                    node_ref,
                    source,
                    heading_shift,
                    footnotes,
                    seen_footnotes,
                )?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FencedDiv) =>
            {
                // Skip include divs — they are handled by the graph.
                let classes = arena[node_ref]
                    .attributes()
                    .get("class")
                    .map(|value| value.str(source))
                    .unwrap_or_default();
                if classes.split_whitespace().any(|c| c == "include") {
                    return Ok(());
                }
                // Format-gating: if the div has classes matching known output
                // format names, only render when "markdown" is among them.
                let present_formats: Vec<&str> = classes
                    .split_whitespace()
                    .filter(|c| crate::KNOWN_OUTPUT_FORMATS.contains(c))
                    .collect();
                if !present_formats.is_empty() && !present_formats.contains(&"markdown") {
                    return Ok(());
                }
                for child in arena[node_ref].children(arena) {
                    self.render_block(
                        out,
                        prepared,
                        scope,
                        arena,
                        child,
                        source,
                        heading_shift,
                        footnotes,
                        seen_footnotes,
                    )?;
                }
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, crate::caption::Caption) =>
            {
                self.render_caption(
                    out,
                    prepared,
                    scope,
                    arena,
                    node_ref,
                    source,
                    heading_shift,
                    footnotes,
                    seen_footnotes,
                )?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::caption::CaptionText
                ) =>
            {
                // CaptionText rendered as part of Caption; standalone: inline children.
                self.render_inline_children(
                    out,
                    prepared,
                    scope,
                    arena,
                    node_ref,
                    source,
                    footnotes,
                    seen_footnotes,
                )?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FootnoteDefinition) =>
            {
                // Suppressed — collected and rendered at document end.
            }
            KindData::Extension(_) => {}
            _ => {}
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_caption(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &RenderScope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        heading_shift: usize,
        footnotes: &mut Vec<FootnoteEntry>,
        seen_footnotes: &mut HashSet<(usize, String)>,
    ) -> Result<()> {
        let caption = as_extension_data!(arena, node_ref, crate::caption::Caption);
        let prefix = match caption.kind() {
            crate::caption::CaptionKind::Table => "Table: ",
            crate::caption::CaptionKind::Figure => "Figure: ",
        };

        // Find the CaptionText child and the captioned block child.
        let mut block_child = None;
        let mut caption_text_child = None;
        for child in arena[node_ref].children(arena) {
            if rushdown::matches_extension_kind!(arena, child, crate::caption::CaptionText) {
                caption_text_child = Some(child);
            } else {
                block_child = Some(child);
            }
        }

        let position = caption.position();

        // Emit in source order: caption before or after block.
        match position {
            crate::caption::CaptionPosition::Before => {
                // Caption text first.
                out.push_str(prefix);
                if let Some(ct) = caption_text_child {
                    self.render_inline_children(
                        out,
                        prepared,
                        scope,
                        arena,
                        ct,
                        source,
                        footnotes,
                        seen_footnotes,
                    )?;
                }
                if let Some(id) = arena[node_ref].attributes().get("id") {
                    write!(out, " {{#{}}}", id.str(source))?;
                }
                out.push_str("\n\n");
                // Then the block.
                if let Some(block) = block_child {
                    self.render_block(
                        out,
                        prepared,
                        scope,
                        arena,
                        block,
                        source,
                        heading_shift,
                        footnotes,
                        seen_footnotes,
                    )?;
                }
            }
            crate::caption::CaptionPosition::After => {
                // Block first.
                if let Some(block) = block_child {
                    self.render_block(
                        out,
                        prepared,
                        scope,
                        arena,
                        block,
                        source,
                        heading_shift,
                        footnotes,
                        seen_footnotes,
                    )?;
                }
                // Then caption text.
                out.push_str(prefix);
                if let Some(ct) = caption_text_child {
                    self.render_inline_children(
                        out,
                        prepared,
                        scope,
                        arena,
                        ct,
                        source,
                        footnotes,
                        seen_footnotes,
                    )?;
                }
                if let Some(id) = arena[node_ref].attributes().get("id") {
                    write!(out, " {{#{}}}", id.str(source))?;
                }
                out.push_str("\n\n");
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_definition_list(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &RenderScope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        heading_shift: usize,
        footnotes: &mut Vec<FootnoteEntry>,
        seen_footnotes: &mut HashSet<(usize, String)>,
    ) -> Result<()> {
        let children = arena[node_ref].children(arena).collect::<Vec<_>>();
        let mut index = 0usize;
        while index < children.len() {
            // Collect consecutive terms.
            let mut terms = Vec::new();
            while index < children.len()
                && rushdown::matches_extension_kind!(arena, children[index], Term)
            {
                terms.push(children[index]);
                index += 1;
            }

            // Collect consecutive definitions.
            let mut definitions = Vec::new();
            while index < children.len()
                && rushdown::matches_extension_kind!(arena, children[index], TermDefinition)
            {
                definitions.push(children[index]);
                index += 1;
            }

            // Emit each term.
            for term_ref in &terms {
                self.render_inline_children(
                    out,
                    prepared,
                    scope,
                    arena,
                    *term_ref,
                    source,
                    footnotes,
                    seen_footnotes,
                )?;
                out.push('\n');
            }

            // Emit each definition prefixed with `:`.
            for def_ref in &definitions {
                for child in arena[*def_ref].children(arena) {
                    let mut inner = String::new();
                    self.render_block(
                        &mut inner,
                        prepared,
                        scope,
                        arena,
                        child,
                        source,
                        heading_shift,
                        footnotes,
                        seen_footnotes,
                    )?;
                    let trimmed = inner.trim_end();
                    let mut first_line = true;
                    for line in trimmed.lines() {
                        if first_line {
                            out.push_str(": ");
                            out.push_str(line);
                            out.push('\n');
                            first_line = false;
                        } else if line.is_empty() {
                            out.push('\n');
                        } else {
                            out.push_str("  ");
                            out.push_str(line);
                            out.push('\n');
                        }
                    }
                    if first_line {
                        // Empty definition.
                        out.push_str(": \n");
                    }
                }
            }
            out.push('\n');
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_table(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &RenderScope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnotes: &mut Vec<FootnoteEntry>,
        seen_footnotes: &mut HashSet<(usize, String)>,
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

        // Collect alignments from first header row (or first body row).
        if let Some(first_row) = header_rows.first().or(body_rows.first()) {
            aligns = arena[*first_row]
                .children(arena)
                .map(|cell| as_kind_data!(arena, cell, TableCell).alignment())
                .collect();
        }

        // Render header rows.
        for row in &header_rows {
            self.render_table_row(
                out,
                prepared,
                scope,
                arena,
                *row,
                source,
                footnotes,
                seen_footnotes,
            )?;
        }

        // Separator row.
        if !header_rows.is_empty() || !aligns.is_empty() {
            out.push('|');
            for align in &aligns {
                match align {
                    TableCellAlignment::Left | TableCellAlignment::None => out.push_str(" --- |"),
                    TableCellAlignment::Center => out.push_str(" :---: |"),
                    TableCellAlignment::Right => out.push_str(" ---: |"),
                    _ => out.push_str(" --- |"),
                }
            }
            out.push('\n');
        }

        // Body rows.
        for row in &body_rows {
            self.render_table_row(
                out,
                prepared,
                scope,
                arena,
                *row,
                source,
                footnotes,
                seen_footnotes,
            )?;
        }

        out.push('\n');
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_table_row(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &RenderScope,
        arena: &Arena,
        row_ref: NodeRef,
        source: &str,
        footnotes: &mut Vec<FootnoteEntry>,
        seen_footnotes: &mut HashSet<(usize, String)>,
    ) -> Result<()> {
        out.push('|');
        for cell in arena[row_ref].children(arena) {
            out.push(' ');
            self.render_inline_children(
                out,
                prepared,
                scope,
                arena,
                cell,
                source,
                footnotes,
                seen_footnotes,
            )?;
            out.push_str(" |");
        }
        out.push('\n');
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_inline_children(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &RenderScope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnotes: &mut Vec<FootnoteEntry>,
        seen_footnotes: &mut HashSet<(usize, String)>,
    ) -> Result<()> {
        for child in arena[node_ref].children(arena) {
            self.render_inline(
                out,
                prepared,
                scope,
                arena,
                child,
                source,
                footnotes,
                seen_footnotes,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_inline(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &RenderScope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnotes: &mut Vec<FootnoteEntry>,
        seen_footnotes: &mut HashSet<(usize, String)>,
    ) -> Result<()> {
        match arena[node_ref].kind_data() {
            KindData::Text(text) => {
                out.push_str(text.str(source));
                if text.has_qualifiers(TextQualifier::HARD_LINE_BREAK) {
                    out.push_str("  \n");
                } else if text.has_qualifiers(TextQualifier::SOFT_LINE_BREAK) {
                    out.push('\n');
                }
            }
            KindData::CodeSpan(_) => {
                let mut text = String::new();
                for child in arena[node_ref].children(arena) {
                    if let KindData::Text(value) = arena[child].kind_data() {
                        text.push_str(value.str(source));
                    }
                }
                // Choose backtick fence that doesn't conflict with content.
                let fence = backtick_fence(&text);
                if text.starts_with('`') || text.ends_with('`') {
                    write!(out, "{fence} {text} {fence}")?;
                } else {
                    write!(out, "{fence}{text}{fence}")?;
                }
            }
            KindData::Emphasis(emphasis) if emphasis.level() == 1 => {
                out.push('*');
                self.render_inline_children(
                    out,
                    prepared,
                    scope,
                    arena,
                    node_ref,
                    source,
                    footnotes,
                    seen_footnotes,
                )?;
                out.push('*');
            }
            KindData::Emphasis(_) => {
                out.push_str("**");
                self.render_inline_children(
                    out,
                    prepared,
                    scope,
                    arena,
                    node_ref,
                    source,
                    footnotes,
                    seen_footnotes,
                )?;
                out.push_str("**");
            }
            KindData::Strikethrough(_) => {
                out.push_str("~~");
                self.render_inline_children(
                    out,
                    prepared,
                    scope,
                    arena,
                    node_ref,
                    source,
                    footnotes,
                    seen_footnotes,
                )?;
                out.push_str("~~");
            }
            KindData::Link(link) => {
                let dest = prepared
                    .graph()
                    .link_target(scope.doc_id, node_ref)
                    .ok_or_else(|| {
                        anyhow!(
                            "missing resolved link target for `{}` in {}",
                            link.destination_str(source),
                            prepared.graph().document(scope.doc_id).path.display()
                        )
                    })?
                    .to_string();
                match link.link_kind() {
                    LinkKind::Auto(_) => {
                        write!(out, "<{dest}>")?;
                    }
                    _ => {
                        let rewritten = self.rewrite_link_for_markdown(&dest);
                        out.push('[');
                        self.render_inline_children(
                            out,
                            prepared,
                            scope,
                            arena,
                            node_ref,
                            source,
                            footnotes,
                            seen_footnotes,
                        )?;
                        out.push(']');
                        out.push('(');
                        out.push_str(&rewritten);
                        if let Some(title) = link.title() {
                            write!(out, " \"{}\"", escape_md_title(&title.str(source)))?;
                        }
                        out.push(')');
                    }
                }
            }
            KindData::Image(image) => {
                let dest = prepared
                    .graph()
                    .image_target(scope.doc_id, node_ref)
                    .ok_or_else(|| {
                        anyhow!(
                            "missing resolved image target for `{}` in {}",
                            image.destination_str(source),
                            prepared.graph().document(scope.doc_id).path.display()
                        )
                    })?
                    .to_string();
                let rewritten = self.rewrite_image_for_markdown(&dest);
                out.push_str("![");
                self.render_image_alt_text(out, arena, node_ref, source)?;
                out.push_str("](");
                out.push_str(&rewritten);
                if let Some(title) = image.title() {
                    write!(out, " \"{}\"", escape_md_title(&title.str(source)))?;
                }
                out.push(')');
            }
            KindData::RawHtml(html) => {
                out.push_str(&html.str(source));
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FootnoteReference) =>
            {
                let footnote = as_extension_data!(arena, node_ref, FootnoteReference);
                let label = footnote.label().str(source).to_string();
                write!(out, "[^{label}]")?;

                // Collect the footnote for rendering at document end.
                let key = (scope.doc_id, label.clone());
                if seen_footnotes.insert(key) {
                    let doc = prepared.graph().document(scope.doc_id);
                    if let Some(definition) = doc.footnote_definition(&label) {
                        footnotes.push(FootnoteEntry {
                            label: label.clone(),
                            doc_id: scope.doc_id,
                            node_ref: definition.node_ref,
                        });
                    }
                }
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::bracketed_span::BracketedSpan
                ) =>
            {
                out.push('[');
                self.render_inline_children(
                    out,
                    prepared,
                    scope,
                    arena,
                    node_ref,
                    source,
                    footnotes,
                    seen_footnotes,
                )?;
                out.push(']');
                // Render attributes.
                let attrs = arena[node_ref].attributes();
                if !attrs.is_empty() {
                    out.push('{');
                    let mut first = true;
                    if let Some(id) = attrs.get("id") {
                        write!(out, "#{}", id.str(source))?;
                        first = false;
                    }
                    if let Some(class) = attrs.get("class") {
                        for cls in class.str(source).split_whitespace() {
                            if !first {
                                out.push(' ');
                            }
                            write!(out, ".{cls}")?;
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
                        write!(out, "{}=\"{}\"", key, value.str(source))?;
                        first = false;
                    }
                    out.push('}');
                }
            }
            KindData::Extension(_) => {}
            _ => {}
        }
        Ok(())
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
                _ => render_image_alt_text_recursive(out, arena, child, source)?,
            }
        }
        Ok(())
    }

    fn render_footnote_definitions(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        footnotes: &[FootnoteEntry],
    ) -> Result<()> {
        if footnotes.is_empty() {
            return Ok(());
        }

        // Track all footnotes we still need to render, including any nested
        // footnotes discovered while rendering bodies.
        let mut pending: Vec<FootnoteEntry> = footnotes.to_vec();
        let mut global_seen: HashSet<(usize, String)> = footnotes
            .iter()
            .map(|e| (e.doc_id, e.label.clone()))
            .collect();

        while !pending.is_empty() {
            let batch = std::mem::take(&mut pending);
            for entry in &batch {
                let doc = prepared.graph().document(entry.doc_id);
                let mut body = String::new();
                let scope = RenderScope {
                    doc_id: entry.doc_id,
                };
                let mut nested_footnotes = Vec::new();
                let mut nested_seen = HashSet::new();
                for child in doc.arena[entry.node_ref].children(&doc.arena) {
                    self.render_block(
                        &mut body,
                        prepared,
                        &scope,
                        &doc.arena,
                        child,
                        &doc.source,
                        0,
                        &mut nested_footnotes,
                        &mut nested_seen,
                    )?;
                }
                let trimmed = body.trim_end();
                let mut first_line = true;
                for line in trimmed.lines() {
                    if first_line {
                        writeln!(out, "[^{}]: {}", entry.label, line)?;
                        first_line = false;
                    } else if line.is_empty() {
                        out.push('\n');
                    } else {
                        writeln!(out, "    {line}")?;
                    }
                }
                if first_line {
                    // Empty footnote body.
                    writeln!(out, "[^{}]:", entry.label)?;
                }

                // Collect newly discovered nested footnotes for the next pass.
                for nested in nested_footnotes {
                    let key = (nested.doc_id, nested.label.clone());
                    if global_seen.insert(key) {
                        pending.push(nested);
                    }
                }
            }
        }
        Ok(())
    }

    /// Rewrite a link target for markdown output.
    ///
    /// Internal links (starting with `#`) are preserved as-is.
    /// External URLs are preserved as-is.
    /// Asset paths (like `static/...`) are rewritten relative to the output directory.
    fn rewrite_link_for_markdown(&self, target: &str) -> String {
        if target.starts_with('#') || target.contains("://") || target.starts_with("mailto:") {
            return target.to_string();
        }
        self.rewrite_asset_path(target)
    }

    /// Rewrite an image path for markdown output.
    ///
    /// Asset paths (`static/...`) are rewritten to be relative from the output
    /// directory to the original file under the project root.
    fn rewrite_image_for_markdown(&self, target: &str) -> String {
        if target.contains("://") {
            return target.to_string();
        }
        self.rewrite_asset_path(target)
    }

    /// Rewrite a `static/...` asset path to a relative path from output_dir to
    /// the original file under base_dir.
    fn rewrite_asset_path(&self, target: &str) -> String {
        // The graph rewrites assets to `static/<base-relative-path>`.
        // Reverse: strip `static/` to get the base-relative path.
        let base_relative = target.strip_prefix("static/").unwrap_or(target);
        let asset_abs = self.base_dir.join(base_relative);
        // Compute relative path from output_dir to the asset.
        let rel = diff_paths(&asset_abs, self.output_dir);
        rel.to_string_lossy().replace('\\', "/")
    }
}

fn render_image_alt_text_recursive(
    out: &mut String,
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Result<()> {
    for child in arena[node_ref].children(arena) {
        match arena[child].kind_data() {
            KindData::Text(text) => out.push_str(text.str(source)),
            KindData::CodeSpan(cs) => out.push_str(&cs.value_str(source)),
            _ => render_image_alt_text_recursive(out, arena, child, source)?,
        }
    }
    Ok(())
}

/// Choose a backtick fence for an inline code span that doesn't conflict with content.
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

/// Escape characters inside a markdown link/image title (double-quoted).
fn escape_md_title(title: &str) -> String {
    title.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Compute a relative path from `base` to `target`.
///
/// Both paths should be absolute. Returns `target` as-is if no relative path
/// can be computed (e.g. different roots on Windows).
fn diff_paths(target: &Path, base: &Path) -> PathBuf {
    pathdiff::diff_paths(target, base).unwrap_or_else(|| target.to_path_buf())
}
