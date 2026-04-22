//! DOCX renderer for Google Docs publishing.
//!
//! Walks the document graph and produces a DOCX file using the `docx-rs`
//! crate.  The output is designed for upload to Google Drive with automatic
//! conversion to native Google Docs format.
//!
//! The main rendering loop mirrors the Typst renderer: it iterates
//! **document-level children** (resolving includes recursively) and calls
//! `render_block` for each top-level block.  `render_block` handles
//! children internally — this avoids the duplication that would occur with
//! a flat preorder walk.
//!
//! Diagrams are rendered to PNG and embedded as inline images.
//! Regular images are read from disk and embedded directly.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Cursor;
use std::path::Path;

use anyhow::{anyhow, Result};
use docx_rs::*;
use resvg::{tiny_skia, usvg};
use rushdown::as_extension_data;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, KindData, NodeRef, TextQualifier};
use rushdown_definition_list::{DefinitionList, Term, TermDefinition};
use rushdown_fenced_div::FencedDiv;

use crate::cache::Cache;
use crate::caption::{Caption, CaptionText};
use crate::diagram::DiagramThemes;
use crate::graph::{DocumentGraph, PreparedDocument};
use crate::DebugDumper;

/// Numbering ID for unordered (bullet) lists.
const BULLET_NUMBERING_ID: usize = 1;
/// Numbering ID for ordered (decimal) lists.
const ORDERED_NUMBERING_ID: usize = 2;

/// EMU per pixel at 96 DPI (1 inch = 914400 EMU, 1 inch = 96 px).
const EMU_PER_PX: u32 = 9525;

/// Maximum image width in EMU (~6 inches at 96 DPI).
const MAX_IMAGE_WIDTH_EMU: u32 = 5_760_000;
const FRONT_LOGO_WIDTH_EMU: u32 = 1_712_140;
const FRONT_LOGO_HEIGHT_EMU: u32 = 330_413;
const ACCENT_ORANGE: &str = "EE721C";

/// Standard body font family.
const BODY_FONT: &str = "Calibri";

/// Heading font sizes in half-points, indexed by heading level (1-based).
/// H1=28pt, H2=22pt, H3=16pt, H4=14pt, H5=12pt, H6=11pt.
const HEADING_SIZES: [usize; 7] = [0, 56, 44, 32, 28, 24, 22];

/// Space before heading in twips, by level.
const HEADING_BEFORE: [u32; 7] = [0, 360, 280, 240, 200, 200, 200];

/// Space after heading in twips, by level.
const HEADING_AFTER: [u32; 7] = [0, 120, 100, 80, 60, 60, 60];

/// Code block background fill colour (light grey).
const CODE_BG: &str = "F2F2F2";

/// Code block border colour (slightly darker grey).
const CODE_BORDER: &str = "D9D9D9";

/// TOC depth: include heading levels 1..=TOC_DEPTH.
const TOC_DEPTH: usize = 3;

/// Admonition colour scheme: (border, title_fill, body_fill).
/// Inspired by the MS Office styling captured from office.docx.
fn admonition_colors(adm_type: &str) -> (&'static str, &'static str, &'static str) {
    match adm_type {
        // green
        "tip" => ("8DD873", "B3E5A1", "D9F2D0"),
        // blue
        "note" => ("6CB4EE", "A6D1F2", "D4E9F9"),
        // red-orange
        "warning" => ("E8895C", "F2C4A6", "F9E2D4"),
        // orange
        "caution" => ("E8B95C", "F2DCA6", "F9EED4"),
        // dark red
        "important" => ("E86C6C", "F2ADAD", "F9D6D6"),
        // grey-blue
        "related" => ("8CA1B8", "BCCBDA", "DDE5ED"),
        _ => ("8CA1B8", "BCCBDA", "DDE5ED"),
    }
}

pub(crate) struct DocxRenderer<'a> {
    cache: &'a Cache,
    diagram_themes: &'a DiagramThemes,
    logger: &'a DebugDumper,
    base_dir: &'a Path,
    logo_asset: Option<(Vec<u8>, String)>,
    #[allow(dead_code)]
    diagram_counter: RefCell<u32>,
    toc_heading_keys: RefCell<VecDeque<String>>,
    bookmark_counter: RefCell<usize>,
}

#[derive(Clone, Copy)]
struct RenderContext<'a> {
    arena: &'a Arena,
    source: &'a str,
    doc_id: usize,
    heading_shift: usize,
    graph: &'a DocumentGraph,
}

impl<'a> DocxRenderer<'a> {
    pub fn new(
        cache: &'a Cache,
        diagram_themes: &'a DiagramThemes,
        logger: &'a DebugDumper,
        base_dir: &'a Path,
        logo_asset: Option<(Vec<u8>, String)>,
    ) -> Self {
        Self {
            cache,
            diagram_themes,
            logger,
            base_dir,
            logo_asset,
            diagram_counter: RefCell::new(0),
            toc_heading_keys: RefCell::new(VecDeque::new()),
            bookmark_counter: RefCell::new(0),
        }
    }

