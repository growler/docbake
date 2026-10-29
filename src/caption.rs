//! Pandoc-style table, diagram, and listing captions extension for rushdown.
//!
//! Detects paragraphs starting with `Table:`, `Figure:`, or `Listing:` and
//! associates them with an adjacent captionable block (table, code block,
//! code-block diagram, or fenced div). The paragraph is removed and the block
//! is wrapped in a [`Caption`] container node whose second child is a
//! [`CaptionText`] node holding the caption's inline content.
//!
//! # Syntax
//!
//! ```text
//! Table: [Applicable standards]{#rfc-table}
//!
//! | RFC      | Description             |
//! |----------|-------------------------|
//! | RFC 2822 | Internet Message Format |
//!
//! Listing: A listing {#sample}
//!
//!     ```rust
//!     fn main() {}
//!     ```
//! ```
//!
//! Without `{#id}` the identifier is auto-generated from the caption text
//! using the same rules as heading IDs.
//!
//! This module is designed to be extracted into a standalone crate later.

use std::any::TypeId;
use std::fmt;
use std::fmt::Write;

use rushdown::as_kind_data;
use rushdown::ast::{
    self, pp_indent, Arena, KindData, NodeKind, NodeRef, NodeType, PrettyPrint, WalkStatus,
};
use rushdown::matches_extension_kind;
use rushdown::parser::{self, AnyAstTransformer, AstTransformer};
#[cfg(test)]
use rushdown::parser::{NoParserOptions, Parser, ParserExtension, ParserExtensionFn};
use rushdown::renderer::html::{self, Renderer, RendererExtension, RendererExtensionFn};
use rushdown::renderer::{
    self, BoxRenderNode, NodeRenderer, NodeRendererRegistry, RenderNode, TextWrite,
};
use rushdown::text::{self, Reader as _};

use crate::diagram::Diagram;
use rushdown_fenced_div::FencedDiv;

// ============================================================================
// AST node types
// ============================================================================

/// The kind of captioned element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionKind {
    Table,
    Figure,
    Listing,
}

/// Whether the caption appeared before or after the captioned block in source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionPosition {
    Before,
    After,
}

/// Container block wrapping a captioned element.
///
/// Children:
/// 1. The captioned block (table, code block, or fenced div) — reparented.
/// 2. A [`CaptionText`] node containing the caption's inline content.
///
/// The `id` attribute on this node holds the resolved caption ID.
#[derive(Debug)]
pub struct Caption {
    kind: CaptionKind,
    position: CaptionPosition,
}

impl Caption {
    pub fn new(kind: CaptionKind, position: CaptionPosition) -> Self {
        Self { kind, position }
    }

    pub fn kind(&self) -> CaptionKind {
        self.kind
    }

    pub fn position(&self) -> CaptionPosition {
        self.position
    }
}

impl NodeKind for Caption {
    fn typ(&self) -> NodeType {
        NodeType::ContainerBlock
    }

    fn kind_name(&self) -> &'static str {
        "Caption"
    }
}

impl PrettyPrint for Caption {
    fn pretty_print(&self, w: &mut dyn Write, _source: &str, level: usize) -> fmt::Result {
        writeln!(
            w,
            "{}Caption({:?}, {:?})",
            pp_indent(level),
            self.kind,
            self.position
        )
    }
}

impl From<Caption> for KindData {
    fn from(c: Caption) -> Self {
        KindData::Extension(Box::new(c))
    }
}

/// Inline container holding the caption text (inline children of the original
/// paragraph, with the `Table:`/`Figure:`/`:` prefix stripped).
#[derive(Debug)]
pub struct CaptionText;

impl NodeKind for CaptionText {
    fn typ(&self) -> NodeType {
        // Block type so it sits as a direct child of the Caption container.
        NodeType::ContainerBlock
    }

    fn kind_name(&self) -> &'static str {
        "CaptionText"
    }
}

impl PrettyPrint for CaptionText {
    fn pretty_print(&self, w: &mut dyn Write, _source: &str, level: usize) -> fmt::Result {
        writeln!(w, "{}CaptionText", pp_indent(level))
    }
}

impl From<CaptionText> for KindData {
    fn from(c: CaptionText) -> Self {
        KindData::Extension(Box::new(c))
    }
}

// ============================================================================
// AST transformer
// ============================================================================

/// Recognised caption prefixes and their corresponding kinds.
const CAPTION_PREFIXES: &[(&str, CaptionKind)] = &[
    ("Table:", CaptionKind::Table),
    ("table:", CaptionKind::Table),
    ("Figure:", CaptionKind::Figure),
    ("figure:", CaptionKind::Figure),
    ("Listing:", CaptionKind::Listing),
    ("listing:", CaptionKind::Listing),
];

