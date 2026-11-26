//! Pandoc-style bracketed spans extension for rushdown.
//!
//! Syntax: `[inline content]{#id .class key="value"}`
//!
//! Renders as: `<span id="id" class="class" key="value">inline content</span>`
//!
//! This module is designed to be extracted into a standalone crate later.
//! It follows the same patterns as `rushdown-link-attribute` and `rushdown-fenced-div`.

use std::any::TypeId;
use std::cell::RefCell;
use std::fmt;
use std::fmt::Write;
use std::rc::Rc;

use rushdown::as_kind_data;
use rushdown::as_kind_data_mut;
use rushdown::ast::{
    pp_indent, Arena, KindData, NodeKind, NodeRef, NodeType, PrettyPrint, Text, WalkStatus,
};
use rushdown::context::{ContextKey, ContextKeyRegistry, ObjectValue};
use rushdown::matches_kind;
use rushdown::parser::{
    self, parse_attributes, parser_extension, AnyInlineParser, InlineParser, NoParserOptions,
    Parser, ParserExtension,
};
use rushdown::renderer::html::{self, Renderer, RendererExtension, RendererExtensionFn};
use rushdown::renderer::{
    self, BoxRenderNode, NodeRenderer, NodeRendererRegistry, RenderNode, RendererOptions, TextWrite,
};
use rushdown::text::{self, Reader as _};
use rushdown::Result;

// AST {{{

/// AST node for a bracketed span container.
///
/// Represents `[inline content]{attributes}` in the source.
#[derive(Debug)]
pub struct BracketedSpan;

impl rushdown::ast::InlineChars for BracketedSpan {}

impl NodeKind for BracketedSpan {
    fn typ(&self) -> NodeType {
        NodeType::Inline
    }

    fn kind_name(&self) -> &'static str {
        "BracketedSpan"
    }
}

impl PrettyPrint for BracketedSpan {
    fn pretty_print(&self, w: &mut dyn Write, _source: &str, level: usize) -> fmt::Result {
        writeln!(w, "{}BracketedSpan", pp_indent(level))
    }
}

impl From<BracketedSpan> for KindData {
    fn from(e: BracketedSpan) -> Self {
        KindData::Extension(Box::new(e))
    }
}

// }}} AST

// Parser {{{

/// Inline parser for bracketed spans.
///
/// Triggers on `{` and walks backwards through siblings to find a matching
/// `[` text node. All inline content between `[` and `]` is reparented into
/// a new [`BracketedSpan`] node.
///
/// The parser runs at priority 600, after the built-in `LinkParser` (200) and
/// `rushdown-link-attribute` (500). When `LinkParser` encounters `[text]` and
/// fails to match a link destination (`(url)` or `[ref]`), the `[` and `]`
/// become regular text nodes. This parser then picks up `{`, verifies the
/// preceding `]`, finds the matching `[`, and creates the span.
#[derive(Debug)]
struct BracketedSpanParser {
    /// Tracks zero-width text nodes created by this parser, cleaned up on block close.
    cleanup_list: ContextKey<ObjectValue>,
}

impl BracketedSpanParser {
    fn new(reg: Rc<RefCell<ContextKeyRegistry>>) -> Self {
        Self {
            cleanup_list: reg.borrow_mut().create::<ObjectValue>(),
        }
    }
}

impl InlineParser for BracketedSpanParser {
    fn trigger(&self) -> &[u8] {
        b"{"
    }

