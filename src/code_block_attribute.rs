//! Fenced code block attribute extension for rushdown.
//!
//! Parses `{#id .class key=val}` attribute blocks from the info string of
//! fenced code blocks and stores them as node attributes.  Also provides an
//! HTML renderer that replaces `rushdown-highlighting` and emits the parsed
//! attributes on the outer `<pre>` element.
//!
//! # Syntax
//!
//! ````markdown
//! ```rust {#example data-line="5"}
//! fn main() {}
//! ```
//! ````
//!
//! Renders as:
//!
//! ```html
//! <pre id="example" data-line="5"
//!      style="background-color: ..."><code class="language-rust">…</code></pre>
//! ```
//!
//! This module is designed to be extracted into a standalone crate later.

use std::any::TypeId;
use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::LazyLock;

use rushdown::as_kind_data;
use rushdown::ast::walk;
use rushdown::ast::{Arena, CodeBlock, CodeBlockKind, NodeRef, WalkStatus};
use rushdown::matches_kind;
use rushdown::parser::{
    self, AnyAstTransformer, AstTransformer, NoParserOptions, Parser, ParserExtension,
    ParserExtensionFn,
};
use rushdown::renderer::html::{self, Renderer, RendererExtension, RendererExtensionFn};
use rushdown::renderer::{
    self, BoxRenderNode, NodeRenderer, NodeRendererRegistry, RenderNode, RendererOptions, TextWrite,
};
use rushdown::text::{self, Reader as _};
use rushdown::Result;

use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::html::{styled_line_to_highlighted_html, IncludeBackground};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

// ============================================================================
// Parser extension (AST transformer)
// ============================================================================

#[derive(Debug)]
struct CodeBlockAttributeAstTransformer;

impl CodeBlockAttributeAstTransformer {
    fn new() -> Self {
        Self
    }
}

impl AstTransformer for CodeBlockAttributeAstTransformer {
    fn transform(
        &self,
        arena: &mut Arena,
        doc_ref: NodeRef,
        reader: &mut text::BasicReader,
        _ctx: &mut parser::Context,
    ) {
        let source = reader.source();

        // Phase 1: collect fenced code blocks that have an attribute block
        // in their info string.
        let mut targets: Vec<(NodeRef, String)> = Vec::new();
        walk(arena, doc_ref, &mut |arena: &Arena,
                                   node_ref: NodeRef,
                                   entering: bool|
         -> Result<WalkStatus> {
            if !entering {
                return Ok(WalkStatus::Continue);
            }
            if !matches_kind!(arena[node_ref], CodeBlock) {
                return Ok(WalkStatus::Continue);
            }
            let code_block = as_kind_data!(arena[node_ref], CodeBlock);
            if code_block.code_block_kind() != CodeBlockKind::Fenced {
                return Ok(WalkStatus::Continue);
            }
            let info = match code_block.info().as_option() {
                Some(value) => value.str(source),
                None => return Ok(WalkStatus::Continue),
            };
            // Find the attribute block — everything from the first '{' that is
            // preceded by a space (or is right after the language token).
            // The language token is the first whitespace-delimited word.
            if let Some(attr_str) = extract_attribute_part(&info) {
                targets.push((node_ref, attr_str.to_string()));
            }
            Ok(WalkStatus::Continue)
        })
        .ok();

        // Phase 2: parse attributes and store them on the nodes.
        for (node_ref, attr_str) in targets {
            let mut attr_reader = text::BasicReader::new(&attr_str);
            let attrs = match parser::parse_attributes(&mut attr_reader) {
                Some(a) => a,
                None => continue,
            };
            // Verify the parser consumed the entire attribute substring.
            if attr_reader.peek_byte() != text::EOS {
                continue;
            }
            // Store parsed attributes on the node.
            for (key, value) in attrs.iter() {
                let resolved = value.str(&attr_str).to_string();
                arena[node_ref]
                    .attributes_mut()
                    .insert(key.clone(), resolved.into());
            }
        }
    }
}

