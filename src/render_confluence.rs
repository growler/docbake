//! Confluence Storage Format renderer.
//!
//! Produces XHTML with `ac:` / `ri:` namespaces suitable for the
//! Confluence Server 7.x REST API `body.storage` field.
//!
//! The renderer is a manual recursive tree walker (like `render_typst.rs`)
//! that traverses the document graph, descending into includes, and
//! collects image/diagram attachments along the way.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{anyhow, Result};
use rushdown::as_extension_data;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, KindData, LinkKind, NodeRef, TableCellAlignment, Task, TextQualifier};
use rushdown_definition_list::{DefinitionList, Term, TermDefinition};
use rushdown_fenced_div::FencedDiv;
use rushdown_footnote::{FootnoteDefinition, FootnoteReference};

use crate::cache::Cache;
use crate::diagram::DiagramThemes;
use crate::graph::PreparedDocument;
use crate::render_html_common::is_inline_image;

use crate::DebugDumper;

/// Default width in pixels for block-level SVG images/diagrams in Confluence.
/// This approximates the typical Confluence content area width.
const SVG_DEFAULT_BLOCK_WIDTH: u32 = 800;

/// Width in pixels for inline SVG images (icon-sized).
const SVG_INLINE_WIDTH: u32 = 16;

/// A single attachment to be uploaded alongside the page.
#[derive(Debug)]
pub(crate) struct ConfluenceAttachment {
    /// Filename in Confluence (e.g. `"diagram-0.png"` or `"photo.jpg"`).
    pub filename: String,
    /// Raw file bytes.
    pub data: Vec<u8>,
    /// `"blake3:<hex>"` content hash stored in the attachment comment
    /// field for change detection.
    pub content_hash: String,
}

/// Result of rendering a document for Confluence.
pub(crate) struct ConfluenceDocument {
    /// XHTML storage-format body (without `<html>` / `<body>` wrapper).
    pub body: String,
    /// Attachments collected during rendering.
    pub attachments: Vec<ConfluenceAttachment>,
}

// ── renderer state ──────────────────────────────────────────────────────

struct Scope {
    doc_id: usize,
}

struct FootnoteEntry {
    label: String,
    doc_id: usize,
    index: usize,
}

pub(crate) struct ConfluenceRenderer<'a> {
    cache: &'a Cache,
    diagram_themes: &'a DiagramThemes,
    logger: &'a DebugDumper,
    base_dir: &'a Path,
    attachments: RefCell<Vec<ConfluenceAttachment>>,
    /// Maps (original absolute path) → filename already added, to avoid
    /// uploading duplicates.
    attachment_names: RefCell<HashMap<String, String>>,
    diagram_counter: RefCell<u32>,
}

impl<'a> ConfluenceRenderer<'a> {
    pub fn new(
        cache: &'a Cache,
        diagram_themes: &'a DiagramThemes,
        logger: &'a DebugDumper,
        base_dir: &'a Path,
    ) -> Self {
        Self {
            cache,
            diagram_themes,
            logger,
            base_dir,
            attachments: RefCell::new(Vec::new()),
            attachment_names: RefCell::new(HashMap::new()),
            diagram_counter: RefCell::new(0),
        }
    }

    // ── public entry point ──────────────────────────────────────────────

    pub fn render(self, prepared: &PreparedDocument) -> Result<ConfluenceDocument> {
        let mut out = String::new();

        // Collect footnotes across the entire document graph.
        let footnotes = self.collect_footnotes(prepared);

        self.render_graph_document(&mut out, prepared, prepared.graph().root_id, 0, &footnotes)?;

        // Render footnotes section at the end.
        if !footnotes.is_empty() {
            self.render_footnotes_section(&mut out, prepared, &footnotes)?;
        }

        Ok(ConfluenceDocument {
            body: out,
            attachments: self.attachments.into_inner(),
        })
    }

    // ── graph traversal ─────────────────────────────────────────────────