/// Detects the caption prefix in a paragraph's first text child.
///
/// Returns `Some((kind, byte_offset_after_prefix))` if a recognised prefix
/// (`Table:`, `Figure:`, `Listing:`) is found. The offset points to the first
/// non-whitespace character after the prefix marker (or to the end of the text
/// if the prefix consumes it entirely).
fn detect_caption_prefix(text_str: &str) -> Option<(CaptionKind, usize)> {
    for &(prefix, kind) in CAPTION_PREFIXES {
        if let Some(rest) = text_str.strip_prefix(prefix) {
            let skip = prefix.len() + rest.len() - rest.trim_start().len();
            return Some((kind, skip));
        }
    }
    None
}

/// Returns `true` if `node_ref` is a captionable block.
///
/// Captionable blocks:
/// - `KindData::Table`
/// - `KindData::CodeBlock` (any language — diagram code blocks are already
///   transformed to `Diagram` nodes by the time the caption transformer runs)
/// - `KindData::Extension(Diagram)` (produced by diagram parser)
/// - `KindData::Extension(FencedDiv)` — excluding those with class `include`
/// - `KindData::Paragraph` containing exactly one `Image` child
fn is_captionable(arena: &Arena, node_ref: NodeRef, source: &str) -> bool {
    match arena[node_ref].kind_data() {
        KindData::Table(_) => true,
        KindData::CodeBlock(_) => true,
        KindData::Extension(_) if matches_extension_kind!(arena, node_ref, Diagram) => true,
        KindData::Extension(_) if matches_extension_kind!(arena, node_ref, FencedDiv) => {
            // Exclude include divs.
            let classes = arena[node_ref]
                .attributes()
                .get("class")
                .map(|v| v.str(source))
                .unwrap_or_default();
            !classes.split_whitespace().any(|c| c == "include")
        }
        KindData::Paragraph(_) => {
            crate::diagram::paragraph_has_sole_image(arena, node_ref).is_some()
        }
        _ => false,
    }
}

/// Find the nearest captionable sibling: prefer next, fall back to previous.
fn find_captionable_sibling(
    arena: &Arena,
    paragraph: NodeRef,
    source: &str,
    used_targets: &[NodeRef],
) -> Option<(NodeRef, CaptionPosition)> {
    let next = arena[paragraph].next_sibling();
    let prev = arena[paragraph].previous_sibling();

    if let Some(n) = next {
        if is_captionable(arena, n, source) && !used_targets.contains(&n) {
            return Some((n, CaptionPosition::Before));
        }
    }
    if let Some(p) = prev {
        if is_captionable(arena, p, source) && !used_targets.contains(&p) {
            return Some((p, CaptionPosition::After));
        }
    }
    None
}

/// Collected caption candidate before AST rewriting.
struct CaptionCandidate {
    paragraph: NodeRef,
    target: NodeRef,
    kind: CaptionKind,
    prefix_len: usize,
    position: CaptionPosition,
}

#[derive(Debug)]
pub(crate) struct CaptionAstTransformer;

