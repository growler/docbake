//! DOCX renderer for Google Docs publishing.
//!
//! Walks the document graph and produces a DOCX file using the `docx-rs`
//! crate.  The output is designed for upload to Google Drive with automatic
//! conversion to native Google Docs format.
//!
//! Diagrams are rendered to PNG and embedded as inline images.
//! Regular images are read from disk and embedded directly.

use std::cell::RefCell;
use std::io::Cursor;
use std::path::Path;

use anyhow::{anyhow, Result};
use docx_rs::*;
use rushdown::as_extension_data;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, KindData, NodeRef, TextQualifier};
use rushdown_definition_list::{DefinitionList, Term, TermDefinition};
use rushdown_fenced_div::FencedDiv;

use crate::cache::Cache;
use crate::diagram::DiagramThemes;
use crate::graph::PreparedDocument;
use crate::DebugDumper;

/// Numbering ID for unordered (bullet) lists.
const BULLET_NUMBERING_ID: usize = 1;
/// Numbering ID for ordered (decimal) lists.
const ORDERED_NUMBERING_ID: usize = 2;

/// EMU per pixel at 96 DPI (1 inch = 914400 EMU, 1 inch = 96 px).
const EMU_PER_PX: u32 = 9525;

/// Maximum image width in EMU (~6 inches at 96 DPI).
const MAX_IMAGE_WIDTH_EMU: u32 = 5_760_000;

pub(crate) struct DocxRenderer<'a> {
    cache: &'a Cache,
    diagram_themes: &'a DiagramThemes,
    logger: &'a DebugDumper,
    base_dir: &'a Path,
    #[allow(dead_code)]
    diagram_counter: RefCell<u32>,
}

