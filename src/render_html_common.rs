use std::any::TypeId;

use rushdown::as_extension_data;
use rushdown::as_kind_data;
use rushdown::ast::{self, Arena, Document, Heading, Image, KindData, Link, NodeRef, WalkStatus};
use rushdown::context::{ContextKey, NodeRefValue, UsizeValue};
use rushdown::renderer;
use rushdown::renderer::html;
use rushdown::renderer::{
    BoxRenderNode, NodeRenderer, NodeRendererRegistry, RenderNode, TextWrite,
};
use rushdown::util::{escape_html, escape_url, EscapeUrlOptions};
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

pub(crate) struct LinkRenderData<'a> {
    pub source: &'a str,
    pub arena: &'a Arena,
    pub node_ref: NodeRef,
    pub link: &'a Link,
    pub target: &'a str,
}

pub(crate) struct ImageRenderData<'a> {
    pub source: &'a str,
    pub arena: &'a Arena,
    pub node_ref: NodeRef,
    pub image: &'a Image,
    pub target: &'a str,
}

pub(crate) fn render_link_open_tag<W: TextWrite>(
    w: &mut W,
    writer: &html::Writer,
    format_options: &html::Options,
    data: &LinkRenderData<'_>,
) -> rushdown::Result<()> {
    let mut dest = escape_url(
        data.target.as_bytes(),
        &EscapeUrlOptions {
            resolves_refs: !matches!(data.link.link_kind(), rushdown::ast::LinkKind::Auto(_)),
            ..EscapeUrlOptions::for_url()
        },
    );
    writer.write_safe_str(w, "<a href=\"")?;
    if format_options.allows_unsafe || !html::is_dangerous_url(&dest) {
        dest = escape_html(dest);
        w.write_str(&String::from_utf8_lossy(&dest))?;
    }
    writer.write_safe_str(w, "\"")?;
    if let Some(title) = data.link.title() {
        writer.write_safe_str(w, " title=\"")?;
        writer.write(w, &title.str(data.source))?;
        writer.write_safe_str(w, "\"")?;
    }
    html::render_attributes(w, data.source, data.arena[data.node_ref].attributes(), None)?;
    writer.write_safe_str(w, ">")
}

pub(crate) fn render_image_tag<W: TextWrite>(
    w: &mut W,
    writer: &html::Writer,
    format_options: &html::Options,
    data: &ImageRenderData<'_>,
) -> rushdown::Result<()> {
    let mut dest = escape_url(
        data.target.as_bytes(),
        &EscapeUrlOptions {
            resolves_refs: true,
            ..EscapeUrlOptions::for_url()
        },
    );
    writer.write_safe_str(w, "<img src=\"")?;
    if format_options.allows_unsafe || !html::is_dangerous_url(&dest) {
        dest = escape_html(dest);
        w.write_str(&String::from_utf8_lossy(&dest))?;
    }
    writer.write_safe_str(w, "\" alt=\"")?;
    render_image_alt_text(w, writer, data.source, data.arena, data.node_ref)?;
    writer.write_safe_str(w, "\"")?;
    if let Some(title) = data.image.title() {
        writer.write_safe_str(w, " title=\"")?;
        writer.write(w, &title.str(data.source))?;
        writer.write_safe_str(w, "\"")?;
    }
    render_image_attributes(w, data.source, data.arena, data.node_ref, format_options)
}

pub(crate) fn render_footnote_reference<W: TextWrite>(
    w: &mut W,
    index: usize,
    ref_index: usize,
) -> rushdown::Result<()> {
    w.write_str(&format!(
        "<sup id=\"fnref:{ref_index}\"><a href=\"#fn:{index}\" class=\"footnote-ref\" role=\"doc-noteref\">{index}</a></sup>"
    ))
}

fn render_image_alt_text<W: TextWrite>(
    w: &mut W,
    writer: &html::Writer,
    source: &str,
    arena: &Arena,
    node_ref: NodeRef,
) -> rushdown::Result<()> {
    for child in arena[node_ref].children(arena) {
        match arena[child].kind_data() {
            KindData::Text(text) => writer.write(w, text.str(source))?,
            KindData::CodeSpan(cs) => writer.write(w, &cs.value_str(source))?,
            _ => render_image_alt_text(w, writer, source, arena, child)?,
        }
    }
    Ok(())
}

pub(crate) struct SharedHeadingHtmlRenderer<W: TextWrite> {
    _phantom: std::marker::PhantomData<W>,
    writer: html::Writer,
    skip_heading_key: ContextKey<NodeRefValue>,
    heading_shift_key: Option<ContextKey<UsizeValue>>,
}

impl<W: TextWrite> SharedHeadingHtmlRenderer<W> {
    pub(crate) fn new(
        options: html::Options,
        skip_heading_key: ContextKey<NodeRefValue>,
        heading_shift_key: Option<ContextKey<UsizeValue>>,
    ) -> Self {
        Self {
            _phantom: std::marker::PhantomData,
            writer: html::Writer::with_options(options),
            skip_heading_key,
            heading_shift_key,
        }
    }
}