impl CaptionAstTransformer {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl AstTransformer for CaptionAstTransformer {
    fn transform(
        &self,
        arena: &mut Arena,
        doc_ref: NodeRef,
        reader: &mut text::BasicReader,
        _ctx: &mut parser::Context,
    ) {
        let source = reader.source();

        // Phase 1: collect candidates (read-only pass).
        //
        // Walk the entire tree so that caption paragraphs nested inside
        // container blocks (fenced divs such as `:::related`, blockquotes,
        // etc.) are discovered — not only direct children of the document
        // root.
        let mut candidates = Vec::new();
        let mut used_targets: Vec<NodeRef> = Vec::new();

        ast::walk(
            arena,
            doc_ref,
            &mut |arena: &Arena,
                  node_ref: NodeRef,
                  entering: bool|
             -> rushdown::Result<WalkStatus> {
                if !entering {
                    return Ok(WalkStatus::Continue);
                }
                let KindData::Paragraph(_) = arena[node_ref].kind_data() else {
                    return Ok(WalkStatus::Continue);
                };
                // Find the first text child of the paragraph.
                let Some(first_child) = arena[node_ref].children(arena).next() else {
                    return Ok(WalkStatus::Continue);
                };
                let KindData::Text(text_val) = arena[first_child].kind_data() else {
                    return Ok(WalkStatus::Continue);
                };
                let text_str = text_val.str(source);
                let Some((kind, prefix_len)) = detect_caption_prefix(text_str) else {
                    return Ok(WalkStatus::Continue);
                };

                // Look for a captionable sibling: prefer next, then previous.
                let Some((target, position)) =
                    find_captionable_sibling(arena, node_ref, source, &used_targets)
                else {
                    eprintln!(
                        "warning: caption paragraph has no adjacent captionable block, \
                     rendering as normal paragraph"
                    );
                    return Ok(WalkStatus::Continue);
                };

                used_targets.push(target);
                candidates.push(CaptionCandidate {
                    paragraph: node_ref,
                    target,
                    kind,
                    prefix_len,
                    position,
                });
                Ok(WalkStatus::Continue)
            },
        )
        .ok();

        // Phase 2: rewrite AST.
        for candidate in candidates {
            let CaptionCandidate {
                paragraph,
                target,
                kind,
                prefix_len,
                position,
            } = candidate;

            // Create the Caption container.
            let caption_node = arena.new_node(Caption::new(kind, position));

            // Copy attributes from the paragraph to the caption (e.g. `id` from
            // bracketed spans). Attributes may exist on the paragraph itself
            // (from `{#id}` syntax at the block level).
            let attrs: Vec<_> = arena[paragraph]
                .attributes()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (key, value) in attrs {
                arena[caption_node].attributes_mut().insert(key, value);
            }

            // Create CaptionText and reparent paragraph's inline children into it.
            let caption_text_node = arena.new_node(CaptionText);

            // Strip the prefix from the first text child.
            let first_child = arena[paragraph].children(arena).next();
            if let Some(fc) = first_child {
                if matches!(arena[fc].kind_data(), KindData::Text(_)) {
                    let text_val = as_kind_data!(arena, fc, Text);
                    let full_str = text_val.str(source);
                    if prefix_len >= full_str.len() {
                        // The entire text node is consumed by the prefix — delete it.
                        // The next text node can start with the space after the
                        // prefix.
                        let next = arena[fc].next_sibling();
                        fc.delete(arena);
                        if let Some(next) = next {
                            trim_text_start(arena, next, source);
                        }
                    } else {
                        // Trim the prefix: create a new text value with offset content.
                        let remaining = &full_str[prefix_len..];
                        let remaining_owned = remaining.to_string();
                        // Check if we can use index-based trimming.
                        if let Some(idx) = text_val.index().cloned() {
                            let new_start = idx.start() + prefix_len;
                            rushdown::as_kind_data_mut!(arena, fc, Text)
                                .set(text::Index::new(new_start, idx.stop()));
                        } else {
                            rushdown::as_kind_data_mut!(arena, fc, Text).set(remaining_owned);
                        }
                    }
                }
            }

            // Reparent all remaining inline children of the paragraph into CaptionText.
            let children: Vec<NodeRef> = arena[paragraph].children(arena).collect();
            for c in children {
                caption_text_node.append_child(arena, c);
            }

            // Extract trailing {#id .class key="value"} attributes from the
            // last text child of the CaptionText node. The attribute block must
            // be at the very end — no text is allowed after it.
            extract_trailing_attributes(arena, caption_text_node, caption_node, source);

            // Build the Caption node:
            // Child 1: the captioned block
            // Child 2: the CaptionText
            // Replace the paragraph with the Caption node in the document tree.
            let parent = arena[paragraph].parent().unwrap();

            // Insert Caption where the paragraph is.
            parent.insert_before(arena, paragraph, caption_node);
            paragraph.delete(arena);

            // Append children in display order: CaptionText first (renders as
            // <figcaption> before the block in HTML), then the captioned block.
            caption_node.append_child(arena, caption_text_node);
            caption_node.append_child(arena, target);
        }
    }
}

/// Removes the leading whitespace of the text node `node`. A node of another
/// kind does not change.
fn trim_text_start(arena: &mut Arena, node: NodeRef, source: &str) {
    let KindData::Text(text_val) = arena[node].kind_data() else {
        return;
    };
    let full_str = text_val.str(source);
    let skip = full_str.len() - full_str.trim_start().len();
    if skip == 0 {
        return;
    }
    if let Some(idx) = text_val.index().cloned() {
        rushdown::as_kind_data_mut!(arena, node, Text)
            .set(text::Index::new(idx.start() + skip, idx.stop()));
    } else {
        let remaining_owned = full_str[skip..].to_string();
        rushdown::as_kind_data_mut!(arena, node, Text).set(remaining_owned);
    }
}

impl From<CaptionAstTransformer> for AnyAstTransformer {
    fn from(t: CaptionAstTransformer) -> Self {
        AnyAstTransformer::Extension(Box::new(t))
    }
}

/// Extract trailing `{#id .class key="value"}` from the text at the end of
/// `caption_text_node` and promote the parsed attributes to `caption_node`.
///
/// The attribute block must be the very last thing in the caption text.
/// Inline parsers may split the `{...}` across multiple text nodes, so this
/// function collects trailing consecutive text nodes, concatenates them, and
/// looks for a `{...}` span in the combined text.
fn extract_trailing_attributes(
    arena: &mut Arena,
    caption_text_node: NodeRef,
    caption_node: NodeRef,
    source: &str,
) {
    // Collect trailing consecutive Text nodes (backwards from last child).
    let mut trailing_text_nodes: Vec<NodeRef> = Vec::new();
    let mut node = arena[caption_text_node].last_child();
    while let Some(n) = node {
        if matches!(arena[n].kind_data(), KindData::Text(_)) {
            trailing_text_nodes.push(n);
            node = arena[n].previous_sibling();
        } else {
            break;
        }
    }
    if trailing_text_nodes.is_empty() {
        return;
    }
    // Reverse to get forward order.
    trailing_text_nodes.reverse();

    // Concatenate the text content and track byte offsets per node.
    let mut combined = String::new();
    let mut node_ranges: Vec<(NodeRef, usize, usize)> = Vec::new(); // (node, start, end)
    for &n in &trailing_text_nodes {
        let start = combined.len();
        let s = as_kind_data!(arena, n, Text).str(source).to_string();
        combined.push_str(&s);
        let end = combined.len();
        node_ranges.push((n, start, end));
    }

    if !combined.ends_with('}') {
        return;
    }

    // Find the matching `{` scanning backwards, handling nesting.
    let bytes = combined.as_bytes();
    let mut depth = 0i32;
    let mut brace_start = None;
    for i in (0..bytes.len()).rev() {
        if bytes[i] == b'}' {
            depth += 1;
        } else if bytes[i] == b'{' {
            depth -= 1;
            if depth == 0 {
                brace_start = Some(i);
                break;
            }
        }
    }
    let brace_start = match brace_start {
        Some(s) => s,
        None => return,
    };

    // Extract the attribute substring and try to parse it.
    let attr_str = &combined[brace_start..];
    let mut reader = text::BasicReader::new(attr_str);
    let attrs = match parser::parse_attributes(&mut reader) {
        Some(a) => a,
        None => return,
    };

    // Verify the parser consumed the entire attribute substring.
    if reader.peek_byte() != text::EOS {
        return;
    }

    // Apply parsed attributes to the Caption node. Values are resolved against
    // attr_str (the substring source) and stored as owned strings.
    for (key, value) in attrs.iter() {
        let resolved = value.str(attr_str).to_string();
        arena[caption_node]
            .attributes_mut()
            .insert(key.clone(), resolved.into());
    }

    // Trim the attribute block (and any preceding whitespace) from the text
    // nodes. The attribute block starts at `brace_start` in the combined text.
    // We also trim whitespace immediately before `{`.
    let trim_start = combined[..brace_start].trim_end().len();

    for &(n, start, end) in &node_ranges {
        if start >= trim_start {
            // This entire node is within the trimmed region — delete it.
            n.delete(arena);
        } else if end > trim_start {
            // The trim point falls within this node — truncate it.
            let keep_len = trim_start - start;
            let text_val = as_kind_data!(arena, n, Text);
            if let Some(idx) = text_val.index().cloned() {
                let new_stop = idx.start() + keep_len;
                rushdown::as_kind_data_mut!(arena, n, Text)
                    .set(text::Index::new(idx.start(), new_stop));
            } else {
                let s = text_val.str(source).to_string();
                rushdown::as_kind_data_mut!(arena, n, Text).set(s[..keep_len].to_string());
            }
        }
        // Nodes entirely before trim_start are kept as-is.
    }
}

/// Returns a parser extension that detects caption paragraphs and restructures
/// the AST.
///
/// Note: in production, the caption transformer is run explicitly in
/// `graph/build.rs` (after the image-to-diagram transform) rather than
/// via this parser extension.  This function is retained for unit tests.
#[cfg(test)]
pub fn caption_parser_extension() -> impl ParserExtension {
    ParserExtensionFn::new(|p: &mut Parser| {
        p.add_ast_transformer(CaptionAstTransformer::new, NoParserOptions, 200);
    })
}

// ============================================================================
// HTML renderer
// ============================================================================

struct CaptionHtmlRenderer<W: TextWrite> {
    _phantom: std::marker::PhantomData<W>,
    writer: html::Writer,
}

impl<W: TextWrite> CaptionHtmlRenderer<W> {
    fn with_options(html_opts: html::Options) -> Self {
        Self {
            _phantom: std::marker::PhantomData,
            writer: html::Writer::with_options(html_opts),
        }
    }
}

impl<W: TextWrite> RenderNode<W> for CaptionHtmlRenderer<W> {
    fn render_node<'a>(
        &self,
        w: &mut W,
        source: &'a str,
        arena: &'a Arena,
        node_ref: NodeRef,
        entering: bool,
        _ctx: &mut renderer::Context,
    ) -> rushdown::Result<WalkStatus> {
        if entering {
            let caption = rushdown::as_extension_data!(arena, node_ref, Caption);
            let kind_class = match caption.kind() {
                CaptionKind::Listing => Some("listing"),
                _ => None,
            };

            // Build full opening tag, merging kind-based class with any
            // user-supplied class from trailing `{.class}` syntax.
            let attrs = arena[node_ref].attributes();
            let user_class = attrs.get("class").map(|v| v.str(source).to_string());
            let combined_class = match (kind_class, &user_class) {
                (Some(kc), Some(uc)) => format!("{kc} {uc}"),
                (Some(kc), None) => kc.to_string(),
                (None, Some(uc)) => uc.clone(),
                (None, None) => String::new(),
            };

            let mut tag = String::from("<figure");
            if !combined_class.is_empty() {
                tag.push_str(" class=\"");
                tag.push_str(&combined_class);
                tag.push('"');
            }
            for (key, value) in attrs.iter() {
                if key == "class" {
                    continue;
                }
                tag.push(' ');
                tag.push_str(key);
                tag.push_str("=\"");
                tag.push_str(&value.str(source));
                tag.push('"');
            }
            tag.push_str(">\n");
            self.writer.write_html(w, &tag)?;
        } else {
            self.writer.write_safe_str(w, "</figure>\n")?;
        }
        Ok(WalkStatus::Continue)
    }
}