    fn render_graph_document(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        doc_id: usize,
        heading_shift: usize,
        footnotes: &[FootnoteEntry],
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
                    )?;
                }
                continue;
            }
            self.render_block(
                out,
                prepared,
                &Scope { doc_id },
                &doc.arena,
                child,
                &doc.source,
                heading_shift,
                footnotes,
            )?;
        }
        Ok(())
    }

    // ── block rendering ─────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn render_block(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &Scope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        heading_shift: usize,
        footnotes: &[FootnoteEntry],
    ) -> Result<()> {
        match arena[node_ref].kind_data() {
            KindData::Paragraph(_) => {
                out.push_str("<p>");
                self.render_inline_children(
                    out, prepared, scope, arena, node_ref, source, footnotes,
                )?;
                out.push_str("</p>\n");
            }
            KindData::Heading(heading) => {
                let level = (heading.level() as usize + heading_shift).min(6);
                let id = arena[node_ref]
                    .attributes()
                    .get("id")
                    .map(|v| v.str(source).to_string());
                write!(out, "<h{level}>")?;
                // Emit an anchor macro so internal links work.
                if let Some(ref id) = id {
                    write!(
                        out,
                        "<ac:structured-macro ac:name=\"anchor\">\
                         <ac:parameter ac:name=\"\">{}</ac:parameter>\
                         </ac:structured-macro>",
                        xml_escape(id)
                    )?;
                }
                self.render_inline_children(
                    out, prepared, scope, arena, node_ref, source, footnotes,
                )?;
                write!(out, "</h{level}>\n")?;
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
                    self.render_block(
                        out,
                        prepared,
                        scope,
                        arena,
                        child,
                        source,
                        heading_shift,
                        footnotes,
                    )?;
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
                                        self.render_block(
                                            out,
                                            prepared,
                                            scope,
                                            arena,
                                            child,
                                            source,
                                            heading_shift,
                                            footnotes,
                                        )?;
                                    }
                                    out.push_str("</ac:task-body></ac:task></ac:task-list>");
                                    out.push_str("</li>\n");
                                    continue;
                                }
                                Some(Task::Unchecked) => {
                                    out.push_str("<ac:task-list><ac:task><ac:task-status>incomplete</ac:task-status><ac:task-body>");
                                    for child in arena[item].children(arena) {
                                        self.render_block(
                                            out,
                                            prepared,
                                            scope,
                                            arena,
                                            child,
                                            source,
                                            heading_shift,
                                            footnotes,
                                        )?;
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
                        self.render_block(
                            out,
                            prepared,
                            scope,
                            arena,
                            child,
                            source,
                            heading_shift,
                            footnotes,
                        )?;
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
                self.render_table(out, prepared, scope, arena, node_ref, source, footnotes)?;
            }
            KindData::HtmlBlock(html) => {
                // Pass through raw HTML as-is — Confluence may or may not
                // render it, but stripping it loses information.
                for line in html.value().iter(source) {
                    out.push_str(line.as_ref());
                }
            }
            KindData::RawHtml(_) => {
                // Inline raw HTML — skip in Confluence output.
            }
            KindData::LinkReferenceDefinition(_) => {}
            // ── extension nodes ─────────────────────────────────────────
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, DefinitionList) =>
            {
                self.render_definition_list(
                    out, prepared, scope, arena, node_ref, source, footnotes,
                )?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FencedDiv) =>
            {
                let classes = arena[node_ref]
                    .attributes()
                    .get("class")
                    .map(|v| v.str(source))
                    .unwrap_or_default();

                // Format-gating: skip blocks not targeting confluence.
                let known_formats = crate::KNOWN_OUTPUT_FORMATS;
                let present_formats = classes
                    .split_whitespace()
                    .filter(|c| known_formats.contains(c))
                    .collect::<Vec<_>>();
                if !present_formats.is_empty() && !present_formats.contains(&"confluence") {
                    return Ok(());
                }

                if let Some(adm_type) = crate::admonition_type(&classes) {
                    self.render_admonition(
                        out,
                        prepared,
                        scope,
                        arena,
                        node_ref,
                        source,
                        heading_shift,
                        footnotes,
                        adm_type,
                    )?;
                } else {
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
                        )?;
                    }
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
                )?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::caption::CaptionText
                ) =>
            {
                self.render_inline_children(
                    out, prepared, scope, arena, node_ref, source, footnotes,
                )?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, crate::diagram::Diagram) =>
            {
                self.render_diagram(out, prepared, scope, arena, node_ref, source)?;
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

    // ── inline rendering ────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn render_inline_children(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &Scope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnotes: &[FootnoteEntry],
    ) -> Result<()> {
        for child in arena[node_ref].children(arena) {
            self.render_inline(out, prepared, scope, arena, child, source, footnotes)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_inline(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &Scope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnotes: &[FootnoteEntry],
    ) -> Result<()> {
        match arena[node_ref].kind_data() {
            KindData::Text(text) => {
                out.push_str(&xml_escape(text.str(source)));
                if text.has_qualifiers(TextQualifier::HARD_LINE_BREAK) {
                    out.push_str("<br />\n");
                } else if text.has_qualifiers(TextQualifier::SOFT_LINE_BREAK) {
                    out.push('\n');
                }
            }
            KindData::CodeSpan(cs) => {
                write!(out, "<code>{}</code>", xml_escape(&cs.value_str(source)))?;
            }
            KindData::Emphasis(emphasis) if emphasis.level() == 1 => {
                out.push_str("<em>");
                self.render_inline_children(
                    out, prepared, scope, arena, node_ref, source, footnotes,
                )?;
                out.push_str("</em>");
            }
            KindData::Emphasis(_) => {
                out.push_str("<strong>");
                self.render_inline_children(
                    out, prepared, scope, arena, node_ref, source, footnotes,
                )?;
                out.push_str("</strong>");
            }
            KindData::Strikethrough(_) => {
                // Confluence doesn't support <del>, use inline style.
                out.push_str("<span style=\"text-decoration: line-through;\">");
                self.render_inline_children(
                    out, prepared, scope, arena, node_ref, source, footnotes,
                )?;
                out.push_str("</span>");
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
                    LinkKind::Auto(auto) => {
                        if dest.starts_with('#') {
                            self.render_internal_link(
                                out,
                                &dest,
                                &xml_escape(auto.text_str(source)),
                            )?;
                        } else {
                            write!(
                                out,
                                "<a href=\"{}\">{}</a>",
                                xml_attr_escape(&dest),
                                xml_escape(auto.text_str(source))
                            )?;
                        }
                    }
                    _ => {
                        if dest.starts_with('#') {
                            // Internal anchor link → ac:link with anchor.
                            let mut link_text = String::new();
                            self.render_inline_children(
                                &mut link_text,
                                prepared,
                                scope,
                                arena,
                                node_ref,
                                source,
                                footnotes,
                            )?;
                            self.render_internal_link(out, &dest, &link_text)?;
                        } else {
                            // External link.
                            write!(out, "<a href=\"{}\">", xml_attr_escape(&dest))?;
                            self.render_inline_children(
                                out, prepared, scope, arena, node_ref, source, footnotes,
                            )?;
                            out.push_str("</a>");
                        }
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
                let alt = collect_image_alt_text(arena, node_ref, source);
                let attrs = arena[node_ref].attributes();
                let width = attrs.get("width").map(|v| v.str(source).to_string());
                let height = attrs.get("height").map(|v| v.str(source).to_string());
                let inline = is_inline_image(arena, node_ref);

                if dest.contains("://") {
                    // External image URL.
                    let is_svg = dest.ends_with(".svg") || dest.contains(".svg?");
                    out.push_str("<ac:image");
                    self.write_image_size_attrs(
                        out,
                        width.as_deref(),
                        height.as_deref(),
                        is_svg,
                        inline,
                    );
                    if !alt.is_empty() {
                        write!(out, " ac:alt=\"{}\"", xml_attr_escape(&alt))?;
                    }
                    write!(
                        out,
                        "><ri:url ri:value=\"{}\" /></ac:image>",
                        xml_attr_escape(&dest)
                    )?;
                } else {
                    // Local image → attachment.
                    let filename = self.resolve_image_attachment(&dest, scope.doc_id, prepared)?;
                    let is_svg = filename.ends_with(".svg");
                    out.push_str("<ac:image");
                    self.write_image_size_attrs(
                        out,
                        width.as_deref(),
                        height.as_deref(),
                        is_svg,
                        inline,
                    );
                    if !alt.is_empty() {
                        write!(out, " ac:alt=\"{}\"", xml_attr_escape(&alt))?;
                    }
                    write!(
                        out,
                        "><ri:attachment ri:filename=\"{}\" /></ac:image>",
                        xml_attr_escape(&filename)
                    )?;
                }
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FootnoteReference) =>
            {
                let footnote = as_extension_data!(arena, node_ref, FootnoteReference);
                let label = footnote.label().str(source).to_string();
                if let Some(entry) = footnotes
                    .iter()
                    .find(|e| e.label == label && e.doc_id == scope.doc_id)
                {
                    let idx = entry.index;
                    write!(
                        out,
                        "<sup><ac:link ac:anchor=\"fn:{idx}\"><ac:plain-text-link-body>\
                         <![CDATA[{idx}]]></ac:plain-text-link-body></ac:link></sup>"
                    )?;
                }
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::bracketed_span::BracketedSpan
                ) =>
            {
                out.push_str("<span>");
                self.render_inline_children(
                    out, prepared, scope, arena, node_ref, source, footnotes,
                )?;
                out.push_str("</span>");
            }
            KindData::Extension(_) => {}
            _ => {}
        }
        Ok(())
    }

    // ── internal link helper ────────────────────────────────────────────

    fn render_internal_link(&self, out: &mut String, dest: &str, link_body: &str) -> Result<()> {
        let anchor = dest.trim_start_matches('#');
        write!(
            out,
            "<ac:link ac:anchor=\"{}\"><ac:plain-text-link-body>\
             <![CDATA[{}]]></ac:plain-text-link-body></ac:link>",
            xml_attr_escape(anchor),
            cdata_escape(link_body)
        )?;
        Ok(())
    }

    // ── image/attachment helpers ─────────────────────────────────────────

    /// Emit `ac:width` / `ac:height` attributes for an `<ac:image>`.
    ///
    /// For SVG attachments Confluence requires an explicit pixel `ac:width`
    /// (otherwise SVGs render at 0×0).  The rules are:
    ///
    /// - **Inline SVG** → `ac:width="16"` (icon-sized).
    /// - **Block SVG, no explicit width** → `ac:width="800"`.
    /// - **Block SVG, width=N%** → `ac:width="{N*8}"` (percent of 800).
    /// - **Block SVG, width=Npx or plain N** → `ac:width="N"`.
    /// - **Non-SVG** → pass through width/height as-is (browsers honour
    ///   intrinsic raster dimensions).
    fn write_image_size_attrs(
        &self,
        out: &mut String,
        width: Option<&str>,
        height: Option<&str>,
        is_svg: bool,
        is_inline: bool,
    ) {
        if is_svg {
            if is_inline {
                let _ = write!(out, " ac:width=\"{SVG_INLINE_WIDTH}\"");
            } else {
                let px = svg_width_to_pixels(width);
                let _ = write!(out, " ac:width=\"{px}\"");
            }
            // For SVGs we intentionally omit ac:height so the aspect ratio
            // is preserved by the browser.
            return;
        }
        // Non-SVG: pass through as before.
        if let Some(w) = width {
            let w = normalize_dimension(w);
            let _ = write!(out, " ac:width=\"{w}\"");
        }
        if let Some(h) = height {
            let h = normalize_dimension(h);
            let _ = write!(out, " ac:height=\"{h}\"");
        }
    }

    /// Register a local image as an attachment.  Returns the filename
    /// to reference via `<ri:attachment>`.
    fn resolve_image_attachment(
        &self,
        dest: &str,
        _doc_id: usize,
        prepared: &PreparedDocument,
    ) -> Result<String> {
        // `dest` is in the form `static/relative/path.png` after graph
        // resolution.  Strip the `static/` prefix to get the relative
        // path, then resolve against base_dir.
        let relative = dest.strip_prefix("static/").unwrap_or(dest);
        let abs_path = self.base_dir.join(relative);

        let key = abs_path.to_string_lossy().to_string();

        // Check if already registered.
        if let Some(existing) = self.attachment_names.borrow().get(&key) {
            return Ok(existing.clone());
        }

        // Derive a unique filename (use the file's own name; disambiguate
        // if multiple files from different directories share a name).
        let original_name = abs_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("attachment-{}", self.attachments.borrow().len()));
        let filename = {
            let names = self.attachment_names.borrow();
            let mut candidate = original_name.clone();
            let mut counter = 1u32;
            while names.values().any(|v| v == &candidate) {
                let stem = abs_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let ext = abs_path
                    .extension()
                    .map(|e| e.to_string_lossy().to_string())
                    .unwrap_or_default();
                candidate = if ext.is_empty() {
                    format!("{stem}-{counter}")
                } else {
                    format!("{stem}-{counter}.{ext}")
                };
                counter += 1;
            }
            candidate
        };

        // Read file data.
        // The file might not exist if the asset_targets set was populated
        // but the file was removed.  Check graph asset_targets for the
        // canonical path.
        let data = if abs_path.exists() {
            std::fs::read(&abs_path)
                .map_err(|e| anyhow!("failed to read image {}: {e}", abs_path.display()))?
        } else {
            // Try finding it in asset_targets via different resolution.
            let found = prepared
                .asset_targets()
                .iter()
                .find(|p| p.file_name() == abs_path.file_name());
            if let Some(canonical) = found {
                std::fs::read(canonical)
                    .map_err(|e| anyhow!("failed to read image {}: {e}", canonical.display()))?
            } else {
                self.logger
                    .log(format!("warning: image not found: {}", abs_path.display()));
                return Ok(filename);
            }
        };

        let hash = format!("blake3:{}", blake3::hash(&data).to_hex());
        self.attachment_names
            .borrow_mut()
            .insert(key, filename.clone());
        self.attachments.borrow_mut().push(ConfluenceAttachment {
            filename: filename.clone(),
            data,
            content_hash: hash,
        });

        Ok(filename)
    }

    // ── diagram rendering ───────────────────────────────────────────────

    fn render_diagram(
        &self,
        out: &mut String,
        _prepared: &PreparedDocument,
        _scope: &Scope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        let diagram = as_extension_data!(arena, node_ref, crate::diagram::Diagram);
        let src = crate::diagram::collect_diagram_source(diagram, source);
        let attrs = arena[node_ref].attributes();
        let width = attrs.get("width").map(|v| v.str(source).to_string());
        let height = attrs.get("height").map(|v| v.str(source).to_string());

        let svg_bytes = self.render_diagram_to_svg(diagram, &src)?;

        let n = {
            let mut counter = self.diagram_counter.borrow_mut();
            let n = *counter;
            *counter += 1;
            n
        };
        let filename = format!("diagram-{n}.svg");
        let hash = format!("blake3:{}", blake3::hash(&svg_bytes).to_hex());

        self.attachment_names
            .borrow_mut()
            .insert(format!("__diagram_{n}"), filename.clone());
        self.attachments.borrow_mut().push(ConfluenceAttachment {
            filename: filename.clone(),
            data: svg_bytes,
            content_hash: hash,
        });

        out.push_str("<ac:image");
        // Diagrams are always block-level SVGs.
        self.write_image_size_attrs(out, width.as_deref(), height.as_deref(), true, false);
        write!(
            out,
            "><ri:attachment ri:filename=\"{}\" /></ac:image>\n",
            xml_attr_escape(&filename)
        )?;
        Ok(())
    }

    /// Render a diagram to SVG bytes for Confluence attachment upload.
    fn render_diagram_to_svg(
        &self,
        diagram: &crate::diagram::Diagram,
        src: &str,
    ) -> Result<Vec<u8>> {
        match diagram.diagram_type() {
            crate::diagram::DiagramType::PlantUml => {
                let themed = crate::diagram::apply_plantuml_theme(
                    src,
                    self.diagram_themes.plantuml.as_deref(),
                );
                crate::diagram::cached_render_bytes(
                    self.cache,
                    themed.as_bytes(),
                    "plantuml.svg",
                    || {
                        crate::diagram::render_plantuml_svg(None, themed.as_bytes())
                            .map_err(|e| anyhow!("{e}"))
                    },
                )
            }
            crate::diagram::DiagramType::Mermaid => {
                let themed = crate::diagram::apply_mermaid_theme(
                    src,
                    self.diagram_themes.mermaid.as_deref(),
                );
                crate::diagram::cached_render_svg(
                    self.cache,
                    themed.as_bytes(),
                    "mermaid.svg",
                    || crate::diagram::render_mermaid_svg_auto(None, &themed),
                )
                .map(|s| s.into_bytes())
            }
            crate::diagram::DiagramType::D2 => {
                let themed = crate::diagram::apply_d2_theme(src, self.diagram_themes.d2.as_deref());
                crate::diagram::cached_render_svg(self.cache, themed.as_bytes(), "d2.svg", || {
                    crate::diagram::render_d2_svg(None, &themed).map_err(|e| anyhow!("{e}"))
                })
                .map(|s| s.into_bytes())
            }
            crate::diagram::DiagramType::DrawIo => {
                if let Some(path) = diagram.drawio_path() {
                    let cache_key = crate::diagram::drawio_cache_key(path, diagram.drawio_page());
                    crate::diagram::cached_render_bytes(
                        self.cache,
                        &cache_key,
                        "drawio.svg",
                        || {
                            crate::diagram::render_drawio(
                                None,
                                path,
                                diagram.drawio_page(),
                                crate::diagram::DrawioFormat::Svg,
                            )
                            .map_err(|e| anyhow!("{e}"))
                        },
                    )
                } else {
                    Err(anyhow!("Draw.io diagram without a file path"))
                }
            }
            crate::diagram::DiagramType::Typst => {
                crate::diagram::cached_render_svg(self.cache, src.as_bytes(), "typst.svg", || {
                    crate::diagram::render_typst_svg(None, src).map_err(|e| anyhow!("{e}"))
                })
                .map(|s| s.into_bytes())
            }
        }
    }

    // ── table rendering ─────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn render_table(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &Scope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnotes: &[FootnoteEntry],
    ) -> Result<()> {
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
                            self.render_inline_children(
                                out, prepared, scope, arena, cell, source, footnotes,
                            )?;
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
                            self.render_inline_children(
                                out, prepared, scope, arena, cell, source, footnotes,
                            )?;
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

    // ── definition list ─────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn render_definition_list(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &Scope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnotes: &[FootnoteEntry],
    ) -> Result<()> {
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
                self.render_inline_children(
                    out, prepared, scope, arena, *term_ref, source, footnotes,
                )?;
                out.push_str("</strong></td><td>");
                for def_ref in &definitions {
                    for child in arena[*def_ref].children(arena) {
                        self.render_block(
                            out, prepared, scope, arena, child, source, 0, footnotes,
                        )?;
                    }
                }
                out.push_str("</td></tr>\n");
            }
        }
        out.push_str("</tbody></table>\n");
        Ok(())
    }

    // ── admonition ──────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn render_admonition(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &Scope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        heading_shift: usize,
        footnotes: &[FootnoteEntry],
        adm_type: &str,
    ) -> Result<()> {
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
                self.render_inline_children(
                    &mut title_buf,
                    prepared,
                    scope,
                    arena,
                    first,
                    source,
                    footnotes,
                )?;
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
            self.render_block(
                out,
                prepared,
                scope,
                arena,
                child,
                source,
                heading_shift,
                footnotes,
            )?;
        }
        out.push_str("</ac:rich-text-body></ac:structured-macro>\n");
        Ok(())
    }

    // ── caption ─────────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn render_caption(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        scope: &Scope,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        heading_shift: usize,
        footnotes: &[FootnoteEntry],
    ) -> Result<()> {
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
            self.render_block(
                out,
                prepared,
                scope,
                arena,
                block,
                source,
                heading_shift,
                footnotes,
            )?;
        }
        if let Some(ct) = caption_text_child {
            out.push_str("<p><em>");
            self.render_inline_children(out, prepared, scope, arena, ct, source, footnotes)?;
            out.push_str("</em></p>\n");
        }
        Ok(())
    }

    // ── footnotes ───────────────────────────────────────────────────────

    fn collect_footnotes(&self, prepared: &PreparedDocument) -> Vec<FootnoteEntry> {
        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut index = 1usize;
        for (doc_id, node_ref, _) in prepared.graph().rendered_nodes_preorder() {
            let doc = prepared.graph().document(doc_id);
            if !rushdown::matches_extension_kind!(&doc.arena, node_ref, FootnoteReference) {
                continue;
            }
            let footnote = as_extension_data!(&doc.arena, node_ref, FootnoteReference);
            let label = footnote.label().str(&doc.source).to_string();
            let key = (doc_id, label.clone());
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);
            entries.push(FootnoteEntry {
                label,
                doc_id,
                index,
            });
            index += 1;
        }
        entries
    }

    fn render_footnotes_section(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        footnotes: &[FootnoteEntry],
    ) -> Result<()> {
        out.push_str("<hr />\n<ol>\n");
        for entry in footnotes {
            let doc = prepared.graph().document(entry.doc_id);
            write!(
                out,
                "<li><ac:structured-macro ac:name=\"anchor\">\
                 <ac:parameter ac:name=\"\">fn:{}</ac:parameter>\
                 </ac:structured-macro>",
                entry.index
            )?;
            if let Some(def) = doc.footnote_definition(&entry.label) {
                for child in doc.arena[def.node_ref].children(&doc.arena) {
                    self.render_block(
                        out,
                        prepared,
                        &Scope {
                            doc_id: entry.doc_id,
                        },
                        &doc.arena,
                        child,
                        &doc.source,
                        0,
                        footnotes,
                    )?;
                }
            }
            out.push_str("</li>\n");
        }
        out.push_str("</ol>\n");
        Ok(())
    }
}