/// Extracts the `{...}` attribute portion from a fenced code block info string.
///
/// The info string has the form `language {#id .class key=val}` or just
/// `{#id .class}` (no language).  Returns the `{...}` substring if found,
/// or `None` if not present.
pub(crate) fn extract_attribute_part(info: &str) -> Option<&str> {
    // Look for a '{' that could start an attribute block.
    // It must appear after the language token (separated by whitespace),
    // or at the very start if there is no language.
    let trimmed = info.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Find the first '{' in the trimmed info string.
    let brace_pos = trimmed.find('{')?;

    // Everything before the brace must be either empty or end with whitespace
    // (i.e., the brace is not embedded in the language token).
    if brace_pos > 0 {
        let before = &trimmed[..brace_pos];
        if !before.ends_with(char::is_whitespace) {
            return None;
        }
    }

    let attr_part = trimmed[brace_pos..].trim_end();

    // Quick sanity check: must end with '}'.
    if !attr_part.ends_with('}') {
        return None;
    }

    Some(attr_part)
}

impl From<CodeBlockAttributeAstTransformer> for AnyAstTransformer {
    fn from(t: CodeBlockAttributeAstTransformer) -> Self {
        AnyAstTransformer::Extension(Box::new(t))
    }
}

/// Returns a parser extension that parses `{#id .class key=val}` attributes
/// from fenced code block info strings and stores them as node attributes.
pub fn code_block_attribute_parser_extension() -> impl ParserExtension {
    ParserExtensionFn::new(|p: &mut Parser| {
        p.add_ast_transformer(CodeBlockAttributeAstTransformer::new, NoParserOptions, 50);
    })
}

// ============================================================================
// HTML renderer extension
// ============================================================================

/// Options for the code block attribute HTML renderer.
#[derive(Debug, Clone, Default)]
pub struct CodeBlockAttributeHtmlRendererOptions;

impl RendererOptions for CodeBlockAttributeHtmlRendererOptions {}

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static DEFAULT_THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

struct CodeBlockAttributeHtmlRenderer<W: TextWrite> {
    _phantom: core::marker::PhantomData<W>,
}

impl<W: TextWrite> CodeBlockAttributeHtmlRenderer<W> {
    fn with_options(_options: CodeBlockAttributeHtmlRendererOptions) -> Self {
        Self {
            _phantom: core::marker::PhantomData,
        }
    }

    /// Renders a code block with inline style-attribute highlighting.
    fn render_code_attr_mode(
        &self,
        w: &mut W,
        source: &str,
        arena: &Arena,
        node_ref: NodeRef,
        code_block: &CodeBlock,
    ) -> Result<()> {
        let language = code_block.language(source);
        let language = language
            .as_option()
            .map_or(Cow::Borrowed("plaintext"), |v| v.str(source));
        let lang: &str = &language;
        let mut code = String::new();
        for line in code_block.value().iter(source) {
            code.push_str(&line);
        }

        let theme = &DEFAULT_THEME_SET.themes["InspiredGitHub"];

        let ps = &*SYNTAX_SET;
        let syntax = ps
            .find_syntax_by_token(if lang.is_empty() { "plaintext" } else { lang })
            .or_else(|| ps.find_syntax_by_extension(lang))
            .unwrap_or_else(|| ps.find_syntax_plain_text());

        let bg = theme
            .settings
            .background
            .map(|c| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b))
            .unwrap_or_else(|| "#ffffff".to_string());

        // Check for the `start` attribute — when present, enable line numbering.
        let start_line: Option<u64> = arena[node_ref]
            .attributes()
            .get("start")
            .and_then(|s| s.str(source).parse::<u64>().ok());