impl<'cb, W> NodeRenderer<'cb, W> for CaptionHtmlRenderer<W>
where
    W: TextWrite + 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<Caption>(), BoxRenderNode::new(self));
    }
}

struct CaptionTextHtmlRenderer<W: TextWrite> {
    _phantom: std::marker::PhantomData<W>,
    writer: html::Writer,
}

impl<W: TextWrite> CaptionTextHtmlRenderer<W> {
    fn with_options(html_opts: html::Options) -> Self {
        Self {
            _phantom: std::marker::PhantomData,
            writer: html::Writer::with_options(html_opts),
        }
    }
}

impl<W: TextWrite> RenderNode<W> for CaptionTextHtmlRenderer<W> {
    fn render_node<'a>(
        &self,
        w: &mut W,
        _source: &'a str,
        _arena: &'a Arena,
        _node_ref: NodeRef,
        entering: bool,
        _ctx: &mut renderer::Context,
    ) -> rushdown::Result<WalkStatus> {
        if entering {
            self.writer.write_safe_str(w, "<figcaption>")?;
        } else {
            self.writer.write_safe_str(w, "</figcaption>\n")?;
        }
        Ok(WalkStatus::Continue)
    }
}

impl<'cb, W> NodeRenderer<'cb, W> for CaptionTextHtmlRenderer<W>
where
    W: TextWrite + 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<CaptionText>(), BoxRenderNode::new(self));
    }
}

