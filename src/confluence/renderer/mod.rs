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
use std::path::Path;

use anyhow::Result;

use crate::cache::Cache;
use crate::conditional::RenderProfile;
use crate::diagram::DiagramThemes;
use crate::graph::PreparedDocument;
use crate::DebugDumper;

mod attachments;
mod diagrams;
mod footnotes;
mod render_blocks;
mod render_inline;
mod types;
mod xml;

#[cfg(test)]
mod tests;

pub(crate) use types::{ConfluenceAttachment, ConfluenceDocument};

/// Default width in pixels for block-level SVG images/diagrams in Confluence.
/// This approximates the typical Confluence content area width.
const SVG_DEFAULT_BLOCK_WIDTH: u32 = 800;

/// Width in pixels for inline SVG images (icon-sized).
const SVG_INLINE_WIDTH: u32 = 16;

pub(crate) struct ConfluenceRenderer<'a> {
    cache: &'a Cache,
    diagram_themes: &'a DiagramThemes,
    logger: &'a DebugDumper,
    base_dir: &'a Path,
    profile: RenderProfile,
    attachments: RefCell<Vec<ConfluenceAttachment>>,
    /// Maps (original absolute path) -> filename already added, to avoid
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
        profile: RenderProfile,
    ) -> Self {
        Self {
            cache,
            diagram_themes,
            logger,
            base_dir,
            profile,
            attachments: RefCell::new(Vec::new()),
            attachment_names: RefCell::new(HashMap::new()),
            diagram_counter: RefCell::new(0),
        }
    }

    pub fn render(self, prepared: &PreparedDocument) -> Result<ConfluenceDocument> {
        let mut out = String::new();

        // Collect footnotes across the entire document graph.
        let footnotes = self.collect_footnotes(prepared)?;

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
}