        // Open <pre> with background style and node attributes.
        w.write_str("<pre style=\"background-color: ")?;
        w.write_str(&bg)?;
        w.write_str("; padding: 12px; overflow: auto;")?;
        if let Some(start) = start_line {
            // CSS counter starting at (start - 1) so the first increment
            // produces the correct starting number.
            let mut buf = String::new();
            write!(
                buf,
                " counter-reset: line-number {};",
                start.saturating_sub(1)
            )
            .expect("write to String never fails");
            w.write_str(&buf)?;
        }
        w.write_str("\"")?;
        if start_line.is_some() {
            w.write_str(" class=\"line-numbers\"")?;
        }
        html::render_attributes(w, source, arena[node_ref].attributes(), None)?;
        w.write_str("><code class=\"language-")?;
        w.write_str(lang)?;
        w.write_str("\">")?;

        let mut h = HighlightLines::new(syntax, theme);
        for line in LinesWithEndings::from(&code) {
            if start_line.is_some() {
                w.write_str("<span class=\"code-line\">")?;
            }
            if let Ok(regions) = h.highlight_line(line, ps) {
                if let Ok(html_line) =
                    styled_line_to_highlighted_html(&regions[..], IncludeBackground::No)
                {
                    w.write_str(&html_line)?;
                    if start_line.is_some() {
                        w.write_str("</span>")?;
                    }
                    continue;
                }
            }
            // Fallback: write the raw line (HTML-escaped by the writer).
            html::write_text(w, line)?;
            if start_line.is_some() {
                w.write_str("</span>")?;
            }
        }

        w.write_str("</code></pre>\n")?;
        Ok(())
    }

    /// Renders a code block without syntax highlighting (plain fallback).
    #[allow(dead_code)]
    fn render_code_plain(
        &self,
        w: &mut W,
        source: &str,
        arena: &Arena,
        node_ref: NodeRef,
        code_block: &CodeBlock,
    ) -> Result<()> {
        w.write_str("<pre")?;
        html::render_attributes(w, source, arena[node_ref].attributes(), None)?;
        w.write_str("><code")?;
        let lang = code_block.language(source);
        if let Some(lang) = lang.as_option() {
            w.write_str(" class=\"language-")?;
            lang.write_to(&mut html::TextWriter::new(w), source)?;
            w.write_str("\"")?;
        }
        w.write_str(">")?;
        for line in code_block.value().iter(source) {
            html::write_text(w, &line)?;
        }
        w.write_str("</code></pre>\n")?;
        Ok(())
    }
}

impl<W: TextWrite> RenderNode<W> for CodeBlockAttributeHtmlRenderer<W> {
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
            let code_block = as_kind_data!(arena, node_ref, CodeBlock);
            self.render_code_attr_mode(w, source, arena, node_ref, code_block)?;
        }
        Ok(WalkStatus::SkipChildren)
    }
}

impl<'r, W> NodeRenderer<'r, W> for CodeBlockAttributeHtmlRenderer<W>
where
    W: TextWrite + 'r,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'r, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<CodeBlock>(), BoxRenderNode::new(self));
    }
}

