//! Pandoc-style table and diagram captions extension for rushdown.
//!
//! Detects paragraphs starting with `Table:`, `Figure:`, or `:` and associates
//! them with an adjacent captionable block (table, code-block diagram, or
//! fenced div). The paragraph is removed and the block is wrapped in a
//! [`Caption`] container node whose second child is a [`CaptionText`] node
//! holding the caption's inline content.
//!
//! # Syntax
//!
//! ```markdown
//! Table: [Applicable standards]{#rfc-table}
//!
//! | RFC      | Description             |
//! |----------|-------------------------|
//! | RFC 2822 | Internet Message Format |
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
    pp_indent, Arena, KindData, NodeKind, NodeRef, NodeType, PrettyPrint, WalkStatus,
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

/// Detects the caption prefix in a paragraph's first text child.
///
/// Returns `Some((kind, byte_offset_after_prefix))` if a prefix is found.
/// The offset points to the first non-whitespace character after the prefix
/// marker (or to the end of the text if the prefix consumes it entirely).
const CAPTION_PREFIXES: &[(&str, CaptionKind)] = &[
    ("Table:", CaptionKind::Table),
    ("table:", CaptionKind::Table),
    ("Figure:", CaptionKind::Figure),
    ("figure:", CaptionKind::Figure),
];

fn detect_caption_prefix(text_str: &str) -> Option<(CaptionKind, usize)> {
    for &(prefix, kind) in CAPTION_PREFIXES {
        if let Some(rest) = text_str.strip_prefix(prefix) {
            let skip = prefix.len() + rest.len() - rest.trim_start().len();
            return Some((kind, skip));
        }
    }
    // Lone `:` — shorthand for Table caption.
    if text_str.starts_with(':') && !text_str.starts_with("::") {
        let rest = &text_str[1..];
        let skip = 1 + rest.len() - rest.trim_start().len();
        return Some((CaptionKind::Table, skip));
    }
    None
}

/// Returns `true` if `node_ref` is a captionable block.
///
/// Captionable blocks:
/// - `KindData::Table`
/// - `KindData::CodeBlock` with language `mermaid` or `plantuml`
/// - `KindData::Extension(Diagram)` (produced by diagram parser)
/// - `KindData::Extension(FencedDiv)` — excluding those with class `include`
/// - `KindData::Paragraph` containing exactly one `Image` child
fn is_captionable(arena: &Arena, node_ref: NodeRef, source: &str) -> bool {
    match arena[node_ref].kind_data() {
        KindData::Table(_) => true,
        KindData::CodeBlock(cb) => {
            matches!(cb.language_str(source), Some("mermaid" | "plantuml"))
        }
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
        let mut candidates = Vec::new();
        let mut used_targets: Vec<NodeRef> = Vec::new();

        for child in arena[doc_ref].children(arena).collect::<Vec<_>>() {
            let KindData::Paragraph(_) = arena[child].kind_data() else {
                continue;
            };
            // Find the first text child of the paragraph.
            let Some(first_child) = arena[child].children(arena).next() else {
                continue;
            };
            let KindData::Text(text_val) = arena[first_child].kind_data() else {
                continue;
            };
            let text_str = text_val.str(source);
            let Some((kind, prefix_len)) = detect_caption_prefix(text_str) else {
                continue;
            };

            // Look for a captionable sibling: prefer next, then previous.
            let Some((target, position)) =
                find_captionable_sibling(arena, child, source, &used_targets)
            else {
                eprintln!(
                    "warning: caption paragraph has no adjacent captionable block, \
                     rendering as normal paragraph"
                );
                continue;
            };

            // `Table:` prefix targeting a non-table is valid — the explicit
            // prefix overrides the inferred block type.

            used_targets.push(target);
            candidates.push(CaptionCandidate {
                paragraph: child,
                target,
                kind,
                prefix_len,
                position,
            });
        }

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
                        fc.delete(arena);
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
            self.writer.write_safe_str(w, "<figure")?;
            html::render_attributes(w, source, arena[node_ref].attributes(), None)?;
            self.writer.write_safe_str(w, ">\n")?;
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
                    rushdown_fenced_div::FencedDivHtmlRendererOptions::default(),
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
    fn caption_after_table() {
        let source = "| A | B |\n|---|---|\n| 1 | 2 |\n\nTable: My table\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1, "expected one Caption node");
        assert!(matches_extension_kind!(&arena, children[0], Caption));
    }

    #[test]
    fn colon_shorthand_before_table() {
        let source = ": My table\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
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
    fn non_diagram_code_block_not_captionable() {
        let source = "Figure: Not a diagram\n\n```rust\nfn main() {}\n```\n";
        let (arena, doc_ref) = parse(source);

        // `rust` code block is not captionable, so the paragraph stays.
        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert!(
            children.len() >= 2,
            "paragraph and code block should remain separate"
        );
        assert!(matches!(
            arena[children[0]].kind_data(),
            KindData::Paragraph(_)
        ));
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
    fn html_colon_shorthand() {
        let source = ": Short caption\n\n| A |\n|---|\n| 1 |\n";
        let html = render_html(source);

        assert!(html.contains("<figure>"), "should contain <figure>: {html}");
        assert!(
            html.contains("Short caption"),
            "should contain caption text: {html}"
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
        // `: {#figure-ident}` — empty caption text, only attributes.
        let source = ": {#figure-ident}\n\n| A |\n|---|\n| 1 |\n";
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
}
