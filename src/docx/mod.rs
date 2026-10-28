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
//! Diagrams are rendered to PNG and embedded as inline images.  Before the
//! body walk, the diagrams that the artifact cache does not hold are
//! rendered in parallel into the cache.
//! Regular images are read from disk and embedded directly.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Cursor;
use std::path::Path;

use anyhow::{anyhow, Result};
use docx_rs::*;
use rushdown::ast::Arena;

use crate::cache::Cache;
use crate::conditional::RenderProfile;
use crate::diagram::DiagramThemes;
use crate::graph::{DocumentGraph, PreparedDocument};
use crate::DebugDumper;

use self::title_page::TitleFields;

mod admonition;
mod blocks;
mod header_footer;
mod inline;
mod media;
mod numbering;
#[allow(dead_code)] // The renderer does not read these parameters yet.
pub(crate) mod params;
mod prefetch;
mod tables;
mod title_page;
mod toc;

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
    profile: RenderProfile,
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
        profile: RenderProfile,
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
            profile,
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
        docx = numbering::add_list_numbering(docx);

        // ── Title page ──────────────────────────────────────────────────
        let fields = TitleFields::from_metadata(&metadata);

        if !fields.version.is_empty() {
            docx = docx.custom_property("Version", &fields.version);
        }
        if !fields.title.is_empty() {
            docx = docx.header(self.build_default_header(fields.title));
        } else {
            docx = docx.header(self.build_default_header(""));
        }
        docx = docx
            .footer(self.build_default_footer(&fields.version))
            .title_pg();

        let has_title_page = !fields.title.is_empty();
        if has_title_page {
            docx = self.add_title_page(docx, &fields);
        }

        docx = self.add_table_of_contents(docx, graph)?;

        self.prefetch_diagrams(graph);

        // ── Walk the document graph (top-level children only) ───────────
        // Mirror the Typst renderer approach: iterate document-level
        // children, resolve includes recursively, call render_block for
        // each top-level block.  render_block handles its own children.
        docx = self.render_graph_document(docx, graph, graph.root_id, 0)?;

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
        mut docx: Docx,
        graph: &DocumentGraph,
        doc_id: usize,
        heading_shift: usize,
    ) -> Result<Docx> {
        let doc = graph.document(doc_id);
        for child in doc.arena[doc.doc_ref].children(&doc.arena) {
            if !crate::conditional::node_is_visible(&doc.arena, child, &doc.source, &self.profile)?
            {
                continue;
            }
            if let Some(entries) = graph.include_entries(doc_id, child) {
                for entry in entries {
                    docx = self.render_graph_document(
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
            docx = self.render_block(docx, &context, child)?;
        }
        Ok(docx)
    }
}

#[cfg(test)]
mod tests;