    /// Render the document graph to DOCX bytes.
    pub fn render(&self, prepared: &PreparedDocument) -> Result<Vec<u8>> {
        let graph = prepared.graph();
        let metadata = prepared.document_metadata();

        let body_fonts = RunFonts::new()
            .ascii(BODY_FONT)
            .hi_ansi(BODY_FONT)
            .east_asia(BODY_FONT)
            .cs(BODY_FONT);

        let mut docx = Docx::new()
            // ── Document-wide defaults ──────────────────────────────────
            // 1.15x line spacing (276/240), 6pt after each paragraph.
            .default_line_spacing(
                LineSpacing::new()
                    .after(120) // 6pt after
                    .line(276) // 1.15x
                    .line_rule(LineSpacingType::Auto),
            )
            .default_fonts(body_fonts)
            .default_size(22); // 11pt body text (half-points)

        // ── Register heading styles ─────────────────────────────────────
        let heading_fonts = RunFonts::new()
            .ascii("Calibri")
            .hi_ansi("Calibri")
            .east_asia("Calibri")
            .cs("Calibri");

        for level in 1..=6usize {
            let style_id = format!("Heading{level}");
            let style_name = format!("heading {level}");

            let spacing = LineSpacing::new()
                .before(HEADING_BEFORE[level])
                .after(HEADING_AFTER[level])
                .line(240) // single-spaced headings
                .line_rule(LineSpacingType::Auto);

            let mut style = Style::new(&style_id, StyleType::Paragraph)
                .name(&style_name)
                .size(HEADING_SIZES[level])
                .bold()
                .fonts(heading_fonts.clone())
                .outline_lvl(level - 1)
                .line_spacing(spacing);

            if level >= 4 {
                style = style.color("333333");
            }

            docx = docx.add_style(style);
        }

        // ── Define list numbering styles ────────────────────────────────
        docx = docx
            .add_abstract_numbering(
                AbstractNumbering::new(BULLET_NUMBERING_ID)
                    .add_level(
                        Level::new(
                            0,
                            Start::new(1),
                            NumberFormat::new("bullet"),
                            LevelText::new("\u{2022}"),
                            LevelJc::new("left"),
                        )
                        .indent(
                            Some(720),
                            Some(SpecialIndentType::Hanging(360)),
                            None,
                            None,
                        ),
                    )
                    .add_level(
                        Level::new(
                            1,
                            Start::new(1),
                            NumberFormat::new("bullet"),
                            LevelText::new("\u{25E6}"),
                            LevelJc::new("left"),
                        )
                        .indent(
                            Some(1440),
                            Some(SpecialIndentType::Hanging(360)),
                            None,
                            None,
                        ),
                    )
                    .add_level(
                        Level::new(
                            2,
                            Start::new(1),
                            NumberFormat::new("bullet"),
                            LevelText::new("\u{25AA}"),
                            LevelJc::new("left"),
                        )
                        .indent(
                            Some(2160),
                            Some(SpecialIndentType::Hanging(360)),
                            None,
                            None,
                        ),
                    ),
            )
            .add_abstract_numbering(
                AbstractNumbering::new(ORDERED_NUMBERING_ID)
                    .add_level(
                        Level::new(
                            0,
                            Start::new(1),
                            NumberFormat::new("decimal"),
                            LevelText::new("%1."),
                            LevelJc::new("left"),
                        )
                        .indent(
                            Some(720),
                            Some(SpecialIndentType::Hanging(360)),
                            None,
                            None,
                        ),
                    )
                    .add_level(
                        Level::new(
                            1,
                            Start::new(1),
                            NumberFormat::new("lowerLetter"),
                            LevelText::new("%2."),
                            LevelJc::new("left"),
                        )
                        .indent(
                            Some(1440),
                            Some(SpecialIndentType::Hanging(360)),
                            None,
                            None,
                        ),
                    )
                    .add_level(
                        Level::new(
                            2,
                            Start::new(1),
                            NumberFormat::new("lowerRoman"),
                            LevelText::new("%3."),
                            LevelJc::new("left"),
                        )
                        .indent(
                            Some(2160),
                            Some(SpecialIndentType::Hanging(360)),
                            None,
                            None,
                        ),
                    ),
            )
            .add_numbering(Numbering::new(BULLET_NUMBERING_ID, BULLET_NUMBERING_ID))
            .add_numbering(Numbering::new(ORDERED_NUMBERING_ID, ORDERED_NUMBERING_ID));

        // ── Title page ──────────────────────────────────────────────────
        let title = metadata
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let subtitle = metadata
            .get("subtitle")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let author = metadata
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let version_str = metadata
            .get("version")
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => String::new(),
            })
            .unwrap_or_default();
        let date = metadata
            .get("date")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        if !version_str.is_empty() {
            docx = docx.custom_property("Version", &version_str);
        }
        if !title.is_empty() {
            docx = docx.header(self.build_default_header(title));
        } else {
            docx = docx.header(self.build_default_header(""));
        }
        docx = docx
            .footer(self.build_default_footer(&version_str))
            .title_pg();

        let cal_fonts = RunFonts::new()
            .ascii("Calibri")
            .hi_ansi("Calibri")
            .east_asia("Calibri")
            .cs("Calibri");

        let has_title_page = !title.is_empty();

        if has_title_page {
            if let Some(logo_pic) = self.resolve_front_logo() {
                docx = docx.add_paragraph(Paragraph::new().add_run(Run::new().add_image(logo_pic)));
            }

            for _ in 0..5 {
                let spacer = Paragraph::new().line_spacing(LineSpacing::new().after(0).line(240));
                docx = docx.add_paragraph(spacer);
            }

            let title_run = Run::new()
                .add_text(title)
                .size(60)
                .bold()
                .fonts(cal_fonts.clone());
            let title_border = ParagraphBorders::with_empty().set(
                ParagraphBorder::new(ParagraphBorderPosition::Bottom)
                    .val(BorderType::Single)
                    .size(4)
                    .space(1)
                    .color(ACCENT_ORANGE),
            );
            let mut title_para = Paragraph::new()
                .add_run(title_run)
                .align(AlignmentType::Right)
                .line_spacing(LineSpacing::new().after(200).line(240))
                .color(ACCENT_ORANGE);
            title_para.property = title_para.property.set_borders(title_border);
            docx = docx.add_paragraph(title_para);

            if !subtitle.is_empty() {
                let sub_run = Run::new()
                    .add_text(subtitle)
                    .size(36)
                    .color("666666")
                    .fonts(cal_fonts.clone());
                docx = docx.add_paragraph(
                    Paragraph::new()
                        .add_run(sub_run)
                        .align(AlignmentType::Center)
                        .line_spacing(LineSpacing::new().after(200).line(240)),
                );
            }

            if !version_str.is_empty() {
                let ver_run = Run::new()
                    .add_text("Version ")
                    .size(28)
                    .color("888888")
                    .fonts(cal_fonts.clone());
                let ver_val_run = Run::new()
                    .add_text(&version_str)
                    .size(28)
                    .color("888888")
                    .fonts(cal_fonts.clone());
                docx = docx.add_paragraph(
                    Paragraph::new()
                        .add_run(ver_run)
                        .add_run(ver_val_run)
                        .align(AlignmentType::Right)
                        .line_spacing(LineSpacing::new().after(120).line(240)),
                );
            }

            if !author.is_empty() {
                let author_run = Run::new()
                    .add_text(author)
                    .size(24)
                    .color("666666")
                    .fonts(cal_fonts.clone());
                docx = docx.add_paragraph(
                    Paragraph::new()
                        .add_run(author_run)
                        .align(AlignmentType::Center)
                        .line_spacing(LineSpacing::new().after(120).line(240)),
                );
            }

            if !date.is_empty() {
                let date_run = Run::new()
                    .add_text(date)
                    .size(24)
                    .color("888888")
                    .fonts(cal_fonts);
                docx = docx.add_paragraph(
                    Paragraph::new()
                        .add_run(date_run)
                        .align(AlignmentType::Center)
                        .line_spacing(LineSpacing::new().after(120).line(240)),
                );
            }

            docx = docx.add_paragraph(
                Paragraph::new()
                    .add_run(Run::new().add_break(BreakType::Page))
                    .line_spacing(LineSpacing::new().after(0).line(240)),
            );
        }

        let toc_entries = self.collect_toc_entries(graph);
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

        // ── Walk the document graph (top-level children only) ───────────
        // Mirror the Typst renderer approach: iterate document-level
        // children, resolve includes recursively, call render_block for
        // each top-level block.  render_block handles its own children.
        self.render_graph_document(&mut docx, graph, graph.root_id, 0)?;

        // Serialize to bytes.
        let mut buf = Cursor::new(Vec::new());
        docx.build()
            .pack(&mut buf)
            .map_err(|e| anyhow!("failed to pack DOCX: {e}"))?;
        Ok(buf.into_inner())
    }

    /// Walk a document occurrence, resolving includes and rendering
    /// top-level blocks.  Mirrors the Typst renderer's
    /// `render_graph_document`.
    fn render_graph_document(
        &self,
        docx: &mut Docx,
        graph: &DocumentGraph,
        doc_id: usize,
        heading_shift: usize,
    ) -> Result<()> {
        let doc = graph.document(doc_id);
        for child in doc.arena[doc.doc_ref].children(&doc.arena) {
            if let Some(entries) = graph.include_entries(doc_id, child) {
                for entry in entries {
                    self.render_graph_document(
                        docx,
                        graph,
                        entry.doc_id,
                        heading_shift + entry.heading_shift,
                    )?;
                }
                continue;
            }
            let context = RenderContext {
                arena: &doc.arena,
                source: &doc.source,
                doc_id,
                heading_shift,
                graph,
            };
            self.render_block(docx, &context, child)?;
        }
        Ok(())
    }

    fn collect_toc_entries(&self, graph: &DocumentGraph) -> Vec<TocEntry> {
        let mut entries = Vec::new();
        let mut idx = 0usize;

        for (doc_id, node_ref, heading_shift) in graph.rendered_nodes_preorder() {
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

        entries
    }

    /// Generate a static table of contents from headings in the graph.
    #[allow(dead_code)]
    fn render_static_toc(&self, docx: &mut Docx, graph: &DocumentGraph) {
        // "Table of Contents" header — bold, larger, but NOT a heading
        // style to avoid it appearing in the outline.
        let toc_header = Paragraph::new()
            .add_run(Run::new().add_text("Table of Contents").size(32).bold())
            .line_spacing(LineSpacing::new().after(200).line(240));
        *docx = std::mem::take(docx).add_paragraph(toc_header);

        // Collect headings from the rendered node preorder walk.
        for (doc_id, node_ref, heading_shift) in graph.rendered_nodes_preorder() {
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
                *docx = std::mem::take(docx).add_paragraph(para);
            }
        }
    }

    fn render_block(
        &self,
        docx: &mut Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        match arena[node_ref].kind_data() {
            KindData::Heading(_) => {
                let heading = as_kind_data!(arena[node_ref], Heading);
                let level = (heading.level() as usize + context.heading_shift).min(6);
                let style = format!("Heading{level}");
                let heading_text = Self::collect_inline_text(arena, source, node_ref);

                let mut para = Paragraph::new().style(&style).keep_next(true);
                if level == 1 {
                    let border = ParagraphBorders::with_empty().set(
                        ParagraphBorder::new(ParagraphBorderPosition::Bottom)
                            .val(BorderType::Single)
                            .size(4)
                            .space(1)
                            .color(ACCENT_ORANGE),
                    );
                    para.property = para.property.set_borders(border);
                    para = para.color(ACCENT_ORANGE);
                }

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
                        if level == 1 {
                            para = para.add_run(run.color(ACCENT_ORANGE));
                        } else {
                            para = para.add_run(run);
                        }
                    }
                }
                if has_bookmark {
                    let id = *self.bookmark_counter.borrow() - 1;
                    para = para.add_bookmark_end(id);
                }
                *docx = std::mem::take(docx).add_paragraph(para);
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
                *docx = std::mem::take(docx).add_paragraph(para);
            }
            KindData::CodeBlock(code_block) => {
                let mut code_text = String::new();
                for line in code_block.value().iter(source) {
                    code_text.push_str(&line);
                }
                self.render_code_block_paragraphs(docx, &code_text);
            }
            KindData::Blockquote(_) => {
                for child in arena[node_ref].children(arena) {
                    self.render_block(docx, context, child)?;
                }
            }
            KindData::List(list) => {
                let is_ordered = list.is_ordered();
                let numbering_id = if is_ordered {
                    ORDERED_NUMBERING_ID
                } else {
                    BULLET_NUMBERING_ID
                };

                self.render_list_items(docx, context, node_ref, numbering_id, 0)?;
            }
            KindData::Table(_) => {
                self.render_table(docx, context, node_ref)?;
            }
            KindData::ThematicBreak(_) => {
                let para = Paragraph::new().add_run(Run::new().add_text(""));
                *docx = std::mem::take(docx).add_paragraph(para);
            }
            _ => {
                // Handle extension nodes (diagrams, fenced divs, captions, etc.)
                if rushdown::matches_extension_kind!(arena, node_ref, Caption) {
                    self.render_caption(docx, context, node_ref)?;
                } else if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::diagram::Diagram
                ) {
                    self.render_diagram(docx, arena, source, node_ref)?;
                } else if rushdown::matches_extension_kind!(arena, node_ref, FencedDiv) {
                    self.render_fenced_div(docx, context, node_ref)?;
                } else if rushdown::matches_extension_kind!(arena, node_ref, DefinitionList) {
                    self.render_definition_list(docx, context, node_ref)?;
                }
                // FootnoteDefinition nodes are rendered inline at reference sites — skip here.
            }
        }
        Ok(())
    }

    /// Render a fenced div, with special handling for admonition blocks.
    ///
    /// For admonition types (`note`, `warning`, `tip`, etc.) the first
    /// child heading is rendered as bold text (not a real heading) to
    /// avoid polluting the document outline / TOC.
    fn render_fenced_div(
        &self,
        docx: &mut Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        let classes = arena[node_ref]
            .attributes()
            .get("class")
            .map(|v| v.str(source))
            .unwrap_or_default();

        let adm_type = crate::admonition_type(&classes);

        if let Some(adm_type) = adm_type {
            self.render_admonition(docx, context, node_ref, adm_type)?;
        } else {
            // Non-admonition fenced div: render children as normal blocks.
            for child in arena[node_ref].children(arena) {
                self.render_block(docx, context, child)?;
            }
        }
        Ok(())
    }

    /// Render an admonition block with coloured borders and background.
    ///
    /// - Title paragraph: darker background, bold text, coloured border.
    /// - Body paragraphs: lighter background, same coloured border.
    /// - Adjacent paragraphs with identical borders merge into a
    ///   continuous box in Word and Google Docs.
    fn render_admonition(
        &self,
        docx: &mut Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
        adm_type: &str,
    ) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        let children: Vec<_> = arena[node_ref].children(arena).collect();

        let (title, skip_first) = if let Some(&first) = children.first() {
            if matches!(arena[first].kind_data(), KindData::Heading(_)) {
                let title = Self::collect_inline_text(arena, source, first);
                (title, true)
            } else {
                (admonition_default_title(adm_type).to_string(), false)
            }
        } else {
            (admonition_default_title(adm_type).to_string(), false)
        };

        let (border_color, title_fill, body_fill) = admonition_colors(adm_type);

        let make_borders = |color: &str| {
            ParagraphBorders::with_empty()
                .set(
                    ParagraphBorder::new(ParagraphBorderPosition::Top)
                        .val(BorderType::Single)
                        .size(4)
                        .space(1)
                        .color(color),
                )
                .set(
                    ParagraphBorder::new(ParagraphBorderPosition::Bottom)
                        .val(BorderType::Single)
                        .size(4)
                        .space(1)
                        .color(color),
                )
                .set(
                    ParagraphBorder::new(ParagraphBorderPosition::Left)
                        .val(BorderType::Single)
                        .size(4)
                        .space(4)
                        .color(color),
                )
                .set(
                    ParagraphBorder::new(ParagraphBorderPosition::Right)
                        .val(BorderType::Single)
                        .size(4)
                        .space(4)
                        .color(color),
                )
        };

        let adm_borders = make_borders(border_color);

        // ── Title paragraph ─────────────────────────────────────────
        let title_run = Run::new().add_text(&title).bold().size(22);
        let mut title_para = Paragraph::new()
            .add_run(title_run)
            .line_spacing(LineSpacing::new().before(120).after(60).line(240));
        title_para.property = title_para
            .property
            .shading(Shading::new().fill(title_fill))
            .set_borders(adm_borders.clone());
        *docx = std::mem::take(docx).add_paragraph(title_para);

        // ── Body children ───────────────────────────────────────────
        // We need to apply the body styling to each body paragraph.
        // For simple paragraphs we can set borders/shading directly.
        // For complex blocks (tables, lists, code) we render them
        // normally (they break out of the box visually, which is
        // acceptable — same limitation as Google Docs import).
        let body = if skip_first {
            &children[1..]
        } else {
            &children[..]
        };
        let body_borders = adm_borders;
        for &child in body {
            match arena[child].kind_data() {
                KindData::Paragraph(_) => {
                    let mut para = Paragraph::new();
                    for inline in arena[child].children(arena) {
                        match arena[inline].kind_data() {
                            KindData::Image(_) => {
                                if let Some(pic) = self.resolve_graph_image(context, inline) {
                                    para = para.add_run(Run::new().add_image(pic));
                                }
                            }
                            _ => {
                                let runs = self.render_inline_runs(context, inline);
                                for run in runs {
                                    para = para.add_run(run);
                                }
                            }
                        }
                    }
                    para.property = para
                        .property
                        .shading(Shading::new().fill(body_fill))
                        .set_borders(body_borders.clone());
                    *docx = std::mem::take(docx).add_paragraph(para);
                }
                _ => {
                    // Complex blocks (lists, tables, code, nested divs):
                    // render normally without the admonition box styling.
                    self.render_block(docx, context, child)?;
                }
            }
        }

        Ok(())
    }

    /// Render an inline node to one or more `Run`s.
    fn render_inline_runs(&self, context: &RenderContext<'_>, node_ref: NodeRef) -> Vec<Run> {
        let arena = context.arena;
        let source = context.source;
        match arena[node_ref].kind_data() {
            KindData::Text(text) => {
                let content = text.str(source);
                let mut runs = vec![Run::new().add_text(content)];
                if text.has_qualifiers(TextQualifier::HARD_LINE_BREAK) {
                    runs.push(Run::new().add_break(BreakType::TextWrapping));
                } else if text.has_qualifiers(TextQualifier::SOFT_LINE_BREAK) {
                    runs.push(Run::new().add_text(" "));
                }
                runs
            }
            KindData::CodeSpan(cs) => {
                let content = cs.value_str(source);
                let mono_fonts = RunFonts::new()
                    .ascii("Courier New")
                    .hi_ansi("Courier New")
                    .east_asia("Courier New")
                    .cs("Courier New");
                vec![Run::new()
                    .add_text(&*content)
                    .fonts(mono_fonts)
                    .shading(Shading::new().fill(CODE_BG))]
            }
            KindData::Emphasis(emphasis) if emphasis.level() == 1 => {
                let children_runs = self.collect_child_runs(context, node_ref);
                children_runs.into_iter().map(|r| r.italic()).collect()
            }
            KindData::Emphasis(_) => {
                let children_runs = self.collect_child_runs(context, node_ref);
                children_runs.into_iter().map(|r| r.bold()).collect()
            }
            KindData::Strikethrough(_) => {
                let children_runs = self.collect_child_runs(context, node_ref);
                children_runs.into_iter().map(|r| r.strike()).collect()
            }
            KindData::Link(link) => {
                let text = Self::collect_inline_text(arena, source, node_ref);
                let _dest = context
                    .graph
                    .link_target(context.doc_id, node_ref)
                    .unwrap_or(link.destination_str(source));
                vec![Run::new()
                    .add_text(&text)
                    .color("0563C1")
                    .underline("single")]
            }
            KindData::Image(_) => {
                if let Some(pic) = self.resolve_graph_image(context, node_ref) {
                    vec![Run::new().add_image(pic)]
                } else {
                    let alt = Self::collect_inline_text(arena, source, node_ref);
                    if alt.is_empty() {
                        vec![]
                    } else {
                        vec![Run::new().add_text(&alt)]
                    }
                }
            }
            _ => self.collect_child_runs(context, node_ref),
        }
    }

    /// Collect `Run`s from all children of a node.
    fn collect_child_runs(&self, context: &RenderContext<'_>, node_ref: NodeRef) -> Vec<Run> {
        let arena = context.arena;
        let mut runs = Vec::new();
        for child in arena[node_ref].children(arena) {
            runs.extend(self.render_inline_runs(context, child));
        }
        runs
    }

    /// Recursively collect plain text from an inline subtree.
    fn collect_inline_text(arena: &Arena, source: &str, node_ref: NodeRef) -> String {
        let mut buf = String::new();
        for child in arena[node_ref].children(arena) {
            match arena[child].kind_data() {
                KindData::Text(text) => {
                    buf.push_str(text.str(source));
                    if text.has_qualifiers(TextQualifier::SOFT_LINE_BREAK) {
                        buf.push(' ');
                    } else if text.has_qualifiers(TextQualifier::HARD_LINE_BREAK) {
                        buf.push('\n');
                    }
                }
                KindData::CodeSpan(cs) => {
                    buf.push_str(&cs.value_str(source));
                }
                _ => {
                    buf.push_str(&Self::collect_inline_text(arena, source, child));
                }
            }
        }
        buf
    }

    /// Render list items recursively, handling nested lists.
    fn render_list_items(
        &self,
        docx: &mut Docx,
        context: &RenderContext<'_>,
        list_ref: NodeRef,
        numbering_id: usize,
        indent_level: usize,
    ) -> Result<()> {
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
                        *docx = std::mem::take(docx).add_paragraph(para);
                    }
                    KindData::List(nested_list) => {
                        let nested_id = if nested_list.is_ordered() {
                            ORDERED_NUMBERING_ID
                        } else {
                            BULLET_NUMBERING_ID
                        };
                        self.render_list_items(docx, context, child, nested_id, indent_level + 1)?;
                    }
                    _ => {
                        self.render_block(docx, context, child)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Render a table to a DOCX table.
    fn render_table(
        &self,
        docx: &mut Docx,
        context: &RenderContext<'_>,
        table_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        let mut header_rows = Vec::new();
        let mut body_rows = Vec::new();

        for child in arena[table_ref].children(arena) {
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

        let mut rows = Vec::new();
        for row_ref in &header_rows {
            let cells = self.render_table_row(docx, context, *row_ref, true)?;
            rows.push(TableRow::new(cells));
        }
        for row_ref in &body_rows {
            let cells = self.render_table_row(docx, context, *row_ref, false)?;
            rows.push(TableRow::new(cells));
        }

        if !rows.is_empty() {
            let table = Table::new(rows);
            *docx = std::mem::take(docx).add_table(table);
            *docx = std::mem::take(docx).add_paragraph(Paragraph::new());
        }

        Ok(())
    }

    /// Render a single table row, returning a Vec of cells.
    fn render_table_row(
        &self,
        _docx: &mut Docx,
        context: &RenderContext<'_>,
        row_ref: NodeRef,
        is_header: bool,
    ) -> Result<Vec<TableCell>> {
        let arena = context.arena;
        let mut cells = Vec::new();
        for cell_ref in arena[row_ref].children(arena) {
            if !matches!(arena[cell_ref].kind_data(), KindData::TableCell(_)) {
                continue;
            }
            let mut para = Paragraph::new();
            for child in arena[cell_ref].children(arena) {
                let mut runs = self.render_inline_runs(context, child);
                if is_header {
                    runs = runs.into_iter().map(|r| r.bold()).collect();
                }
                for run in runs {
                    para = para.add_run(run);
                }
            }
            cells.push(TableCell::new().add_paragraph(para));
        }
        Ok(cells)
    }

    /// Render a definition list.
    fn render_definition_list(
        &self,
        docx: &mut Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
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
                *docx = std::mem::take(docx).add_paragraph(para);
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
                        *docx = std::mem::take(docx).add_paragraph(para);
                    }
                }
            }
        }
        Ok(())
    }

    /// Render a Caption extension node.
    fn render_caption(
        &self,
        docx: &mut Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
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
            *docx = std::mem::take(docx).add_paragraph(para);
        }

        if let Some(br) = block_ref {
            self.render_block(docx, context, br)?;
        }

        Ok(())
    }

    /// Render code block lines as monospace paragraphs with grey background
    /// and a light border box (matching MS Office code block styling).
    fn render_code_block_paragraphs(&self, docx: &mut Docx, code_text: &str) {
        let mono_fonts = RunFonts::new()
            .ascii("Courier New")
            .hi_ansi("Courier New")
            .east_asia("Courier New")
            .cs("Courier New");

        // Paragraph-level borders: light grey single border on all sides.
        // Adjacent paragraphs with identical borders merge into a
        // continuous box in Word and Google Docs.
        let code_borders = ParagraphBorders::with_empty()
            .set(
                ParagraphBorder::new(ParagraphBorderPosition::Top)
                    .val(BorderType::Single)
                    .size(4)
                    .space(1)
                    .color(CODE_BORDER),
            )
            .set(
                ParagraphBorder::new(ParagraphBorderPosition::Bottom)
                    .val(BorderType::Single)
                    .size(4)
                    .space(1)
                    .color(CODE_BORDER),
            )
            .set(
                ParagraphBorder::new(ParagraphBorderPosition::Left)
                    .val(BorderType::Single)
                    .size(4)
                    .space(4)
                    .color(CODE_BORDER),
            )
            .set(
                ParagraphBorder::new(ParagraphBorderPosition::Right)
                    .val(BorderType::Single)
                    .size(4)
                    .space(4)
                    .color(CODE_BORDER),
            );

        for line in code_text.lines() {
            let run = Run::new()
                .add_text(line)
                .fonts(mono_fonts.clone())
                .size(20) // 10pt
                .shading(Shading::new().fill(CODE_BG));
            let mut para = Paragraph::new()
                .add_run(run)
                .line_spacing(LineSpacing::new().before(0).after(0).line(240));
            // Apply paragraph-level shading and borders.
            para.property = para
                .property
                .shading(Shading::new().fill(CODE_BG))
                .set_borders(code_borders.clone());
            *docx = std::mem::take(docx).add_paragraph(para);
        }
    }

    /// Render a diagram to PNG and embed it as an inline image.
    fn render_diagram(
        &self,
        docx: &mut Docx,
        arena: &Arena,
        source: &str,
        node_ref: NodeRef,
    ) -> Result<()> {
        let diagram = as_extension_data!(arena, node_ref, crate::diagram::Diagram);
        let src = crate::diagram::collect_diagram_source(diagram, source);

        let png_result: Result<Vec<u8>> = match diagram.diagram_type() {
            crate::diagram::DiagramType::Mermaid => {
                let themed = crate::diagram::apply_mermaid_theme(
                    &src,
                    self.diagram_themes.mermaid.as_deref(),
                );
                crate::diagram::cached_render_bytes(
                    self.cache,
                    themed.as_bytes(),
                    "mermaid.png",
                    || {
                        crate::diagram::render_mermaid_png(None, &themed)
                            .map_err(|e| anyhow!("mermaid PNG rendering: {e}"))
                    },
                )
            }
            crate::diagram::DiagramType::PlantUml => {
                let themed = crate::diagram::apply_plantuml_theme(
                    &src,
                    self.diagram_themes.plantuml.as_deref(),
                );
                crate::diagram::cached_render_bytes(
                    self.cache,
                    themed.as_bytes(),
                    "plantuml.png",
                    || {
                        crate::diagram::render_plantuml_png(None, themed.as_bytes())
                            .map_err(|e| anyhow!("plantuml PNG rendering: {e}"))
                    },
                )
            }
            crate::diagram::DiagramType::D2 => {
                let themed =
                    crate::diagram::apply_d2_theme(&src, self.diagram_themes.d2.as_deref());
                crate::diagram::cached_render_bytes(self.cache, themed.as_bytes(), "d2.png", || {
                    crate::diagram::render_d2_png(None, &themed)
                        .map_err(|e| anyhow!("D2 PNG rendering: {e}"))
                })
            }
            crate::diagram::DiagramType::Typst => {
                crate::diagram::cached_render_bytes(self.cache, src.as_bytes(), "typst.png", || {
                    crate::diagram::render_typst_png(None, &src)
                        .map_err(|e| anyhow!("Typst PNG rendering: {e}"))
                })
            }
            crate::diagram::DiagramType::DrawIo => {
                if let Some(path) = diagram.drawio_path() {
                    let cache_key = crate::diagram::drawio_cache_key(path, diagram.drawio_page());
                    crate::diagram::cached_render_bytes(
                        self.cache,
                        &cache_key,
                        "drawio.png",
                        || {
                            crate::diagram::render_drawio(
                                None,
                                path,
                                diagram.drawio_page(),
                                crate::diagram::DrawioFormat::Png,
                            )
                            .map_err(|e| anyhow!("Draw.io PNG rendering: {e}"))
                        },
                    )
                } else {
                    return Ok(());
                }
            }
        };

        match png_result {
            Ok(png_data) => {
                if let Some(pic) = self.pic_from_png(&png_data) {
                    let para = Paragraph::new().add_run(Run::new().add_image(pic));
                    *docx = std::mem::take(docx).add_paragraph(para);
                } else {
                    self.logger
                        .log("warning: failed to create DOCX image from diagram PNG");
                }
            }
            Err(e) => {
                self.logger
                    .log(format!("warning: diagram rendering failed: {e}"));
                let para = Paragraph::new().add_run(
                    Run::new()
                        .add_text(format!("[Diagram rendering error: {e}]"))
                        .italic()
                        .color("FF0000"),
                );
                *docx = std::mem::take(docx).add_paragraph(para);
            }
        }

        Ok(())
    }

    fn build_default_header(&self, title: &str) -> Header {
        let header_border = ParagraphBorders::with_empty().set(
            ParagraphBorder::new(ParagraphBorderPosition::Bottom)
                .val(BorderType::Single)
                .size(4)
                .space(1)
                .color(ACCENT_ORANGE),
        );

        let mut para = Paragraph::new();
        para.property = para
            .property
            .style("Header")
            .set_borders(header_border)
            .add_tab(Tab::new().val(TabValueType::Center).pos(4680))
            .add_tab(Tab::new().val(TabValueType::Right).pos(9360));

        let run_style = Run::new().color(ACCENT_ORANGE).size(18);

        para = para
            .add_run(run_style.clone().add_tab())
            .add_run(run_style.clone().add_tab())
            .add_run(
                run_style
                    .clone()
                    .add_field_char(FieldCharType::Begin, false),
            )
            .add_run(run_style.clone().add_instr_text(InstrText::Unsupported(
                " TITLE  \\* MERGEFORMAT ".to_string(),
            )))
            .add_run(
                run_style
                    .clone()
                    .add_field_char(FieldCharType::Separate, false),
            );

        if !title.is_empty() {
            para = para.add_run(run_style.clone().add_text(title));
        }

        para = para.add_run(run_style.add_field_char(FieldCharType::End, false));

        Header::new().add_paragraph(para)
    }

    fn build_default_footer(&self, version_str: &str) -> Footer {
        let footer_border = ParagraphBorders::with_empty().set(
            ParagraphBorder::new(ParagraphBorderPosition::Top)
                .val(BorderType::Single)
                .size(4)
                .space(1)
                .color(ACCENT_ORANGE),
        );

        let mut para = Paragraph::new();
        para.property = para
            .property
            .style("Footer")
            .set_borders(footer_border)
            .add_tab(Tab::new().val(TabValueType::Center).pos(4680))
            .add_tab(Tab::new().val(TabValueType::Right).pos(9360));

        let run_style = Run::new().color(ACCENT_ORANGE).size(18);

        para = para
            .add_run(
                run_style
                    .clone()
                    .add_field_char(FieldCharType::Begin, false),
            )
            .add_run(run_style.clone().add_instr_text(InstrText::Unsupported(
                " DOCPROPERTY Version \\* MERGEFORMAT ".to_string(),
            )))
            .add_run(
                run_style
                    .clone()
                    .add_field_char(FieldCharType::Separate, false),
            );

        if !version_str.is_empty() {
            para = para.add_run(run_style.clone().add_text(version_str));
        }

        para = para
            .add_run(run_style.clone().add_field_char(FieldCharType::End, false))
            .add_run(run_style.clone().add_tab())
            .add_run(run_style.clone().add_tab())
            .add_run(
                run_style
                    .clone()
                    .add_field_char(FieldCharType::Begin, false),
            )
            .add_run(
                run_style
                    .clone()
                    .add_instr_text(InstrText::PAGE(InstrPAGE::new())),
            )
            .add_run(
                run_style
                    .clone()
                    .add_field_char(FieldCharType::Separate, false),
            )
            .add_run(run_style.clone().add_text("1"))
            .add_run(run_style.add_field_char(FieldCharType::End, false));

        Footer::new().add_paragraph(para)
    }

    fn resolve_front_logo(&self) -> Option<Pic> {
        let (bytes, ext) = self.logo_asset.as_ref()?;
        let ext = ext.to_ascii_lowercase();

        let pic = if ext == "svg" {
            let png = self.svg_to_png(bytes)?;
            self.pic_from_png(&png)?
        } else if ext == "png" {
            self.pic_from_png(bytes)?
        } else {
            self.pic_from_raster(bytes)?
        };

        Some(pic.size(FRONT_LOGO_WIDTH_EMU, FRONT_LOGO_HEIGHT_EMU))
    }

    fn svg_to_png(&self, svg_data: &[u8]) -> Option<Vec<u8>> {
        let opt = usvg::Options::default();
        let tree = usvg::Tree::from_data(svg_data, &opt).ok()?;
        let size = tree.size().to_int_size();
        let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())?;
        let mut pmut = pixmap.as_mut();
        resvg::render(&tree, tiny_skia::Transform::default(), &mut pmut);
        pixmap.encode_png().ok()
    }

    /// Look up the graph-resolved image target for an `Image` node and
    /// load the file as a `Pic`.
    fn resolve_graph_image(&self, context: &RenderContext<'_>, node_ref: NodeRef) -> Option<Pic> {
        let target = context.graph.image_target(context.doc_id, node_ref)?;
        self.resolve_image(target)
    }

    /// Resolve an image from the graph-resolved target path and load it as
    /// a `Pic`.
    ///
    /// `target` is the value returned by `graph.image_target(doc_id,
    /// node_ref)` — typically `static/<relative-to-base-dir>`.  We strip
    /// the `static/` prefix and join with `base_dir` to locate the file
    /// on disk.
    ///
    /// Falls back to treating `target` as a raw relative path against
    /// `base_dir` when the prefix is absent.
    fn resolve_image(&self, target: &str) -> Option<Pic> {
        if target.contains("://") || target.is_empty() {
            return None;
        }

        let rel = target.strip_prefix("static/").unwrap_or(target);
        let path = self.base_dir.join(rel);

        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                self.logger.log(format!(
                    "warning: cannot read image {}: {e}",
                    path.display()
                ));
                return None;
            }
        };

        if data.is_empty() {
            return None;
        }

        // SVG images are not supported by the DOCX `image` crate — skip
        // with a warning.
        let is_svg = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("svg"))
            .unwrap_or(false);
        if is_svg {
            self.logger.log(format!(
                "warning: SVG images are not supported in DOCX output, skipping {}",
                path.display()
            ));
            return None;
        }

        // For PNG data we can use our fast IHDR-based dimension parser.
        // For other raster formats (JPEG, GIF, …) `Pic::new` from
        // docx-rs runs the full `image` crate decoder (which also
        // converts the data to PNG internally).
        if detect_png_dimensions(&data).is_some() {
            self.pic_from_png(&data)
        } else {
            self.pic_from_raster(&data)
        }
    }

    /// Create a `Pic` from a non-PNG raster image (JPEG, GIF, etc.).
    ///
    /// Uses `Pic::new` which internally decodes via the `image` crate and
    /// re-encodes to PNG.  We then apply the `MAX_IMAGE_WIDTH_EMU` cap.
    fn pic_from_raster(&self, data: &[u8]) -> Option<Pic> {
        // `Pic::new` panics on unsupported formats — catch that.
        let result = std::panic::catch_unwind(|| Pic::new(data));
        match result {
            Ok(pic) => {
                // Apply our width cap (Pic stores size as (w_emu, h_emu)).
                let (w_emu, h_emu) = pic.size;
                let (final_w, final_h) = if w_emu > MAX_IMAGE_WIDTH_EMU {
                    let scale = MAX_IMAGE_WIDTH_EMU as f64 / w_emu as f64;
                    (MAX_IMAGE_WIDTH_EMU, (h_emu as f64 * scale) as u32)
                } else {
                    (w_emu, h_emu)
                };
                Some(pic.size(final_w, final_h))
            }
            Err(_) => {
                self.logger
                    .log("warning: failed to decode image for DOCX embedding");
                None
            }
        }
    }

    /// Create a `Pic` from raw image bytes with size constraints.
    fn pic_from_png(&self, data: &[u8]) -> Option<Pic> {
        if data.is_empty() {
            return None;
        }
        let (w_px, h_px) = detect_png_dimensions(data).unwrap_or((800, 600));
        let w_emu = w_px * EMU_PER_PX;
        let h_emu = h_px * EMU_PER_PX;

        let (final_w, final_h) = if w_emu > MAX_IMAGE_WIDTH_EMU {
            let scale = MAX_IMAGE_WIDTH_EMU as f64 / w_emu as f64;
            (MAX_IMAGE_WIDTH_EMU, (h_emu as f64 * scale) as u32)
        } else {
            (w_emu, h_emu)
        };

        Some(Pic::new(data).size(final_w, final_h))
    }
}

/// Default admonition title for a given type.
fn admonition_default_title(adm_type: &str) -> &'static str {
    match adm_type {
        "note" => "Note",
        "warning" => "Warning",
        "important" => "Important",
        "caution" => "Caution",
        "tip" => "Tip",
        "related" => "Related",
        _ => "Note",
    }
}

struct TocEntry {
    key: String,
    text: String,
    level: usize,
}

/// Detect PNG image dimensions from the IHDR chunk.
fn detect_png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 {
        return None;
    }
    if &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((width, height))
}