// ── XML escaping utilities ──────────────────────────────────────────────

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

fn xml_attr_escape(s: &str) -> String {
    xml_escape(s)
}

/// Escape text for CDATA sections.  CDATA cannot contain `]]>`, so we
/// split the CDATA section around it.
fn cdata_escape(s: &str) -> String {
    s.replace("]]>", "]]]]><![CDATA[>")
}

/// Extract the alt text from an Image node by collecting text from its
/// inline children (the `![alt text](url)` content).
fn collect_image_alt_text(arena: &Arena, node_ref: NodeRef, source: &str) -> String {
    let mut alt = String::new();
    for child in arena[node_ref].children(arena) {
        match arena[child].kind_data() {
            KindData::Text(text) => alt.push_str(text.str(source)),
            KindData::CodeSpan(cs) => alt.push_str(&cs.value_str(source)),
            _ => {}
        }
    }
    alt
}

/// Normalize a CSS dimension value to a plain pixel number for Confluence
/// `ac:width` / `ac:height` attributes.
fn normalize_dimension(value: &str) -> String {
    let trimmed = value.trim();
    // Already a plain number.
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return trimmed.to_string();
    }
    // Strip "px" suffix.
    if let Some(num) = trimmed.strip_suffix("px") {
        return num.trim().to_string();
    }
    // Percentage — not directly supported by ac:width, pass through as-is.
    trimmed.to_string()
}

/// Convert a width attribute value to a pixel value for SVG images.
///
/// Confluence requires a numeric pixel `ac:width` for SVGs; without it
/// the image renders at 0×0.  Percentages are converted relative to
/// [`SVG_DEFAULT_BLOCK_WIDTH`].
fn svg_width_to_pixels(width: Option<&str>) -> u32 {
    let Some(w) = width else {
        return SVG_DEFAULT_BLOCK_WIDTH;
    };
    let trimmed = w.trim();
    // Percentage: "50%" → 50% of 800 = 400
    if let Some(pct) = trimmed.strip_suffix('%') {
        if let Ok(n) = pct.trim().parse::<f64>() {
            return ((n / 100.0) * SVG_DEFAULT_BLOCK_WIDTH as f64).round() as u32;
        }
    }
    // Pixels: "200px" or "200"
    let num_str = trimmed.strip_suffix("px").unwrap_or(trimmed);
    if let Ok(n) = num_str.trim().parse::<u32>() {
        return n;
    }
    SVG_DEFAULT_BLOCK_WIDTH
}
