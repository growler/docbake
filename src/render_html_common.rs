use std::any::TypeId;

use rushdown::as_extension_data;
use rushdown::ast::{self, Arena, Document, KindData, NodeRef, WalkStatus};
use rushdown::renderer;
use rushdown::renderer::html;
use rushdown::renderer::{
    BoxRenderNode, NodeRenderer, NodeRendererRegistry, RenderNode, TextWrite,
};
use rushdown_fenced_div::FencedDiv;
use rushdown_footnote::FootnoteDefinition;

/// Returns `true` when the image at `node_ref` is not the sole child of a
/// `Paragraph` — i.e. it appears inline alongside other content.
pub(crate) fn is_inline_image(arena: &Arena, node_ref: NodeRef) -> bool {
    if let Some(parent_ref) = arena[node_ref].parent() {
        matches!(arena[parent_ref].kind_data(), KindData::Paragraph(_))
            && (arena[node_ref].previous_sibling().is_some()
                || arena[node_ref].next_sibling().is_some())
    } else {
        false
    }
}

/// Escape text for safe inclusion in HTML content/attributes.
pub(crate) fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Extract heading text as HTML suitable for TOC links.
/// Text content is HTML-escaped; code spans are wrapped in `<code>`.
pub(crate) fn heading_text_html(arena: &Arena, node_ref: NodeRef, source: &str) -> String {
    let mut html = String::new();
    let _ = ast::walk(arena, node_ref, &mut |arena: &Arena,
                                             child_ref: NodeRef,
                                             entering: bool|
     -> std::result::Result<
        WalkStatus,
        std::io::Error,
    > {
        if !entering || child_ref == node_ref {
            return Ok(WalkStatus::Continue);
        }
        match arena[child_ref].kind_data() {
            KindData::Text(value) => html.push_str(&html_escape(value.str(source))),
            KindData::CodeSpan(cs) => {
                html.push_str("<code>");
                html.push_str(&html_escape(&cs.value_str(source)));
                html.push_str("</code>");
            }
            _ => {}
        }
        Ok(WalkStatus::Continue)
    });
    html
}

/// Returns `true` when the heading at `node_ref` is the first child of an
/// admonition fenced div.  Such headings are rendered as admonition titles
/// rather than structural headings, so they must be excluded from the TOC.
pub(crate) fn is_admonition_title_heading(arena: &Arena, node_ref: NodeRef, source: &str) -> bool {
    if let Some(parent_ref) = arena[node_ref].parent() {
        if rushdown::matches_extension_kind!(arena, parent_ref, FencedDiv) {
            let classes = arena[parent_ref]
                .attributes()
                .get("class")
                .map(|v| v.str(source))
                .unwrap_or_default();
            if crate::admonition_type(&classes).is_some() {
                // Check this heading is the first child.
                return arena[node_ref].previous_sibling().is_none();
            }
        }
    }
    false
}

/// Render the image attribute portion of an `<img>` tag: the `.inline-image`
/// class logic, remaining user-supplied attributes, and the closing `>` or
/// `/>`.
///
/// This is called **after** `src="…" alt="…"` and optional `title="…"` have
/// already been written.  It handles:
/// - Auto-adding the `inline-image` CSS class when the image appears inline.
/// - Emitting the existing `class` attribute value (merged with the auto
///   class when needed).
/// - Emitting all other user-supplied attributes.
/// - Writing the closing `>` (or `/>` when `format_options.xhtml` is set).
pub(crate) fn render_image_attributes<W: TextWrite>(
    w: &mut W,
    source: &str,
    arena: &Arena,
    node_ref: NodeRef,
    format_options: &html::Options,
) -> rushdown::Result<()> {
    // Auto-add .inline-image class when the image is not the sole
    // child of a paragraph (i.e. it appears inline alongside text).
    let attrs = arena[node_ref].attributes();
    let is_inline = is_inline_image(arena, node_ref);
    let existing_class = attrs
        .get("class")
        .map(|v| v.str(source))
        .unwrap_or_default();
    let has_inline_class = existing_class
        .split_whitespace()
        .any(|c| c == "inline-image");
    let needs_auto_class = is_inline && !has_inline_class;

    if needs_auto_class || !existing_class.is_empty() {
        w.write_str(" class=\"")?;
        if needs_auto_class {
            w.write_str("inline-image")?;
            if !existing_class.is_empty() {
                w.write_str(" ")?;
            }
        }
        if !existing_class.is_empty() {
            w.write_str(&html_escape(&existing_class))?;
        }
        w.write_str("\"")?;
    }
    // Emit remaining attributes, skipping `class` (already handled).
    for (key, value) in attrs.iter() {
        if key == "class" {
            continue;
        }
        w.write_str(" ")?;
        w.write_str(key)?;
        w.write_str("=\"")?;
        w.write_str(&html_escape(&value.str(source)))?;
        w.write_str("\"")?;
    }
    if format_options.xhtml {
        w.write_str(" />")?;
    } else {
        w.write_str(">")?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared renderer structs for suppressing default node rendering
// ---------------------------------------------------------------------------

/// Suppress default Document node rendering (returns `SkipChildren`).
///
/// Both single-page and multi-page renderers use a custom pre-render hook to
/// drive document traversal, so the built-in Document renderer must be
/// replaced with this no-op that prevents double rendering.
pub(crate) struct SuppressDocumentRenderer;

impl<W: TextWrite> RenderNode<W> for SuppressDocumentRenderer {
    fn render_node<'a>(
        &self,
        _w: &mut W,
        _source: &'a str,
        _arena: &'a Arena,
        _node_ref: NodeRef,
        _entering: bool,
        _ctx: &mut renderer::Context,
    ) -> rushdown::Result<WalkStatus> {
        Ok(WalkStatus::SkipChildren)
    }
}

impl<'cb, W> NodeRenderer<'cb, W> for SuppressDocumentRenderer
where
    W: TextWrite + 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<Document>(), BoxRenderNode::new(self));
    }
}

/// Suppress inline footnote definitions (returns `SkipChildren`).
///
/// Both renderers collect footnotes into a dedicated section at the end of
/// the page, so the inline `FootnoteDefinition` nodes must be silenced.
pub(crate) struct SuppressFootnoteDefinitionRenderer;

impl<W: TextWrite> RenderNode<W> for SuppressFootnoteDefinitionRenderer {
    fn render_node<'a>(
        &self,
        _w: &mut W,
        _source: &'a str,
        arena: &'a Arena,
        node_ref: NodeRef,
        _entering: bool,
        _ctx: &mut renderer::Context,
    ) -> rushdown::Result<WalkStatus> {
        let _ = as_extension_data!(arena, node_ref, FootnoteDefinition);
        Ok(WalkStatus::SkipChildren)
    }
}

impl<'cb, W> NodeRenderer<'cb, W> for SuppressFootnoteDefinitionRenderer
where
    W: TextWrite + 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<FootnoteDefinition>(), BoxRenderNode::new(self));
    }
}