    fn parse(
        &self,
        arena: &mut Arena,
        parent_ref: NodeRef,
        reader: &mut text::BlockReader,
        ctx: &mut parser::Context,
    ) -> Option<NodeRef> {
        // The last child should be a Text node ending with `]`.
        let last = arena[parent_ref].last_child()?;
        if !matches_kind!(arena[last], Text) {
            return None;
        }
        let source = reader.source();
        let last_str = as_kind_data!(arena[last], Text).value().str(source);
        if !last_str.ends_with(']') {
            return None;
        }

        // Try to parse attributes from the reader. If this fails, bail out
        // and the `{` is treated as regular text.
        let attributes = parse_attributes(reader)?;

        // Walk backwards through siblings to find the matching `[`.
        let (open_node, open_byte_offset) = find_matching_open_bracket(arena, last, source)?;

        // Create the BracketedSpan node with the parsed attributes.
        let span_ref = arena.new_node(BracketedSpan);
        arena[span_ref].attributes_mut().extend(attributes);

        // Trim `]` from the last text node.
        // When open_node == last (e.g. `[]`), the `]` trimming happens on the
        // same node that also contains `[`. We handle both together below.
        let same_node = open_node == last;
        trim_text_end(arena, last, source, 1);

        // Split the open_node at the `[` position:
        // - Content before `[` stays in open_node
        // - `[` is discarded
        // - Content after `[` (up to the already-trimmed `]`) becomes a new text node

        let after_bracket_node =
            create_after_bracket_node(arena, open_node, open_byte_offset, source);
        if let Some(after_node) = after_bracket_node {
            parent_ref.insert_after(arena, open_node, after_node);
        }

        // Truncate open_node to content before `[` (may become empty).
        truncate_text_at(arena, open_node, open_byte_offset, source);

        // Determine the first node to reparent into the span.
        // When open_node == last and there's no after_bracket_node, the span is empty.
        let first_content = if same_node {
            after_bracket_node
        } else {
            after_bracket_node.or_else(|| arena[open_node].next_sibling())
        };

        // Reparent all nodes from first_content through last (inclusive) into the span.
        // When same_node is true and there is no after_bracket_node, nothing to reparent.
        if let Some(first) = first_content {
            let reparent_last = if same_node {
                // When open_node == last, the after_bracket_node (if any) is the only
                // content to reparent, and it's the "last" for reparenting purposes.
                after_bracket_node.unwrap_or(first)
            } else {
                last
            };
            reparent_siblings(arena, span_ref, Some(first), reparent_last);
        }

        // Insert the span after open_node (which now contains only text before `[`).
        parent_ref.insert_after(arena, open_node, span_ref);

        // Clean up open_node if it became empty.
        if is_text_empty(arena, open_node, source) {
            open_node.delete(arena);
        }

        // Clean up the last text node if it became empty after trimming `]`.
        // Skip if same_node — already handled above as open_node.
        if !same_node && arena[last].parent().is_some() && is_text_empty(arena, last, source) {
            last.delete(arena);
        }

        // Return a zero-width text node. The inline parse loop will append it at the end
        // of the block. We clean it up in close_block.
        let placeholder = arena.new_node(Text::new(text::Value::from_index(
            text::Index::new(0, 0),
            text::Decoder::Default,
        )));
        track_placeholder(ctx, self.cleanup_list, placeholder);
        Some(placeholder)
    }

    fn close_block(
        &self,
        arena: &mut Arena,
        _node_ref: NodeRef,
        _reader: &mut text::BlockReader,
        ctx: &mut parser::Context,
    ) {
        if let Some(list) = ctx.get_mut(self.cleanup_list) {
            let list = list.downcast_mut::<Vec<NodeRef>>().unwrap();
            for n in list.drain(..) {
                n.delete(arena);
            }
        }
    }
}

impl From<BracketedSpanParser> for AnyInlineParser {
    fn from(p: BracketedSpanParser) -> Self {
        AnyInlineParser::Extension(Box::new(p))
    }
}

/// Track a placeholder node for cleanup on block close.
fn track_placeholder(ctx: &mut parser::Context, key: ContextKey<ObjectValue>, node: NodeRef) {
    let mut list_opt = ctx.get_mut(key);
    if list_opt.is_none() {
        ctx.insert(key, Box::new(Vec::<NodeRef>::new()));
        list_opt = ctx.get_mut(key);
    }
    let list = list_opt.unwrap().downcast_mut::<Vec<NodeRef>>().unwrap();
    list.push(node);
}

/// Find the matching `[` for a `]` at the end of `last_node`.
///
/// Returns the node containing `[` and the byte offset of `[` within that node's text.
/// Handles balanced brackets: nested `[...]` pairs are skipped.
fn find_matching_open_bracket(
    arena: &Arena,
    last_node: NodeRef,
    source: &str,
) -> Option<(NodeRef, usize)> {
    let mut depth: i32 = 0;
    let mut current = Some(last_node);

    while let Some(node) = current {
        if matches_kind!(arena[node], Text) {
            let text = as_kind_data!(arena[node], Text);
            let s = text.value().str(source);

            // For last_node, skip the trailing `]` we already accounted for.
            let scan_bytes = if node == last_node {
                &s.as_bytes()[..s.len() - 1]
            } else {
                s.as_bytes()
            };

            // Scan backwards through the text.
            let mut i = scan_bytes.len();
            while i > 0 {
                i -= 1;
                let c = scan_bytes[i];
                if c == b']' {
                    if i > 0 && scan_bytes[i - 1] == b'\\' {
                        i -= 1;
                        continue;
                    }
                    depth += 1;
                } else if c == b'[' {
                    if i > 0 && scan_bytes[i - 1] == b'\\' {
                        i -= 1;
                        continue;
                    }
                    if depth == 0 {
                        return Some((node, i));
                    }
                    depth -= 1;
                }
            }
        }
        current = arena[node].previous_sibling();
    }
    None
}