impl<'a> DocxRenderer<'a> {
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
            diagram_counter: RefCell::new(0),
        }
    }

    /// Render the document graph to DOCX bytes.
    pub fn render(&self, prepared: &PreparedDocument) -> Result<Vec<u8>> {
        let graph = prepared.graph();

        let mut docx = Docx::new();

        // Define list numbering styles.
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

        // Walk the document graph in rendered order.
        for (doc_id, node_ref, heading_shift) in graph.rendered_nodes_preorder() {
            let doc = graph.document(doc_id);
            self.render_block(
                &mut docx,
                &doc.arena,
                &doc.source,
                node_ref,
                heading_shift,
                doc_id,
                graph,
            )?;
        }

        // Serialize to bytes.
        let mut buf = Cursor::new(Vec::new());
        docx.build()
            .pack(&mut buf)
            .map_err(|e| anyhow!("failed to pack DOCX: {e}"))?;
        Ok(buf.into_inner())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_block(
        &self,
        docx: &mut Docx,
        arena: &Arena,
        source: &str,
        node_ref: NodeRef,
        heading_shift: usize,
        doc_id: usize,
        graph: &crate::graph::DocumentGraph,
    ) -> Result<()> {
        match arena[node_ref].kind_data() {
            KindData::Heading(_) => {
                let heading = as_kind_data!(arena[node_ref], Heading);
                let level = (heading.level() as usize + heading_shift).min(6);
                let style = format!("Heading{level}");

                let mut para = Paragraph::new().style(&style);
                for child in arena[node_ref].children(arena) {
                    let runs = self.render_inline_runs(arena, source, child, doc_id, graph);
                    for run in runs {
                        para = para.add_run(run);
                    }
                }
                *docx = std::mem::take(docx).add_paragraph(para);
            }
            KindData::Paragraph(_) => {
                let mut para = Paragraph::new();
                for child in arena[node_ref].children(arena) {
                    match arena[child].kind_data() {
                        KindData::Image(image) => {
                            let dest = image.destination_str(source);
                            if let Some(pic) = self.resolve_image(dest) {
                                para = para.add_run(Run::new().add_image(pic));
                            }
                        }
                        _ => {
                            let runs =
                                self.render_inline_runs(arena, source, child, doc_id, graph);
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
                let mono_fonts = RunFonts::new()
                    .ascii("Courier New")
                    .hi_ansi("Courier New")
                    .east_asia("Courier New")
                    .cs("Courier New");

                // Render each line as a separate paragraph with monospace font.
                for line in code_text.lines() {
                    let run = Run::new()
                        .add_text(line)
                        .fonts(mono_fonts.clone())
                        .size(20); // 10pt (half-points)
                    let para = Paragraph::new()
                        .add_run(run)
                        .line_spacing(LineSpacing::new().line(240));
                    *docx = std::mem::take(docx).add_paragraph(para);
                }
            }
            KindData::Blockquote(_) => {
                // Render blockquote children with indent.
                for child in arena[node_ref].children(arena) {
                    self.render_block(
                        docx,
                        arena,
                        source,
                        child,
                        heading_shift,
                        doc_id,
                        graph,
                    )?;
                }
            }
            KindData::List(list) => {
                let is_ordered = list.is_ordered();
                let numbering_id = if is_ordered {
                    ORDERED_NUMBERING_ID
                } else {
                    BULLET_NUMBERING_ID
                };

                self.render_list_items(
                    docx,
                    arena,
                    source,
                    node_ref,
                    numbering_id,
                    0,
                    heading_shift,
                    doc_id,
                    graph,
                )?;
            }
            KindData::Table(_) => {
                self.render_table(docx, arena, source, node_ref, doc_id, graph)?;
            }
            KindData::ThematicBreak(_) => {
                // Insert an empty paragraph as a separator.
                let para = Paragraph::new().add_run(Run::new().add_text(""));
                *docx = std::mem::take(docx).add_paragraph(para);
            }
            _ => {
                // Handle extension nodes (diagrams, fenced divs, etc.)
                if rushdown::matches_extension_kind!(arena, node_ref, crate::diagram::Diagram) {
                    self.render_diagram(docx, arena, source, node_ref)?;
                } else if rushdown::matches_extension_kind!(arena, node_ref, FencedDiv) {
                    // Render fenced div children as normal blocks.
                    for child in arena[node_ref].children(arena) {
                        self.render_block(
                            docx,
                            arena,
                            source,
                            child,
                            heading_shift,
                            doc_id,
                            graph,
                        )?;
                    }
                } else if rushdown::matches_extension_kind!(arena, node_ref, DefinitionList) {
                    self.render_definition_list(docx, arena, source, node_ref, doc_id, graph)?;
                }
                // FootnoteDefinition nodes are rendered inline at reference sites — skip here.
            }
        }
        Ok(())
    }

    /// Render an inline node to one or more `Run`s.
    fn render_inline_runs(
        &self,
        arena: &Arena,
        source: &str,
        node_ref: NodeRef,
        doc_id: usize,
        graph: &crate::graph::DocumentGraph,
    ) -> Vec<Run> {
        match arena[node_ref].kind_data() {
            KindData::Text(text) => {
                let content = text.str(source);
                let mut runs = vec![Run::new().add_text(content)];
                // Handle soft/hard breaks that may be attached as qualifiers.
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
                vec![Run::new().add_text(&*content).fonts(mono_fonts)]
            }
            KindData::Emphasis(emphasis) if emphasis.level() == 1 => {
                // Italic (emph)
                let children_runs =
                    self.collect_child_runs(arena, source, node_ref, doc_id, graph);
                children_runs.into_iter().map(|r| r.italic()).collect()
            }
            KindData::Emphasis(_) => {
                // Bold (strong)
                let children_runs =
                    self.collect_child_runs(arena, source, node_ref, doc_id, graph);
                children_runs.into_iter().map(|r| r.bold()).collect()
            }
            KindData::Strikethrough(_) => {
                let children_runs =
                    self.collect_child_runs(arena, source, node_ref, doc_id, graph);
                children_runs.into_iter().map(|r| r.strike()).collect()
            }
            KindData::Link(link) => {
                let text = self.collect_inline_text(arena, source, node_ref);
                // Use the graph-resolved link target if available.
                let _dest = graph
                    .link_target(doc_id, node_ref)
                    .unwrap_or(link.destination_str(source));
                // DOCX hyperlinks require special handling via Paragraph::add_hyperlink,
                // but since we're returning Runs, render as colored underlined text.
                vec![Run::new()
                    .add_text(&text)
                    .color("0563C1")
                    .underline("single")]
            }
            KindData::Image(image) => {
                let dest = image.destination_str(source);
                if let Some(pic) = self.resolve_image(dest) {
                    vec![Run::new().add_image(pic)]
                } else {
                    let alt = self.collect_inline_text(arena, source, node_ref);
                    if alt.is_empty() {
                        vec![]
                    } else {
                        vec![Run::new().add_text(&alt)]
                    }
                }
            }
            _ => {
                // For other inline nodes, collect text from children.
                self.collect_child_runs(arena, source, node_ref, doc_id, graph)
            }
        }
    }

    /// Collect `Run`s from all children of a node.
    fn collect_child_runs(
        &self,
        arena: &Arena,
        source: &str,
        node_ref: NodeRef,
        doc_id: usize,
        graph: &crate::graph::DocumentGraph,
    ) -> Vec<Run> {
        let mut runs = Vec::new();
        for child in arena[node_ref].children(arena) {
            runs.extend(self.render_inline_runs(arena, source, child, doc_id, graph));
        }
        runs
    }

    /// Recursively collect plain text from an inline subtree.
    fn collect_inline_text(&self, arena: &Arena, source: &str, node_ref: NodeRef) -> String {
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
                    buf.push_str(&self.collect_inline_text(arena, source, child));
                }
            }
        }
        buf
    }

    /// Render list items recursively, handling nested lists.
    #[allow(clippy::too_many_arguments)]
    fn render_list_items(
        &self,
        docx: &mut Docx,
        arena: &Arena,
        source: &str,
        list_ref: NodeRef,
        numbering_id: usize,
        indent_level: usize,
        heading_shift: usize,
        doc_id: usize,
        graph: &crate::graph::DocumentGraph,
    ) -> Result<()> {
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
                            let runs =
                                self.render_inline_runs(arena, source, inline, doc_id, graph);
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
                        self.render_list_items(
                            docx,
                            arena,
                            source,
                            child,
                            nested_id,
                            indent_level + 1,
                            heading_shift,
                            doc_id,
                            graph,
                        )?;
                    }
                    _ => {
                        self.render_block(
                            docx,
                            arena,
                            source,
                            child,
                            heading_shift,
                            doc_id,
                            graph,
                        )?;
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
        arena: &Arena,
        source: &str,
        table_ref: NodeRef,
        doc_id: usize,
        graph: &crate::graph::DocumentGraph,
    ) -> Result<()> {
        let mut header_rows = Vec::new();
        let mut body_rows = Vec::new();

        // Collect header and body rows from the table sections.
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

        // Render header rows (bold text).
        for row_ref in &header_rows {
            let cells = self.render_table_row(docx, arena, source, *row_ref, doc_id, graph, true)?;
            rows.push(TableRow::new(cells));
        }

        // Render body rows.
        for row_ref in &body_rows {
            let cells =
                self.render_table_row(docx, arena, source, *row_ref, doc_id, graph, false)?;
            rows.push(TableRow::new(cells));
        }

        if !rows.is_empty() {
            let table = Table::new(rows);
            *docx = std::mem::take(docx).add_table(table);
            // DOCX requires a paragraph after every table.
            *docx = std::mem::take(docx).add_paragraph(Paragraph::new());
        }

        Ok(())
    }

    /// Render a single table row, returning a Vec of cells.
    fn render_table_row(
        &self,
        _docx: &mut Docx,
        arena: &Arena,
        source: &str,
        row_ref: NodeRef,
        doc_id: usize,
        graph: &crate::graph::DocumentGraph,
        is_header: bool,
    ) -> Result<Vec<TableCell>> {
        let mut cells = Vec::new();
        for cell_ref in arena[row_ref].children(arena) {
            if !matches!(arena[cell_ref].kind_data(), KindData::TableCell(_)) {
                continue;
            }

            let mut para = Paragraph::new();
            for child in arena[cell_ref].children(arena) {
                let mut runs = self.render_inline_runs(arena, source, child, doc_id, graph);
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
        arena: &Arena,
        source: &str,
        node_ref: NodeRef,
        doc_id: usize,
        graph: &crate::graph::DocumentGraph,
    ) -> Result<()> {
        for child in arena[node_ref].children(arena) {
            if rushdown::matches_extension_kind!(arena, child, Term) {
                // Term — render as bold paragraph.
                let mut para = Paragraph::new();
                for inline in arena[child].children(arena) {
                    let runs = self.render_inline_runs(arena, source, inline, doc_id, graph);
                    for run in runs {
                        para = para.add_run(run.bold());
                    }
                }
                *docx = std::mem::take(docx).add_paragraph(para);
            } else if rushdown::matches_extension_kind!(arena, child, TermDefinition) {
                // Definition — render as indented paragraph.
                for def_child in arena[child].children(arena) {
                    if matches!(arena[def_child].kind_data(), KindData::Paragraph(_)) {
                        let mut para = Paragraph::new().indent(Some(720), None, None, None);
                        for inline in arena[def_child].children(arena) {
                            let runs =
                                self.render_inline_runs(arena, source, inline, doc_id, graph);
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
                crate::diagram::cached_render_bytes(
                    self.cache,
                    themed.as_bytes(),
                    "d2.png",
                    || {
                        crate::diagram::render_d2_png(None, &themed)
                            .map_err(|e| anyhow!("D2 PNG rendering: {e}"))
                    },
                )
            }
            crate::diagram::DiagramType::Typst => crate::diagram::cached_render_bytes(
                self.cache,
                src.as_bytes(),
                "typst.png",
                || {
                    crate::diagram::render_typst_png(None, &src)
                        .map_err(|e| anyhow!("Typst PNG rendering: {e}"))
                },
            ),
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
                    return Ok(()); // no path, skip
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
                // Insert error text as fallback.
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

    /// Resolve an image path and load it as a `Pic`.
    fn resolve_image(&self, dest: &str) -> Option<Pic> {
        // Skip external URLs — can't embed them in DOCX directly.
        if dest.contains("://") {
            return None;
        }
        if dest.is_empty() {
            return None;
        }

        let path = if dest.starts_with('/') {
            self.base_dir.join(dest.trim_start_matches('/'))
        } else {
            self.base_dir.join(dest)
        };

        let data = std::fs::read(&path).ok()?;
        self.pic_from_png(&data)
    }

    /// Create a `Pic` from raw image bytes with size constraints.
    fn pic_from_png(&self, data: &[u8]) -> Option<Pic> {
        if data.is_empty() {
            return None;
        }
        // Try to detect PNG dimensions from header.
        let (w_px, h_px) = detect_png_dimensions(data).unwrap_or((800, 600));

        let w_emu = w_px * EMU_PER_PX;
        let h_emu = h_px * EMU_PER_PX;

        // Scale down if wider than max.
        let (final_w, final_h) = if w_emu > MAX_IMAGE_WIDTH_EMU {
            let scale = MAX_IMAGE_WIDTH_EMU as f64 / w_emu as f64;
            (MAX_IMAGE_WIDTH_EMU, (h_emu as f64 * scale) as u32)
        } else {
            (w_emu, h_emu)
        };

        Some(Pic::new(data).size(final_w, final_h))
    }
}

/// Detect PNG image dimensions from the IHDR chunk.
fn detect_png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    // PNG signature: 8 bytes, then IHDR chunk.
    // IHDR starts at byte 8: 4 bytes length, 4 bytes "IHDR", then 4 bytes width, 4 bytes height.
    if data.len() < 24 {
        return None;
    }
    // Check PNG signature.
    if &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((width, height))
}
