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
use std::path::Path;

use anyhow::Result;
use docx_rs::*;
use rushdown::ast::Arena;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::cache::Cache;
use crate::conditional::RenderProfile;
use crate::diagram::DiagramThemes;
use crate::graph::{DocumentGraph, PreparedDocument};
use crate::DebugDumper;

use self::package::Package;
use self::params::DocxParams;
use self::tables::TableLayout;
use self::title_page::TitleFields;

mod admonition;
mod blocks;
mod doc_props;
mod fields;
mod font_table;
mod header_footer;
mod inline;
mod lang;
mod media;
mod numbering;
mod package;
pub(crate) mod params;
mod prefetch;
mod stylesheet;
mod tables;
mod title_page;
mod toc;

pub(crate) struct DocxRenderer<'a> {
    cache: &'a Cache,
    diagram_themes: &'a DiagramThemes,
    logger: &'a DebugDumper,
    base_dir: &'a Path,
    logo_asset: Option<(Vec<u8>, String)>,
    params: &'a DocxParams,
    #[allow(dead_code)]
    diagram_counter: RefCell<u32>,
    toc_heading_keys: RefCell<VecDeque<String>>,
    bookmark_counter: RefCell<usize>,
    /// The `w:numId` of the next list.
    list_num_counter: RefCell<usize>,
    /// The layout of each rendered table, in document order.
    tables: RefCell<Vec<TableLayout>>,
    profile: RenderProfile,
}

#[derive(Clone, Copy)]
struct RenderContext<'a> {
    arena: &'a Arena,
    source: &'a str,
    doc_id: usize,
    heading_shift: usize,
    graph: &'a DocumentGraph,
    /// The style of the body paragraphs in the block that is open, for
    /// example `Quote` in a block quote. `None` gives the `Normal` style.
    paragraph_style: Option<&'static str>,
}

impl<'a> DocxRenderer<'a> {
    pub fn new(
        cache: &'a Cache,
        diagram_themes: &'a DiagramThemes,
        logger: &'a DebugDumper,
        base_dir: &'a Path,
        logo_asset: Option<(Vec<u8>, String)>,
        params: &'a DocxParams,
        profile: RenderProfile,
    ) -> Self {
        Self {
            cache,
            diagram_themes,
            logger,
            base_dir,
            logo_asset,
            params,
            diagram_counter: RefCell::new(0),
            toc_heading_keys: RefCell::new(VecDeque::new()),
            bookmark_counter: RefCell::new(0),
            list_num_counter: RefCell::new(numbering::FIRST_LIST_NUM_ID),
            tables: RefCell::new(Vec::new()),
            profile,
        }
    }

    /// Render the document graph to a DOCX package. `metadata` is the
    /// document metadata after the command-line changes.
    pub fn render(
        &self,
        prepared: &PreparedDocument,
        metadata: &JsonMap<String, JsonValue>,
    ) -> Result<Package> {
        let graph = prepared.graph();
        let lang = lang::document_lang(metadata)?;

        // ── Page size and margins ───────────────────────────────────────
        let page = &self.params.page;
        let (width, height) = page.size.dimensions();
        let margins = &page.margins;
        let mut docx = Docx::new().page_size(width, height).page_margin(
            PageMargin::new()
                .top(margins.top.0 as i32)
                .right(margins.right.0 as i32)
                .bottom(margins.bottom.0 as i32)
                .left(margins.left.0 as i32)
                .header(margins.header.0 as i32)
                .footer(margins.footer.0 as i32)
                .gutter(0),
        );

        // ── Define list numbering styles ────────────────────────────────
        docx = numbering::add_list_numbering(docx, &self.params.lists);

        // ── Title page ──────────────────────────────────────────────────
        let fields = TitleFields::from_metadata(metadata);

        if !fields.version.is_empty() {
            docx = docx.custom_property("Version", &fields.version);
        }
        let logo = self.theme_logo();
        docx = docx.header(self.build_default_header(fields.title, logo.as_ref()));
        docx = docx
            .footer(self.build_default_footer(&fields.version))
            .title_pg();

        let has_title_page = !fields.title.is_empty();
        if has_title_page {
            docx = self.add_title_page(docx, &fields, logo.as_ref());
        }

        docx = self.add_table_of_contents(docx, graph)?;

        self.prefetch_diagrams(graph);

        // ── Walk the document graph (top-level children only) ───────────
        // Mirror the Typst renderer approach: iterate document-level
        // children, resolve includes recursively, call render_block for
        // each top-level block.  render_block handles its own children.
        docx = self.render_graph_document(docx, graph, graph.root_id, 0)?;

        Package::new(docx, self.params, lang, fields.title, &self.tables.take())
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
                paragraph_style: None,
            };
            docx = self.render_block(docx, &context, child)?;
        }
        Ok(docx)
    }
}

#[cfg(test)]
mod tests;