/// Returns an HTML renderer extension that renders code blocks with syntax
/// highlighting and emits parsed `{#id .class key=val}` attributes on the
/// outer `<pre>` element.
///
/// This extension **replaces** `rushdown-highlighting`'s code block renderer;
/// do not register both.
pub fn code_block_attribute_html_renderer_extension<'cb, W>(
    options: CodeBlockAttributeHtmlRendererOptions,
) -> impl RendererExtension<'cb, W>
where
    W: TextWrite + 'cb,
{
    RendererExtensionFn::new(move |r: &mut Renderer<'cb, W>| {
        r.add_node_renderer(CodeBlockAttributeHtmlRenderer::with_options, options);
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_with_code_block_attrs(input: &str) -> (Arena, NodeRef) {
        let parser = parser::Parser::with_extensions(
            parser::Options {
                attributes: true,
                auto_heading_ids: false,
                ..Default::default()
            },
            code_block_attribute_parser_extension(),
        );
        parser.parse(input, rushdown::parser::ParseOptions::default())
    }

    fn first_code_block(arena: &Arena, doc_ref: NodeRef) -> Option<NodeRef> {
        let mut result = None;
        rushdown::ast::walk(arena, doc_ref, &mut |arena: &Arena,
                                                  node_ref: NodeRef,
                                                  entering: bool|
         -> Result<WalkStatus> {
            if entering && matches_kind!(arena[node_ref], CodeBlock) {
                result = Some(node_ref);
                return Ok(WalkStatus::SkipChildren);
            }
            Ok(WalkStatus::Continue)
        })
        .ok();
        result
    }

    #[test]
    fn parses_id_and_class() {
        let input = "```rust {#example .note}\nfn main() {}\n```\n";
        let (arena, doc_ref) = parse_with_code_block_attrs(input);
        let cb = first_code_block(&arena, doc_ref).expect("code block not found");

        let code = as_kind_data!(arena[cb], CodeBlock);
        assert_eq!(code.language(input).str(input), "rust");

        let attrs = arena[cb].attributes();
        assert!(attrs.get("id").is_some_and(|v| v.str(input) == "example"));
        assert!(attrs.get("class").is_some_and(|v| v.str(input) == "note"));
    }

    #[test]
    fn parses_custom_attributes() {
        let input = "```python {.warning data-line=\"5\"}\nprint(\"hi\")\n```\n";
        let (arena, doc_ref) = parse_with_code_block_attrs(input);
        let cb = first_code_block(&arena, doc_ref).expect("code block not found");

        let attrs = arena[cb].attributes();
        assert!(attrs
            .get("class")
            .is_some_and(|v| v.str(input) == "warning"));
        assert!(attrs.get("data-line").is_some_and(|v| v.str(input) == "5"));
    }

    #[test]
    fn no_attributes_when_absent() {
        let input = "```rust\nfn main() {}\n```\n";
        let (arena, doc_ref) = parse_with_code_block_attrs(input);
        let cb = first_code_block(&arena, doc_ref).expect("code block not found");
        assert!(arena[cb].attributes().is_empty());
    }

    #[test]
    fn malformed_attributes_ignored() {
        let input = "```rust {broken\nfn main() {}\n```\n";
        let (arena, doc_ref) = parse_with_code_block_attrs(input);
        let cb = first_code_block(&arena, doc_ref).expect("code block not found");
        assert!(arena[cb].attributes().is_empty());
    }

    #[test]
    fn language_unaffected() {
        let input = "```javascript {#my-code}\nconsole.log('hi');\n```\n";
        let (arena, doc_ref) = parse_with_code_block_attrs(input);
        let cb = first_code_block(&arena, doc_ref).expect("code block not found");
        let code = as_kind_data!(arena[cb], CodeBlock);
        assert_eq!(code.language(input).str(input), "javascript");
    }

    #[test]
    fn attributes_only_no_language() {
        let input = "``` {#my-id}\nsome code\n```\n";
        let (arena, doc_ref) = parse_with_code_block_attrs(input);
        let cb = first_code_block(&arena, doc_ref).expect("code block not found");

        let attrs = arena[cb].attributes();
        assert!(attrs.get("id").is_some_and(|v| v.str(input) == "my-id"));

        // language() returns the first whitespace-delimited token from the
        // raw info string.  When the info string is just `{#my-id}` (no
        // language token), language() returns `"{#my-id}"` because
        // the underlying parser does not strip attribute blocks from the info
        // string.  The attribute block itself is parsed separately by the
        // transformer.  Renderers will not recognise `{#my-id}` as a valid
        // language and will fall back to plain text — this is acceptable.
        let code = as_kind_data!(arena[cb], CodeBlock);
        assert_eq!(code.language(input).str(input), "{#my-id}");
    }

    #[test]
    fn brace_in_language_not_parsed() {
        // A '{' embedded in the language token (no preceding space) should
        // not be treated as an attribute block.
        let input = "```lang{foo}\ncode\n```\n";
        let (arena, doc_ref) = parse_with_code_block_attrs(input);
        let cb = first_code_block(&arena, doc_ref).expect("code block not found");
        assert!(arena[cb].attributes().is_empty());
    }

    #[test]
    fn html_rendering_emits_attributes() {
        let input = "```rust {#example .note}\nfn main() {}\n```\n";
        let (arena, doc_ref) = parse_with_code_block_attrs(input);

        let renderer = html::Renderer::with_extensions(
            html::Options::default(),
            code_block_attribute_html_renderer_extension(CodeBlockAttributeHtmlRendererOptions),
        );
        let mut output = String::new();
        renderer
            .render(
                &mut output,
                input,
                &arena,
                doc_ref,
                rushdown::renderer::RenderOptions::default(),
            )
            .unwrap();

        assert!(
            output.contains("id=\"example\""),
            "output should contain id attribute: {output}"
        );
        assert!(
            output.contains("class=\"note\""),
            "output should contain class attribute: {output}"
        );
        assert!(
            output.contains("class=\"language-rust\""),
            "output should contain language class on <code>: {output}"
        );
    }

    #[test]
    fn html_rendering_without_attributes() {
        let input = "```rust\nfn main() {}\n```\n";
        let (arena, doc_ref) = parse_with_code_block_attrs(input);

        let renderer = html::Renderer::with_extensions(
            html::Options::default(),
            code_block_attribute_html_renderer_extension(CodeBlockAttributeHtmlRendererOptions),
        );
        let mut output = String::new();
        renderer
            .render(
                &mut output,
                input,
                &arena,
                doc_ref,
                rushdown::renderer::RenderOptions::default(),
            )
            .unwrap();

        assert!(
            output.contains("<pre"),
            "output should contain <pre>: {output}"
        );
        assert!(
            output.contains("class=\"language-rust\""),
            "output should contain language class: {output}"
        );
    }

    #[test]
    fn html_rendering_with_start_attribute() {
        let input = "```rust {start=10}\nfn main() {}\n```\n";
        let (arena, doc_ref) = parse_with_code_block_attrs(input);

        let renderer = html::Renderer::with_extensions(
            html::Options::default(),
            code_block_attribute_html_renderer_extension(CodeBlockAttributeHtmlRendererOptions),
        );
        let mut output = String::new();
        renderer
            .render(
                &mut output,
                input,
                &arena,
                doc_ref,
                rushdown::renderer::RenderOptions::default(),
            )
            .unwrap();

        assert!(
            output.contains("class=\"line-numbers\""),
            "output should contain line-numbers class: {output}"
        );
        assert!(
            output.contains("counter-reset: line-number 9;"),
            "output should contain counter-reset with start-1: {output}"
        );
        assert!(
            output.contains("<span class=\"code-line\">"),
            "output should contain code-line spans: {output}"
        );
    }

    #[test]
    fn html_rendering_without_start_has_no_line_numbers() {
        let input = "```rust\nfn main() {}\n```\n";
        let (arena, doc_ref) = parse_with_code_block_attrs(input);

        let renderer = html::Renderer::with_extensions(
            html::Options::default(),
            code_block_attribute_html_renderer_extension(CodeBlockAttributeHtmlRendererOptions),
        );
        let mut output = String::new();
        renderer
            .render(
                &mut output,
                input,
                &arena,
                doc_ref,
                rushdown::renderer::RenderOptions::default(),
            )
            .unwrap();

        assert!(
            !output.contains("line-numbers"),
            "output should NOT contain line-numbers class: {output}"
        );
        assert!(
            !output.contains("code-line"),
            "output should NOT contain code-line spans: {output}"
        );
    }

    #[test]
    fn extract_attribute_part_cases() {
        assert_eq!(
            extract_attribute_part("rust {#id .cls}"),
            Some("{#id .cls}")
        );
        assert_eq!(extract_attribute_part("{#id}"), Some("{#id}"));
        assert_eq!(extract_attribute_part("rust"), None);
        assert_eq!(extract_attribute_part(""), None);
        assert_eq!(extract_attribute_part("lang{foo}"), None);
        assert_eq!(extract_attribute_part("rust {broken"), None);
    }
}