impl<W: TextWrite> RenderNode<W> for SharedHeadingHtmlRenderer<W> {
    fn render_node<'a>(
        &self,
        w: &mut W,
        source: &'a str,
        arena: &'a Arena,
        node_ref: NodeRef,
        entering: bool,
        ctx: &mut renderer::Context,
    ) -> rushdown::Result<WalkStatus> {
        let heading = as_kind_data!(arena, node_ref, Heading);
        if let Some(&skip_ref) = ctx.get(self.skip_heading_key) {
            if skip_ref == node_ref {
                if !entering {
                    ctx.remove(self.skip_heading_key);
                }
                return Ok(WalkStatus::SkipChildren);
            }
        }
        let shift = self
            .heading_shift_key
            .and_then(|key| ctx.get(key).copied())
            .unwrap_or(0);
        let level = (heading.level() as usize + shift).min(6);
        if entering {
            w.write_str(&format!("<h{level}"))?;
            html::render_attributes(w, source, arena[node_ref].attributes(), None)?;
            self.writer.write_safe_str(w, ">")?;
        } else {
            w.write_str(&format!("</h{level}>\n"))?;
        }
        Ok(WalkStatus::Continue)
    }
}

impl<'cb, W> NodeRenderer<'cb, W> for SharedHeadingHtmlRenderer<W>
where
    W: TextWrite + 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<Heading>(), BoxRenderNode::new(self));
    }
}

pub(crate) struct SharedFormatFencedDivHtmlRenderer<W: TextWrite> {
    _phantom: std::marker::PhantomData<W>,
    writer: html::Writer,
    skip_heading_key: ContextKey<NodeRefValue>,
    skip_include_blocks: bool,
}

impl<W: TextWrite> SharedFormatFencedDivHtmlRenderer<W> {
    pub(crate) fn with_options(
        options: html::Options,
        skip_heading_key: ContextKey<NodeRefValue>,
        skip_include_blocks: bool,
    ) -> Self {
        Self {
            _phantom: std::marker::PhantomData,
            writer: html::Writer::with_options(options),
            skip_heading_key,
            skip_include_blocks,
        }
    }
}

impl<W: TextWrite> RenderNode<W> for SharedFormatFencedDivHtmlRenderer<W> {
    fn render_node<'a>(
        &self,
        w: &mut W,
        source: &'a str,
        arena: &'a Arena,
        node_ref: NodeRef,
        entering: bool,
        ctx: &mut renderer::Context,
    ) -> rushdown::Result<WalkStatus> {
        let _ = as_extension_data!(arena, node_ref, FencedDiv);
        let classes = arena[node_ref]
            .attributes()
            .get("class")
            .map(|value| value.str(source))
            .unwrap_or_default();

        if self.skip_include_blocks
            && classes
                .split_whitespace()
                .any(|class_name| class_name == "include")
        {
            return Ok(WalkStatus::SkipChildren);
        }

        let known_formats = crate::KNOWN_OUTPUT_FORMATS;
        let present_formats = classes
            .split_whitespace()
            .filter(|class_name| known_formats.contains(class_name))
            .collect::<Vec<_>>();
        if !present_formats.is_empty() && !present_formats.contains(&"html") {
            return Ok(WalkStatus::SkipChildren);
        }

        let is_admonition = crate::admonition_type(&classes).is_some();

        if entering {
            if is_admonition {
                w.write_str("<div class=\"admonition ")?;
                let escaped = escape_html(classes.as_bytes());
                w.write_str(std::str::from_utf8(&escaped).unwrap_or_default())?;
                w.write_str("\"")?;
                for (key, value) in arena[node_ref].attributes().iter() {
                    if key == "class" {
                        continue;
                    }
                    w.write_str(" ")?;
                    w.write_str(key)?;
                    w.write_str("=\"")?;
                    let escaped = escape_html(value.bytes(source));
                    w.write_str(std::str::from_utf8(&escaped).unwrap_or_default())?;
                    w.write_str("\"")?;
                }
                w.write_str(">")?;

                if let Some(first_child) = arena[node_ref].children(arena).next() {
                    if matches!(arena[first_child].kind_data(), KindData::Heading(_)) {
                        let title_html = heading_text_html(arena, first_child, source);
                        w.write_str("<p class=\"admonition-title\">")?;
                        w.write_str(&title_html)?;
                        w.write_str("</p>\n")?;
                        ctx.insert(self.skip_heading_key, first_child);
                    }
                }
            } else {
                self.writer.write_safe_str(w, "<div")?;
                html::render_attributes(w, source, arena[node_ref].attributes(), None)?;
                self.writer.write_safe_str(w, ">")?;
            }
        } else {
            self.writer.write_safe_str(w, "</div>")?;
        }
        Ok(WalkStatus::Continue)
    }
}

impl<'cb, W> NodeRenderer<'cb, W> for SharedFormatFencedDivHtmlRenderer<W>
where
    W: TextWrite + 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<FencedDiv>(), BoxRenderNode::new(self));
    }
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