/// Returns a renderer extension that renders [`Caption`] and [`CaptionText`]
/// nodes in HTML as `<figure>` and `<figcaption>`.
pub fn caption_html_renderer_extension<'cb, W>() -> impl RendererExtension<'cb, W>
where
    W: TextWrite + 'cb,
{
    RendererExtensionFn::new(move |r: &mut Renderer<'cb, W>| {
        r.add_node_renderer(
            CaptionHtmlRenderer::with_options,
            rushdown::renderer::NoRendererOptions,
        );
        r.add_node_renderer(
            CaptionTextHtmlRenderer::with_options,
            rushdown::renderer::NoRendererOptions,
        );
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rushdown::parser;
    use rushdown::renderer::html;
    use rushdown::text::BasicReader;

    fn test_parser() -> parser::Parser {
        parser::Parser::with_extensions(
            parser::Options {
                attributes: true,
                ..Default::default()
            },
            parser::gfm(parser::GfmOptions::default())
                .and(rushdown_link_attribute::link_attribute_parser_extension())
                .and(crate::bracketed_span::bracketed_span_parser_extension())
                .and(rushdown_fenced_div::fenced_div_parser_extension())
                .and(caption_parser_extension()),
        )
    }

    fn parse(source: &str) -> (Arena, NodeRef) {
        let parser = test_parser();
        let mut reader = BasicReader::new(source);
        parser.parse(&mut reader)
    }

    fn render_html(source: &str) -> String {
        let (arena, doc_ref) = parse(source);
        let renderer = html::Renderer::with_extensions(
            html::Options::default(),
            crate::bracketed_span::bracketed_span_html_renderer_extension()
                .and(rushdown_fenced_div::fenced_div_html_renderer_extension(
                    rushdown_fenced_div::FencedDivHtmlRendererOptions,
                ))
                .and(caption_html_renderer_extension()),
        );
        let mut output = String::new();
        renderer
            .render(&mut output, source, &arena, doc_ref)
            .unwrap();
        output
    }

    // -- AST structure tests --

    #[test]
    fn caption_before_table() {
        let source = "Table: My table\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
        let (arena, doc_ref) = parse(source);

        // The document should have one child: a Caption node.
        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1, "expected one Caption node");

        let caption_ref = children[0];
        assert!(
            matches_extension_kind!(&arena, caption_ref, Caption),
            "expected Caption extension node"
        );

        // Caption should have two children: CaptionText first, then the table.
        let caption_children: Vec<_> = arena[caption_ref].children(&arena).collect();
        assert_eq!(
            caption_children.len(),
            2,
            "Caption should have 2 children (text + block)"
        );
        assert!(
            matches_extension_kind!(&arena, caption_children[0], CaptionText),
            "first child should be CaptionText"
        );
        assert!(
            matches!(arena[caption_children[1]].kind_data(), KindData::Table(_)),
            "second child should be Table"
        );
    }

    #[test]
    fn one_word_caption_text_has_no_leading_space() {
        let source = "Table: Values\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
        let (arena, doc_ref) = parse(source);

        let caption_ref = arena[doc_ref].children(&arena).next().unwrap();
        let caption_text = arena[caption_ref].children(&arena).next().unwrap();
        assert!(matches_extension_kind!(&arena, caption_text, CaptionText));
        let texts: Vec<_> = arena[caption_text]
            .children(&arena)
            .map(|c| as_kind_data!(arena, c, Text).str(source).to_string())
            .collect();
        assert_eq!(texts, ["Values"]);
    }

    #[test]
    fn caption_after_table() {
        let source = "| A | B |\n|---|---|\n| 1 | 2 |\n\nTable: My table\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1, "expected one Caption node");
        assert!(matches_extension_kind!(&arena, children[0], Caption));
    }

    #[test]
    fn table_prefix_before_table() {
        let source = "Table: My table\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1);
        assert!(matches_extension_kind!(&arena, children[0], Caption));

        let caption = rushdown::as_extension_data!(&arena, children[0], Caption);
        assert_eq!(caption.kind(), CaptionKind::Table);
    }

    #[test]
    fn figure_caption_before_code_block() {
        let source = "Figure: My diagram\n\n```mermaid\ngraph LR\n  A --> B\n```\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1);
        assert!(matches_extension_kind!(&arena, children[0], Caption));

        let caption = rushdown::as_extension_data!(&arena, children[0], Caption);
        assert_eq!(caption.kind(), CaptionKind::Figure);

        let caption_children: Vec<_> = arena[children[0]].children(&arena).collect();
        assert!(
            matches_extension_kind!(&arena, caption_children[0], CaptionText),
            "first child should be CaptionText"
        );
        assert!(
            matches!(
                arena[caption_children[1]].kind_data(),
                KindData::CodeBlock(_)
            ),
            "second child should be CodeBlock"
        );
    }

    #[test]
    fn figure_caption_before_fenced_div() {
        let source = "Figure: My figure\n\n:::myclass\nContent here\n:::\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1);
        assert!(matches_extension_kind!(&arena, children[0], Caption));
    }

    #[test]
    fn orphan_caption_stays_as_paragraph() {
        let source = "Table: Orphan caption\n\nJust a paragraph.\n";
        let (arena, doc_ref) = parse(source);

        // The caption paragraph should remain because the next sibling is not
        // captionable (it's another paragraph).
        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 2, "both paragraphs should remain");
        assert!(
            matches!(arena[children[0]].kind_data(), KindData::Paragraph(_)),
            "first child should still be a Paragraph"
        );
    }

    #[test]
    fn code_block_captionable_with_figure_prefix() {
        // All code blocks are now captionable; `Figure:` prefix overrides kind.
        let source = "Figure: A code example\n\n```rust\nfn main() {}\n```\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1, "expected one Caption node");
        assert!(matches_extension_kind!(&arena, children[0], Caption));

        let caption = rushdown::as_extension_data!(&arena, children[0], Caption);
        assert_eq!(caption.kind(), CaptionKind::Figure);
    }

    #[test]
    fn listing_caption_before_code_block() {
        let source = "Listing: Sample code\n\n```rust\nfn main() {}\n```\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1, "expected one Caption node");
        assert!(matches_extension_kind!(&arena, children[0], Caption));

        let caption = rushdown::as_extension_data!(&arena, children[0], Caption);
        assert_eq!(caption.kind(), CaptionKind::Listing);
    }

    #[test]
    fn listing_caption_with_id() {
        let source = "Listing: Sample {#my-listing}\n\n```rust\nfn main() {}\n```\n";
        let (arena, doc_ref) = parse(source);

        let caption_ref = arena[doc_ref].children(&arena).next().unwrap();
        assert!(matches_extension_kind!(&arena, caption_ref, Caption));

        let id = arena[caption_ref]
            .attributes()
            .get("id")
            .map(|v| v.str(source).to_string());
        assert_eq!(id, Some("my-listing".to_string()));
    }

    #[test]
    fn include_div_not_captionable() {
        let source = "Figure: Not captionable\n\n:::include\nSome path\n:::\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        // The include div should not be wrapped.
        assert!(children.len() >= 2);
        assert!(matches!(
            arena[children[0]].kind_data(),
            KindData::Paragraph(_)
        ));
    }

    // -- HTML rendering tests --

    #[test]
    fn html_figure_wraps_table() {
        let source = "Table: My table\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
        let html = render_html(source);

        assert!(html.contains("<figure>"), "should contain <figure>: {html}");
        assert!(
            html.contains("<figcaption>"),
            "should contain <figcaption>: {html}"
        );
        assert!(
            html.contains("</figcaption>"),
            "should contain </figcaption>: {html}"
        );
        assert!(
            html.contains("</figure>"),
            "should contain </figure>: {html}"
        );
        assert!(html.contains("<table>"), "should contain <table>: {html}");

        // figcaption should appear before the table.
        let figcaption_pos = html.find("<figcaption>").unwrap();
        let table_pos = html.find("<table>").unwrap();
        assert!(
            figcaption_pos < table_pos,
            "<figcaption> should appear before <table> in HTML: {html}"
        );
    }

    #[test]
    fn html_caption_text_content() {
        let source = "Table: Hello **world**\n\n| A |\n|---|\n| 1 |\n";
        let html = render_html(source);

        assert!(
            html.contains("<figcaption>Hello <strong>world</strong>"),
            "caption should preserve inline markup: {html}"
        );
    }

    #[test]
    fn html_table_prefix() {
        let source = "Table: Short caption\n\n| A |\n|---|\n| 1 |\n";
        let html = render_html(source);

        assert!(html.contains("<figure>"), "should contain <figure>: {html}");
        assert!(
            html.contains("Short caption"),
            "should contain caption text: {html}"
        );
    }

    #[test]
    fn html_listing_caption() {
        let source = "Listing: A listing\n\n```rust\nfn main() {}\n```\n";
        let html = render_html(source);

        assert!(
            html.contains("<figure class=\"listing\">"),
            "should contain <figure class=\"listing\">: {html}"
        );
        assert!(
            html.contains("<figcaption>A listing</figcaption>"),
            "should contain figcaption: {html}"
        );

        // figcaption should appear before the code block.
        let figcaption_pos = html.find("<figcaption>").unwrap();
        let code_pos = html.find("<code").unwrap();
        assert!(
            figcaption_pos < code_pos,
            "<figcaption> should appear before <code> in HTML: {html}"
        );
    }

    #[test]
    fn case_insensitive_prefix() {
        let source = "table: lowercase\n\n| A |\n|---|\n| 1 |\n";
        let html = render_html(source);
        assert!(
            html.contains("<figure>"),
            "lowercase 'table:' should work: {html}"
        );

        let source2 = "figure: lowercase fig\n\n```mermaid\ngraph LR\n```\n";
        let html2 = render_html(source2);
        assert!(
            html2.contains("<figure>"),
            "lowercase 'figure:' should work: {html2}"
        );
    }

    #[test]
    fn parse_attributes_with_basic_reader_smoke() {
        let s = "{#my-id .cls}";
        let mut r = BasicReader::new(s);
        let attrs = parser::parse_attributes(&mut r);
        assert!(attrs.is_some(), "parse_attributes should succeed on {s:?}");
        let attrs = attrs.unwrap();
        assert_eq!(
            attrs.get("id").map(|v| v.str(s).to_string()),
            Some("my-id".to_string())
        );
        assert_eq!(
            attrs.get("class").map(|v| v.str(s).to_string()),
            Some("cls".to_string())
        );
    }

    // -- Trailing attribute tests --

    #[test]
    fn trailing_id_attribute() {
        let source = "Table: My caption {#my-tbl}\n\n| A |\n|---|\n| 1 |\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1);
        let caption_ref = children[0];
        assert!(matches_extension_kind!(&arena, caption_ref, Caption));

        // The Caption node should have the promoted id attribute.
        let id = arena[caption_ref]
            .attributes()
            .get("id")
            .map(|v| v.str(source).to_string());
        assert_eq!(id, Some("my-tbl".to_string()), "id should be promoted");

        // The CaptionText should NOT contain the `{{#my-tbl}}` text.
        let html = render_html(source);
        assert!(
            !html.contains("{#my-tbl}"),
            "attribute block should be stripped from rendered text: {html}"
        );
        assert!(
            html.contains("My caption"),
            "caption text should remain: {html}"
        );
    }

    #[test]
    fn trailing_multiple_attributes() {
        let source = "Table: Standards {#rfc .reference}\n\n| A |\n|---|\n| 1 |\n";
        let (arena, doc_ref) = parse(source);

        let caption_ref = arena[doc_ref].children(&arena).next().unwrap();
        let id = arena[caption_ref]
            .attributes()
            .get("id")
            .map(|v| v.str(source).to_string());
        let class = arena[caption_ref]
            .attributes()
            .get("class")
            .map(|v| v.str(source).to_string());
        assert_eq!(id, Some("rfc".to_string()));
        assert_eq!(class, Some("reference".to_string()));
    }

    #[test]
    fn attributes_only_caption() {
        // `Table: {#figure-ident}` — empty caption text, only attributes.
        let source = "Table: {#figure-ident}\n\n| A |\n|---|\n| 1 |\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1);
        let caption_ref = children[0];
        assert!(matches_extension_kind!(&arena, caption_ref, Caption));

        let id = arena[caption_ref]
            .attributes()
            .get("id")
            .map(|v| v.str(source).to_string());
        assert_eq!(id, Some("figure-ident".to_string()));
    }

    #[test]
    fn html_renders_caption_id() {
        let source = "Table: My table {#tbl-1}\n\n| A |\n|---|\n| 1 |\n";
        let html = render_html(source);
        assert!(
            html.contains(r#"<figure id="tbl-1">"#),
            "figure should have id attribute: {html}"
        );
    }

    #[test]
    fn attributes_not_at_end_stay_as_text() {
        // `{#id}` is NOT at the end — followed by " trailing".
        // This should NOT be parsed as attributes.
        let source = "Table: Caption {#id} trailing\n\n| A |\n|---|\n| 1 |\n";
        let html = render_html(source);
        // The {#id} should be rendered as literal text in the figcaption.
        assert!(
            html.contains("{#id} trailing"),
            "non-trailing attributes should stay as text: {html}"
        );
    }

    // -- Nested container tests --

    #[test]
    fn caption_inside_fenced_div() {
        // Regression: caption paragraphs inside fenced divs (e.g. :::note,
        // :::related) must be detected, not only top-level ones.
        let source = ":::note\nListing: Nested code {#nested-listing}\n\n```rust\nfn nested() {}\n```\n:::\n";
        let (arena, doc_ref) = parse(source);

        // Walk the tree to find a Caption node anywhere.
        let mut found_caption = false;
        let mut caption_id = None;
        ast::walk(
            &arena,
            doc_ref,
            &mut |arena: &Arena,
                  node_ref: NodeRef,
                  entering: bool|
             -> rushdown::Result<WalkStatus> {
                if entering && matches_extension_kind!(arena, node_ref, Caption) {
                    found_caption = true;
                    caption_id = arena[node_ref]
                        .attributes()
                        .get("id")
                        .map(|v| v.str(source).to_string());
                }
                Ok(WalkStatus::Continue)
            },
        )
        .ok();

        assert!(
            found_caption,
            "Caption node should exist inside the fenced div"
        );
        assert_eq!(
            caption_id,
            Some("nested-listing".to_string()),
            "Caption should have the explicit id"
        );
    }

    #[test]
    fn caption_inside_fenced_div_html() {
        let source = ":::note\nListing: Nested listing\n\n```rust\nfn nested() {}\n```\n:::\n";
        let html = render_html(source);

        assert!(
            html.contains("<figure class=\"listing\">"),
            "should contain <figure class=\"listing\"> inside fenced div: {html}"
        );
        assert!(
            html.contains("<figcaption>Nested listing</figcaption>"),
            "should contain figcaption inside fenced div: {html}"
        );
    }
}
