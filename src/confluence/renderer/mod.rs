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
use rushdown::ast::{KindData, NodeRef};
use rushdown_fenced_div::FencedDiv;

use crate::cache::Cache;
use crate::conditional::RenderProfile;
use crate::diagram::DiagramThemes;
use crate::graph::PreparedDocument;
use crate::DebugDumper;

use self::types::{FootnoteEntry, RenderContext, Scope};

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

#[derive(Clone, Copy)]
struct FirstRenderNode {
    doc_id: usize,
    node_ref: NodeRef,
    heading_shift: usize,
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

        let mut skip_first_rendered_block = false;
        if let Some(first) =
            self.find_first_visible_render_node(prepared, prepared.graph().root_id, 0)?
        {
            if self.first_node_should_precede_toc(prepared, first) {
                self.render_first_node(&mut out, prepared, first, &footnotes)?;
                skip_first_rendered_block = true;
            }
        }

        // Add native Confluence TOC near the top of the page.
        out.push_str("<ac:structured-macro ac:name=\"toc\"></ac:structured-macro>\n");

        self.render_graph_document(
            &mut out,
            prepared,
            prepared.graph().root_id,
            0,
            &footnotes,
            &mut skip_first_rendered_block,
        )?;

        // Render footnotes section at the end.
        if !footnotes.is_empty() {
            self.render_footnotes_section(&mut out, prepared, &footnotes)?;
        }

        Ok(ConfluenceDocument {
            body: out,
            attachments: self.attachments.into_inner(),
        })
    }

    fn find_first_visible_render_node(
        &self,
        prepared: &PreparedDocument,
        doc_id: usize,
        heading_shift: usize,
    ) -> Result<Option<FirstRenderNode>> {
        let doc = prepared.graph().document(doc_id);
        for child in doc.arena[doc.doc_ref].children(&doc.arena) {
            if !crate::conditional::node_is_visible(&doc.arena, child, &doc.source, &self.profile)?
            {
                continue;
            }
            if let Some(entries) = prepared.graph().include_entries(doc_id, child) {
                for entry in entries {
                    if let Some(found) = self.find_first_visible_render_node(
                        prepared,
                        entry.doc_id,
                        heading_shift + entry.heading_shift,
                    )? {
                        return Ok(Some(found));
                    }
                }
                continue;
            }
            return Ok(Some(FirstRenderNode {
                doc_id,
                node_ref: child,
                heading_shift,
            }));
        }
        Ok(None)
    }

    fn first_node_should_precede_toc(
        &self,
        prepared: &PreparedDocument,
        first: FirstRenderNode,
    ) -> bool {
        let doc = prepared.graph().document(first.doc_id);
        if !matches!(
            doc.arena[first.node_ref].kind_data(),
            KindData::Extension(_)
        ) {
            return false;
        }
        if !rushdown::matches_extension_kind!(&doc.arena, first.node_ref, FencedDiv) {
            return false;
        }
        let classes = doc.arena[first.node_ref]
            .attributes()
            .get("class")
            .map(|v| v.str(&doc.source))
            .unwrap_or_default();
        matches!(
            crate::admonition_type(&classes),
            Some("warning" | "caution" | "important")
        )
    }

    fn render_first_node(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        first: FirstRenderNode,
        footnotes: &[FootnoteEntry],
    ) -> Result<()> {
        let doc = prepared.graph().document(first.doc_id);
        let context = RenderContext {
            prepared,
            scope: Scope {
                doc_id: first.doc_id,
            },
            arena: &doc.arena,
            source: &doc.source,
            heading_shift: first.heading_shift,
            footnotes,
        };
        self.render_block(out, &context, first.node_ref)
    }
}