/// Create a text node for content after `[` in the open_node, if any.
fn create_after_bracket_node(
    arena: &mut Arena,
    open_node: NodeRef,
    open_byte_offset: usize,
    source: &str,
) -> Option<NodeRef> {
    let text = as_kind_data!(arena[open_node], Text);
    let s = text.value().str(source);
    let after_bracket = &s[open_byte_offset + 1..];
    if after_bracket.is_empty() {
        return None;
    }

    let idx = text.index().cloned();
    if let Some(idx) = idx {
        let after_start = idx.start() + open_byte_offset + 1;
        Some(arena.new_node(Text::new(text::Value::from_index(
            text::Index::new(after_start, idx.stop()),
            text::Decoder::Default,
        ))))
    } else {
        Some(arena.new_node(Text::new(after_bracket.to_string())))
    }
}

/// Truncate a text node to keep only content before `byte_offset`.
fn truncate_text_at(arena: &mut Arena, node: NodeRef, byte_offset: usize, source: &str) {
    let text = as_kind_data!(arena[node], Text);
    if let Some(idx) = text.index().cloned() {
        as_kind_data_mut!(arena[node], Text).set(text::Value::from_index(
            text::Index::new(idx.start(), idx.start() + byte_offset),
            text::Decoder::Default,
        ));
    } else {
        let s = text.value().str(source).to_string();
        as_kind_data_mut!(arena[node], Text).set(s[..byte_offset].to_string());
    }
}

/// Trim `n` bytes from the end of a Text node.
fn trim_text_end(arena: &mut Arena, node: NodeRef, source: &str, n: usize) {
    let text = as_kind_data!(arena[node], Text);
    if let Some(idx) = text.index().cloned() {
        let new_stop = idx.stop().saturating_sub(n);
        as_kind_data_mut!(arena[node], Text).set(text::Value::from_index(
            text::Index::new(idx.start(), new_stop),
            text::Decoder::Default,
        ));
    } else {
        let s = text.value().str(source).to_string();
        as_kind_data_mut!(arena[node], Text).set(s[..s.len().saturating_sub(n)].to_string());
    }
}

/// Check if a text node is empty.
fn is_text_empty(arena: &Arena, node: NodeRef, source: &str) -> bool {
    let text = as_kind_data!(arena[node], Text);
    if let Some(idx) = text.index() {
        idx.is_empty()
    } else {
        text.value().str(source).is_empty()
    }
}

/// Reparent all siblings from `first` (inclusive) through `last` (inclusive) into `new_parent`.
fn reparent_siblings(
    arena: &mut Arena,
    new_parent: NodeRef,
    first: Option<NodeRef>,
    last: NodeRef,
) {
    let mut current = first;
    while let Some(node) = current {
        let next = arena[node].next_sibling();
        let is_last = node == last;
        new_parent.append_child(arena, node);
        if is_last {
            break;
        }
        current = next;
    }
}

/// Returns a parser extension that parses bracketed spans (`[text]{attributes}`).
pub fn bracketed_span_parser_extension() -> impl ParserExtension {
    parser_extension(|p: &mut Parser| {
        p.add_inline_parser(BracketedSpanParser::new, NoParserOptions, 600);
    })
}

// }}} Parser

// HTML Renderer {{{

/// Options for the bracketed span HTML renderer.
#[derive(Debug, Clone, Default)]
pub struct BracketedSpanHtmlRendererOptions;

impl RendererOptions for BracketedSpanHtmlRendererOptions {}

/// HTML renderer for [`BracketedSpan`] nodes.
///
/// Renders as `<span ...attributes>children</span>`.
struct BracketedSpanHtmlRenderer<W: TextWrite> {
    _phantom: core::marker::PhantomData<W>,
}

impl<W: TextWrite> BracketedSpanHtmlRenderer<W> {
    fn with_options(_options: BracketedSpanHtmlRendererOptions) -> Self {
        Self {
            _phantom: core::marker::PhantomData,
        }
    }
}

impl<W: TextWrite> RenderNode<W> for BracketedSpanHtmlRenderer<W> {
    fn render_node<'a>(
        &self,
        w: &mut W,
        source: &'a str,
        arena: &'a Arena,
        node_ref: NodeRef,
        entering: bool,
        _ctx: &mut renderer::Context,
    ) -> Result<WalkStatus> {
        if entering {
            w.write_str("<span")?;
            html::render_attributes(w, source, arena[node_ref].attributes(), None)?;
            w.write_str(">")?;
        } else {
            w.write_str("</span>")?;
        }
        Ok(WalkStatus::Continue)
    }
}

impl<'cb, W> NodeRenderer<'cb, W> for BracketedSpanHtmlRenderer<W>
where
    W: TextWrite + 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<BracketedSpan>(), BoxRenderNode::new(self));
    }
}

/// Returns a renderer extension that renders bracketed spans in HTML.
pub fn bracketed_span_html_renderer_extension<'cb, W>() -> impl RendererExtension<'cb, W>
where
    W: TextWrite + 'cb,
{
    RendererExtensionFn::new(move |r: &mut Renderer<'cb, W>| {
        r.add_node_renderer(
            BracketedSpanHtmlRenderer::with_options,
            BracketedSpanHtmlRendererOptions,
        );
    })
}

// }}} HTML Renderer

// Tests {{{

#[cfg(test)]
mod tests {
    use super::*;
    use rushdown::ast::KindData;
    use rushdown::parser;
    use rushdown::renderer::html;

    /// Build a parser with GFM + bracketed span extension (and link-attribute,
    /// since it also triggers on `{` and must coexist).
    fn test_parser() -> parser::Parser {
        parser::Parser::with_extensions(
            parser::Options {
                attributes: true,
                ..Default::default()
            },
            parser::gfm(parser::GfmOptions::default())
                .and(rushdown_link_attribute::link_attribute_parser_extension())
                .and(bracketed_span_parser_extension()),
        )
    }

    /// Parse markdown and render to HTML.
    fn render_html(source: &str) -> String {
        let parser = test_parser();
        let (arena, doc_ref) = parser.parse(source, rushdown::parser::ParseOptions::default());
        let renderer = html::Renderer::with_extensions(
            html::Options::default(),
            bracketed_span_html_renderer_extension(),
        );
        let mut output = String::new();
        renderer
            .render(
                &mut output,
                source,
                &arena,
                doc_ref,
                rushdown::renderer::RenderOptions::default(),
            )
            .unwrap();
        output
    }

    #[test]
    fn basic_class() {
        let source = "[text]{.warning}\n";
        let parser = test_parser();
        let (arena, doc_ref) = parser.parse(source, rushdown::parser::ParseOptions::default());

        // Check AST: paragraph -> BracketedSpan
        let para = arena[doc_ref].children(&arena).next().unwrap();
        assert!(matches!(arena[para].kind_data(), KindData::Paragraph(_)));
        let span = arena[para]
            .children(&arena)
            .find(|n| rushdown::matches_extension_kind!(&arena, *n, BracketedSpan))
            .expect("BracketedSpan node not found in AST");

        let class = arena[span]
            .attributes()
            .get("class")
            .map(|v| v.str(source))
            .unwrap_or_default();
        assert_eq!(class, "warning");

        // Check HTML rendering
        let html = render_html(source);
        assert!(
            html.contains(r#"<span class="warning">text</span>"#),
            "unexpected HTML: {html}"
        );
    }

    #[test]
    fn multiple_attributes() {
        let html = render_html("[text]{#notice .alert data-level=\"high\"}\n");
        assert!(
            html.contains(r#"<span id="notice" class="alert" data-level="high">text</span>"#),
            "unexpected HTML: {html}"
        );
    }

    #[test]
    fn with_emphasis() {
        let html = render_html("[**bold** and *italic*]{.highlight}\n");
        assert!(
            html.contains(
                "<span class=\"highlight\"><strong>bold</strong> and <em>italic</em></span>"
            ),
            "unexpected HTML: {html}"
        );
    }

    #[test]
    fn empty_content() {
        let html = render_html("[]{.empty}\n");
        assert!(
            html.contains(r#"<span class="empty"></span>"#),
            "unexpected HTML: {html}"
        );
    }

    #[test]
    fn does_not_interfere_with_links() {
        let html = render_html("[link text](https://example.com)\n");
        assert!(
            html.contains(r#"<a href="https://example.com">link text</a>"#),
            "unexpected HTML: {html}"
        );
    }

    #[test]
    fn text_before_bracket() {
        let html = render_html("before [span]{.cls} after\n");
        assert!(
            html.contains(r#"before <span class="cls">span</span> after"#),
            "unexpected HTML: {html}"
        );
    }

    #[test]
    fn brackets_without_attributes_are_plain_text() {
        let html = render_html("[just brackets]\n");
        assert!(!html.contains("<span"), "should not contain <span>: {html}");
    }

    #[test]
    fn space_between_bracket_and_brace_is_not_a_span() {
        let html = render_html("[text] {.class}\n");
        assert!(
            !html.contains("<span"),
            "space between ] and {{ should prevent span: {html}"
        );
    }
}

// }}} Tests
