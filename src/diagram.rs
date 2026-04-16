//! Internal diagram extension for rushdown.
//!
//! Supports:
//!
//! - **Mermaid** diagrams — client-side JS rendering (HTML), SVG via
//!   `mermaid-cli` (`mmdc`) with built-in `mermaid-rs-renderer` fallback
//!   for PDF output.
//! - **PlantUML** diagrams — server-side SVG via the external `plantuml` CLI.
//! - **Typst** diagrams — direct embedding in PDF output, SVG via `typst
//!   compile` for HTML output.
//! - **Draw.io** diagrams — SVG for HTML output, PDF for Typst/PDF output
//!   (avoids the `<foreignObject>` problem in Typst's SVG renderer).
//!   Rendering is deferred to the render stage so each renderer can choose
//!   the appropriate output format.
//!
//! Artifact caching is supported via the [`crate::cache::Cache`] struct
//! threaded through option structs.  When the cache is enabled, rendered
//! output is stored as `<blake3-hex>.<renderer>` and reused on subsequent
//! builds if the source content has not changed.

use std::any::TypeId;
use std::cell::RefCell;
use std::fmt;
use std::fmt::Write;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;

use anyhow::Context as _;

use rushdown::as_extension_data;
use rushdown::as_extension_data_mut;
use rushdown::as_kind_data;
use rushdown::as_type_data;
use rushdown::as_type_data_mut;
use rushdown::ast::walk;
use rushdown::ast::{
    pp_indent, Arena, KindData, NodeKind, NodeRef, NodeType, PrettyPrint, WalkStatus,
};
use rushdown::context::{BoolValue, ContextKey, ContextKeyRegistry};
use rushdown::matches_kind;
use rushdown::parser::{
    self, AnyAstTransformer, AstTransformer, Parser, ParserExtension, ParserExtensionFn,
    ParserOptions,
};
use rushdown::renderer::html::{self, Renderer, RendererExtension, RendererExtensionFn};
use rushdown::renderer::{
    self, BoxRenderNode, NodeRenderer, NodeRendererRegistry, PostRender, Render, RenderNode,
    RendererOptions, TextWrite,
};
use rushdown::text::{self, Reader as _};
use rushdown::Result;

// ============================================================================
// AST node types
// ============================================================================

/// The type of diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramType {
    Mermaid,
    PlantUml,
    Typst,
    DrawIo,
    D2,
}

/// A diagram node in the AST, replacing a fenced code block with a
/// diagram-language info string (`mermaid`, `plantuml`), or an image
/// reference to a `.puml`/`.mmd`/`.drawio` file.
#[derive(Debug)]
pub struct Diagram {
    diagram_type: DiagramType,
    value: text::Lines,
    /// Diagram source read from an external file (`.puml`, `.mmd`).
    /// When set, takes precedence over `value` during rendering.
    file_source: Option<String>,
    /// Resolved filesystem path to a `.drawio` file.  Set by
    /// [`transform_diagram_images`] for deferred rendering — the actual
    /// conversion to SVG or PDF happens at render time so that each
    /// renderer can choose the appropriate output format.
    drawio_path: Option<PathBuf>,
    /// Optional 1-based page index within a multi-page `.drawio` file
    /// (extracted from a `#N` URL fragment, e.g. `diagram.drawio#2`).
    drawio_page: Option<u32>,
}

impl Diagram {
    pub fn new(diagram_type: DiagramType) -> Self {
        Self {
            diagram_type,
            value: text::Lines::default(),
            file_source: None,
            drawio_path: None,
            drawio_page: None,
        }
    }

    /// Creates a new [`Diagram`] whose source was read from an external file.
    pub fn from_file(diagram_type: DiagramType, content: String) -> Self {
        Self {
            diagram_type,
            value: text::Lines::default(),
            file_source: Some(content),
            drawio_path: None,
            drawio_page: None,
        }
    }

    /// Creates a new [`Diagram`] that references a `.drawio` file for
    /// deferred rendering.  The actual SVG/PDF conversion is performed at
    /// render time so that each renderer can choose the appropriate format.
    pub fn from_drawio(path: PathBuf, page: Option<u32>) -> Self {
        Self {
            diagram_type: DiagramType::DrawIo,
            value: text::Lines::default(),
            file_source: None,
            drawio_path: Some(path),
            drawio_page: page,
        }
    }

    /// Returns the type of the diagram.
    pub fn diagram_type(&self) -> DiagramType {
        self.diagram_type
    }

    /// Returns the lines of the diagram source (from a fenced code block).
    pub fn value(&self) -> &text::Lines {
        &self.value
    }

    /// Returns the file-sourced content, if any.
    pub fn file_source(&self) -> Option<&str> {
        self.file_source.as_deref()
    }

    /// Returns the resolved path to a `.drawio` file, if this diagram was
    /// created via [`Diagram::from_drawio`].
    pub fn drawio_path(&self) -> Option<&Path> {
        self.drawio_path.as_deref()
    }

    /// Returns the optional 1-based page index for a multi-page `.drawio`
    /// file.
    pub fn drawio_page(&self) -> Option<u32> {
        self.drawio_page
    }

    /// Sets the diagram source lines (for code-block-based diagrams).
    pub fn set_value(&mut self, value: impl Into<text::Lines>) {
        self.value = value.into();
    }
}

impl NodeKind for Diagram {
    fn typ(&self) -> NodeType {
        NodeType::LeafBlock
    }

    fn kind_name(&self) -> &'static str {
        "Diagram"
    }
}

impl PrettyPrint for Diagram {
    fn pretty_print(&self, w: &mut dyn Write, source: &str, level: usize) -> fmt::Result {
        writeln!(
            w,
            "{}DiagramType: {:?}",
            pp_indent(level),
            self.diagram_type()
        )?;
        if let Some(file_src) = &self.file_source {
            write!(w, "{}FileSource: ", pp_indent(level))?;
            writeln!(w, "[ ")?;
            for line in file_src.lines() {
                writeln!(w, "{}{}", pp_indent(level + 1), line)?;
            }
            writeln!(w, "{}]", pp_indent(level))
        } else {
            write!(w, "{}Value: ", pp_indent(level))?;
            writeln!(w, "[ ")?;
            for line in self.value.iter(source) {
                write!(w, "{}{}", pp_indent(level + 1), line)?;
            }
            writeln!(w)?;
            writeln!(w, "{}]", pp_indent(level))
        }
    }
}

impl From<Diagram> for KindData {
    fn from(e: Diagram) -> Self {
        KindData::Extension(Box::new(e))
    }
}

// ============================================================================
// Parser extension
// ============================================================================

/// Options for the diagram parser.
#[derive(Debug, Clone)]
pub struct DiagramParserOptions {
    pub mermaid_enabled: bool,
    pub plantuml_enabled: bool,
    pub typst_enabled: bool,
    pub d2_enabled: bool,
}

impl Default for DiagramParserOptions {
    fn default() -> Self {
        Self {
            mermaid_enabled: true,
            plantuml_enabled: true,
            typst_enabled: true,
            d2_enabled: true,
        }
    }
}

impl ParserOptions for DiagramParserOptions {}

#[derive(Debug)]
struct DiagramAstTransformer {
    options: DiagramParserOptions,
}

impl DiagramAstTransformer {
    fn with_options(options: DiagramParserOptions) -> Self {
        Self { options }
    }
}

/// Returns `true` if the node has the `.as-code` class in its attributes.
fn has_as_code_class(arena: &Arena, node_ref: NodeRef, source: &str) -> bool {
    arena[node_ref]
        .attributes()
        .get("class")
        .is_some_and(|v| v.str(source).split_whitespace().any(|c| c == "as-code"))
}

impl AstTransformer for DiagramAstTransformer {
    fn transform(
        &self,
        arena: &mut Arena,
        doc_ref: NodeRef,
        reader: &mut text::BasicReader,
        _ctx: &mut parser::Context,
    ) {
        // Phase 1: collect code blocks that should become diagrams (read-only walk).
        let mut target_codes: Vec<NodeRef> = Vec::new();
        walk(arena, doc_ref, &mut |arena: &Arena,
                                   node_ref: NodeRef,
                                   entering: bool|
         -> Result<WalkStatus> {
            if entering && matches_kind!(arena[node_ref], CodeBlock) {
                // The `.as-code` class suppresses diagram transformation,
                // keeping the block as ordinary syntax-highlighted code.
                if has_as_code_class(arena, node_ref, reader.source()) {
                    return Ok(WalkStatus::Continue);
                }
                let code_block = as_kind_data!(arena[node_ref], CodeBlock);
                if let Some(lang) = code_block.language_str(reader.source()) {
                    let dominated = match lang {
                        "mermaid" => self.options.mermaid_enabled,
                        "plantuml" => self.options.plantuml_enabled,
                        "typst" => self.options.typst_enabled,
                        "d2" => self.options.d2_enabled,
                        _ => false,
                    };
                    if dominated {
                        target_codes.push(node_ref);
                    }
                }
            }
            Ok(WalkStatus::Continue)
        })
        .ok();

        // Phase 2: replace collected code blocks with Diagram nodes.
        for code_ref in target_codes {
            let code_block = as_kind_data!(arena[code_ref], CodeBlock);
            let lines = code_block.value().clone();
            let diagram_type = match code_block.language_str(reader.source()) {
                Some("mermaid") => DiagramType::Mermaid,
                Some("plantuml") => DiagramType::PlantUml,
                Some("typst") => DiagramType::Typst,
                Some("d2") => DiagramType::D2,
                _ => continue,
            };
            let pos = arena[code_ref].pos();
            let diagram = arena.new_node(Diagram::new(diagram_type));
            if let Some(pos) = pos {
                arena[diagram].set_pos(pos);
            }
            as_extension_data_mut!(arena, diagram, Diagram).set_value(lines);
            let hbl = as_type_data!(arena, code_ref, Block).has_blank_previous_line();
            as_type_data_mut!(arena, diagram, Block).set_blank_previous_line(hbl);
            // Propagate user attributes (e.g. {width=50%}) from the original
            // code block to the replacement Diagram node so that renderers can
            // apply them to the output element.
            let attrs: Vec<_> = arena[code_ref]
                .attributes()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (key, value) in attrs {
                arena[diagram].attributes_mut().insert(key, value);
            }
            arena[code_ref]
                .parent()
                .unwrap()
                .replace_child(arena, code_ref, diagram);
        }
    }
}

impl From<DiagramAstTransformer> for AnyAstTransformer {
    fn from(t: DiagramAstTransformer) -> Self {
        AnyAstTransformer::Extension(Box::new(t))
    }
}

/// Returns a parser extension that detects diagram code blocks and transforms
/// them into [`Diagram`] AST nodes.
pub fn diagram_parser_extension(options: DiagramParserOptions) -> impl ParserExtension {
    ParserExtensionFn::new(|p: &mut Parser| {
        p.add_ast_transformer(DiagramAstTransformer::with_options, options, 100);
    })
}

// ============================================================================
// HTML renderer extension
// ============================================================================

/// CSS properties injected into the root `<svg>` element of every rendered
/// diagram so that the SVG scales responsively within its container instead
/// of overflowing at its intrinsic pixel size.
const SVG_RESPONSIVE_STYLE: &str = "width: 100%; height: auto; display: block;";

/// Post-process an SVG string to make it responsive.
///
/// Injects [`SVG_RESPONSIVE_STYLE`] into the `style` attribute of the first
/// `<svg` element.  If the element already has a `style` attribute the
/// responsive properties are prepended to the existing value; otherwise a
/// new `style` attribute is inserted.
fn make_svg_responsive(svg: &str) -> String {
    // Find the opening `<svg` tag (case-sensitive — all renderers emit lowercase).
    let Some(svg_tag_start) = svg.find("<svg") else {
        return svg.to_string();
    };
    let after_tag = svg_tag_start + 4; // position right after "<svg"
    let rest = &svg[after_tag..];

    // Find the closing `>` of this opening tag.
    let Some(tag_end_rel) = rest.find('>') else {
        return svg.to_string();
    };
    let tag_body = &rest[..tag_end_rel]; // everything between `<svg` and `>`

    // Check if there is already a `style="..."` attribute in the tag.
    if let Some(style_pos) = tag_body.find("style=\"") {
        // Append our responsive properties at the end of the existing style
        // value so they override any pixel dimensions the renderer set
        // (CSS cascade: last declaration wins within the same `style`).
        let style_val_start = style_pos + 7; // relative to tag_body
        let Some(style_val_end) = tag_body[style_val_start..].find('"') else {
            return svg.to_string();
        };
        let abs_insert = after_tag + style_val_start + style_val_end; // right before closing `"`
        let mut out = String::with_capacity(svg.len() + SVG_RESPONSIVE_STYLE.len() + 2);
        out.push_str(&svg[..abs_insert]);
        // Ensure a separator before our properties.
        let before = &svg[..abs_insert];
        if !before.ends_with(';') && !before.ends_with(' ') {
            out.push_str("; ");
        } else if !before.ends_with(' ') {
            out.push(' ');
        }
        out.push_str(SVG_RESPONSIVE_STYLE);
        out.push_str(&svg[abs_insert..]);
        out
    } else {
        // No existing style — insert a new style attribute right after `<svg`.
        let mut out = String::with_capacity(svg.len() + SVG_RESPONSIVE_STYLE.len() + 10);
        out.push_str(&svg[..after_tag]);
        out.push_str(" style=\"");
        out.push_str(SVG_RESPONSIVE_STYLE);
        out.push('"');
        out.push_str(&svg[after_tag..]);
        out
    }
}

const HAS_MERMAID_DIAGRAM: &str = "docbake-diagram-has-mermaid";

const DEFAULT_MERMAID_CDN_URL: &str =
    "https://cdn.jsdelivr.net/npm/mermaid@latest/dist/mermaid.esm.min.mjs";

/// How Mermaid diagrams should be rendered in HTML output.
#[derive(Debug, Clone)]
pub enum MermaidHtmlMode {
    /// Emit `<pre class="mermaid">` and a `<script>` tag importing the
    /// Mermaid JS module from a CDN. The browser renders the diagram.
    ClientSide {
        /// URL to the Mermaid ESM module.
        mermaid_url: String,
    },
}

impl Default for MermaidHtmlMode {
    fn default() -> Self {
        Self::ClientSide {
            mermaid_url: DEFAULT_MERMAID_CDN_URL.to_string(),
        }
    }
}

/// Theme content to prepend to diagram sources.
#[derive(Debug, Clone, Default)]
pub struct DiagramThemes {
    /// PlantUML theme content (inserted after `@start*` line).
    pub plantuml: Option<String>,
    /// Mermaid theme content (prepended before diagram source).
    pub mermaid: Option<String>,
    /// D2 theme content (prepended before diagram source).
    pub d2: Option<String>,
}

/// Options for the diagram HTML renderer.
#[derive(Debug, Clone, Default)]
pub struct DiagramHtmlRendererOptions {
    pub mermaid: MermaidHtmlMode,
    /// Diagram theme content from the theme directory.
    pub themes: DiagramThemes,
    /// Artifact cache for rendered diagrams.
    pub cache: crate::cache::Cache,
}

impl RendererOptions for DiagramHtmlRendererOptions {}

struct DiagramHtmlRenderer<W: TextWrite> {
    _phantom: std::marker::PhantomData<W>,
    options: DiagramHtmlRendererOptions,
    writer: html::Writer,
    has_mermaid_diagram: ContextKey<BoolValue>,
}

impl<W: TextWrite> DiagramHtmlRenderer<W> {
    fn new(
        html_opts: html::Options,
        options: DiagramHtmlRendererOptions,
        reg: Rc<RefCell<ContextKeyRegistry>>,
    ) -> Self {
        let has_mermaid_diagram = reg
            .borrow_mut()
            .get_or_create::<BoolValue>(HAS_MERMAID_DIAGRAM);
        Self {
            _phantom: std::marker::PhantomData,
            options,
            writer: html::Writer::with_options(html_opts),
            has_mermaid_diagram,
        }
    }
}

impl<W: TextWrite> RenderNode<W> for DiagramHtmlRenderer<W> {
    fn render_node<'a>(
        &self,
        w: &mut W,
        source: &'a str,
        arena: &'a Arena,
        node_ref: NodeRef,
        entering: bool,
        ctx: &mut renderer::Context,
    ) -> Result<WalkStatus> {
        let has_attrs = !arena[node_ref].attributes().is_empty();

        // Wrap diagram output in a <div> when user attributes are present
        // (e.g. {width=50%}) so that sizing/styling applies to the rendered
        // diagram, mirroring how attributes work on regular images.
        //
        // `width` and `height` are converted to CSS `style` properties
        // because HTML `width`/`height` attributes have no effect on
        // `<div>` elements — only CSS sizing works.  Other attributes
        // (class, id, data-*, etc.) are emitted as normal HTML attributes.
        //
        // The opening tag is emitted here; the closing </div> is emitted
        // after the diagram-type-specific rendering below.
        if has_attrs && entering {
            let attrs = arena[node_ref].attributes();
            let width = attrs.get("width").map(|v| v.str(source).to_string());
            let height = attrs.get("height").map(|v| v.str(source).to_string());

            // Build CSS style from width/height, merging with any
            // existing user-provided `style` attribute.
            let mut style = String::new();
            if let Some(ref w_val) = width {
                let _ = write!(style, "width: {w_val};");
            }
            if let Some(ref h_val) = height {
                if !style.is_empty() {
                    style.push(' ');
                }
                let _ = write!(style, "height: {h_val};");
            }
            if let Some(existing) = attrs.get("style").map(|v| v.str(source)) {
                if !style.is_empty() {
                    style.push(' ');
                }
                style.push_str(&existing);
            }

            // Build the opening <div> tag with style and remaining
            // attributes as a single string, then emit via write_html.
            let mut tag = String::from("<div");
            if !style.is_empty() {
                let _ = write!(tag, " style=\"{style}\"");
            }
            // Emit remaining attributes, skipping width/height/style
            // (already folded into the CSS style above).
            for (key, value) in attrs.iter() {
                if key == "width" || key == "height" || key == "style" {
                    continue;
                }
                let v = crate::render_html_common::html_escape(&value.str(source));
                let _ = write!(tag, " {key}=\"{v}\"");
            }
            tag.push('>');
            self.writer.write_html(w, &tag)?;
        }

        let diagram = as_extension_data!(arena, node_ref, Diagram);
        match diagram.diagram_type() {
            DiagramType::Mermaid => {
                ctx.insert(self.has_mermaid_diagram, true);
                if entering {
                    self.writer.write_safe_str(w, "<pre class=\"mermaid\">\n")?;
                    let src = collect_diagram_source(diagram, source);
                    let src = apply_mermaid_theme(&src, self.options.themes.mermaid.as_deref());
                    self.writer.raw_write(w, &src)?;
                } else {
                    self.writer.write_safe_str(w, "</pre>\n")?;
                }
            }
            DiagramType::PlantUml => {
                if entering {
                    let src = collect_diagram_source(diagram, source);
                    let src = apply_plantuml_theme(&src, self.options.themes.plantuml.as_deref());
                    match cached_render_bytes(
                        &self.options.cache,
                        src.as_bytes(),
                        "plantuml.svg",
                        || {
                            render_plantuml_svg(None, src.as_bytes())
                                .map_err(|e| anyhow::anyhow!("{e}"))
                        },
                    ) {
                        Ok(svg) => {
                            let svg = String::from_utf8_lossy(&svg);
                            let svg = if has_attrs {
                                make_svg_responsive(&svg)
                            } else {
                                svg.into_owned()
                            };
                            self.writer.write_html(w, &svg)?;
                        }
                        Err(e) => {
                            self.writer.write_html(
                                w,
                                &format!(
                                    "<pre class=\"plantuml-error\">Error rendering PlantUML diagram: {e}</pre>"
                                ),
                            )?;
                        }
                    }
                }
            }
            DiagramType::Typst => {
                if entering {
                    let src = collect_diagram_source(diagram, source);
                    match cached_render_svg(
                        &self.options.cache,
                        src.as_bytes(),
                        "typst.svg",
                        || render_typst_svg(None, &src).map_err(|e| anyhow::anyhow!("{e}")),
                    ) {
                        Ok(svg) => {
                            let svg = if has_attrs {
                                make_svg_responsive(&svg)
                            } else {
                                svg
                            };
                            self.writer.write_html(w, &svg)?;
                        }
                        Err(e) => {
                            self.writer.write_html(
                                w,
                                &format!(
                                    "<pre class=\"typst-error\">Error rendering Typst diagram: {e}</pre>"
                                ),
                            )?;
                        }
                    }
                }
            }
            DiagramType::DrawIo => {
                if entering {
                    if let Some(path) = diagram.drawio_path() {
                        let cache_key = drawio_cache_key(path, diagram.drawio_page());
                        match cached_render_bytes(
                            &self.options.cache,
                            &cache_key,
                            "drawio.svg",
                            || {
                                render_drawio(None, path, diagram.drawio_page(), DrawioFormat::Svg)
                                    .map_err(|e| anyhow::anyhow!("{e}"))
                            },
                        )
                        .and_then(|bytes| {
                            String::from_utf8(bytes).map_err(|e| anyhow::anyhow!("{e}"))
                        }) {
                            Ok(svg) => {
                                let svg = if has_attrs {
                                    make_svg_responsive(&svg)
                                } else {
                                    svg
                                };
                                self.writer.write_html(w, &svg)?;
                            }
                            Err(e) => {
                                self.writer.write_html(
                                    w,
                                    &format!(
                                        "<pre class=\"drawio-error\">Error rendering Draw.io diagram: {e}</pre>"
                                    ),
                                )?;
                            }
                        }
                    }
                }
            }
            DiagramType::D2 => {
                if entering {
                    let src = collect_diagram_source(diagram, source);
                    let src = apply_d2_theme(&src, self.options.themes.d2.as_deref());
                    match cached_render_svg(&self.options.cache, src.as_bytes(), "d2.svg", || {
                        render_d2_svg(None, &src).map_err(|e| anyhow::anyhow!("{e}"))
                    }) {
                        Ok(svg) => {
                            let svg = if has_attrs {
                                make_svg_responsive(&svg)
                            } else {
                                svg
                            };
                            self.writer.write_html(w, &svg)?;
                        }
                        Err(e) => {
                            self.writer.write_html(
                                w,
                                &format!(
                                    "<pre class=\"d2-error\">Error rendering D2 diagram: {e}</pre>"
                                ),
                            )?;
                        }
                    }
                }
            }
        }

        // Close the attribute wrapper <div> after the diagram content.
        if has_attrs && !entering {
            self.writer.write_safe_str(w, "</div>\n")?;
        }

        Ok(WalkStatus::Continue)
    }
}

impl<'cb, W> NodeRenderer<'cb, W> for DiagramHtmlRenderer<W>
where
    W: TextWrite + 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<Diagram>(), BoxRenderNode::new(self));
    }
}

// ============================================================================
// Post-render hook (Mermaid CDN script injection)
// ============================================================================

struct DiagramPostRenderHook<W: TextWrite> {
    _phantom: std::marker::PhantomData<W>,
    writer: html::Writer,
    mermaid: MermaidHtmlMode,
    has_mermaid_diagram: ContextKey<BoolValue>,
}

impl<W: TextWrite> DiagramPostRenderHook<W> {
    fn new(
        html_opts: html::Options,
        options: DiagramHtmlRendererOptions,
        reg: Rc<RefCell<ContextKeyRegistry>>,
    ) -> Self {
        let has_mermaid_diagram = reg
            .borrow_mut()
            .get_or_create::<BoolValue>(HAS_MERMAID_DIAGRAM);
        Self {
            _phantom: std::marker::PhantomData,
            writer: html::Writer::with_options(html_opts),
            mermaid: options.mermaid,
            has_mermaid_diagram,
        }
    }
}

impl<W: TextWrite> PostRender<W> for DiagramPostRenderHook<W> {
    fn post_render(
        &self,
        w: &mut W,
        _source: &str,
        _arena: &Arena,
        _node_ref: NodeRef,
        _render: &dyn Render<W>,
        ctx: &mut renderer::Context,
    ) -> Result<()> {
        if *ctx.get(self.has_mermaid_diagram).unwrap_or(&false) {
            let MermaidHtmlMode::ClientSide { ref mermaid_url } = self.mermaid;
            self.writer.write_html(
                w,
                &format!(
                    "<script type=\"module\">\nimport mermaid from '{mermaid_url}';\n</script>\n"
                ),
            )?;
        }
        Ok(())
    }
}

/// Returns a renderer extension that renders [`Diagram`] nodes in HTML.
pub fn diagram_html_renderer_extension<'cb, W>(
    options: DiagramHtmlRendererOptions,
) -> impl RendererExtension<'cb, W>
where
    W: TextWrite + 'cb,
{
    RendererExtensionFn::new(move |r: &mut Renderer<'cb, W>| {
        r.add_post_render_hook(DiagramPostRenderHook::new, options.clone(), 500);
        r.add_node_renderer(DiagramHtmlRenderer::new, options);
    })
}

// ============================================================================
// Rendering backends
// ============================================================================

/// Collect diagram source text from the [`Diagram`] AST node into a single
/// [`String`].  Prefers `file_source` (from `.puml`/`.mmd` images) over
/// `value` (from fenced code blocks).
pub(crate) fn collect_diagram_source(diagram: &Diagram, source: &str) -> String {
    if let Some(fs) = &diagram.file_source {
        return fs.clone();
    }
    let mut buf = String::new();
    for line in diagram.value().iter(source) {
        buf.push_str(&line);
    }
    buf
}

// ============================================================================
// Cache helpers
// ============================================================================

/// Render a diagram to SVG (as `String`), using the cache when available.
///
/// `content` is the raw bytes used to derive the cache key (typically the
/// diagram source text).  `renderer` is the cache tag (e.g. `"mermaid.svg"`).
/// `render_fn` is the fallback that performs the actual rendering on a cache
/// miss.
pub(crate) fn cached_render_svg(
    cache: &crate::cache::Cache,
    content: &[u8],
    renderer: &str,
    render_fn: impl FnOnce() -> anyhow::Result<String>,
) -> anyhow::Result<String> {
    if let Some(data) = cache.lookup(content, renderer) {
        return Ok(String::from_utf8(data)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()));
    }
    let svg = render_fn()?;
    // Best-effort cache store — do not fail the build on cache write errors.
    let _ = cache.store(content, renderer, svg.as_bytes());
    Ok(svg)
}

/// Render a diagram to raw bytes, using the cache when available.
///
/// Same as [`cached_render_svg`] but for renderers that produce opaque byte
/// output (e.g. PlantUML SVG as `Vec<u8>`, Draw.io PDF).
pub(crate) fn cached_render_bytes(
    cache: &crate::cache::Cache,
    content: &[u8],
    renderer: &str,
    render_fn: impl FnOnce() -> anyhow::Result<Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    if let Some(data) = cache.lookup(content, renderer) {
        return Ok(data);
    }
    let data = render_fn()?;
    let _ = cache.store(content, renderer, &data);
    Ok(data)
}

/// Build a cache key for a `.drawio` file.
///
/// The key is the file contents concatenated with the page number (if any),
/// so the cache correctly invalidates when the file changes or a different
/// page is requested.
pub(crate) fn drawio_cache_key(path: &Path, page: Option<u32>) -> Vec<u8> {
    let mut key = fs::read(path).unwrap_or_default();
    if let Some(p) = page {
        key.extend_from_slice(b"\0page:");
        key.extend_from_slice(p.to_string().as_bytes());
    }
    key
}

/// Render a Mermaid diagram to SVG using the `mermaid-rs-renderer` crate.
fn render_mermaid_svg(src: &str) -> anyhow::Result<String> {
    mermaid_rs_renderer::render(src)
}

/// Render a Mermaid diagram to SVG by shelling out to `mmdc` (mermaid-cli).
///
/// If `command` is `None`, the binary is located via `which::which("mmdc")`.
fn render_mermaid_svg_cli(
    command: Option<&str>,
    src: &str,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("mmdc")?,
    };

    let mut child = Command::new(path)
        .args(["-i", "-", "-o", "-", "-e", "svg", "-q"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().ok_or("failed to open mmdc stdin")?;
        stdin.write_all(src.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Render a Mermaid diagram to SVG, trying `mmdc` first and falling back
/// to the built-in `mermaid-rs-renderer`.
pub(crate) fn render_mermaid_svg_auto(command: Option<&str>, src: &str) -> anyhow::Result<String> {
    let mmdc_available = match command {
        Some(_) => true,
        None => which::which("mmdc").is_ok(),
    };
    if mmdc_available {
        if let Ok(svg) = render_mermaid_svg_cli(command, src) {
            return Ok(svg);
        }
    }
    render_mermaid_svg(src)
}

/// Render a PlantUML diagram to SVG by shelling out to the `plantuml` binary.
///
/// If `command` is `None`, the binary is located via `which::which("plantuml")`.
pub(crate) fn render_plantuml_svg(
    command: Option<&str>,
    src: &[u8],
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("plantuml")?,
    };

    let mut child = Command::new(path)
        .args(["-tsvg", "-p", "-Djava.awt.headless=true"])
        .env("JAVA_OPTS", "-Djava.awt.headless=true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or("failed to open plantuml stdin")?;
        stdin.write_all(src)?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Typst document preamble for SVG rendering.
///
/// Sets page dimensions to `auto` with zero margins so the resulting SVG
/// is tightly cropped around the diagram content.
const TYPST_SVG_PREAMBLE: &str = "#set page(width: auto, height: auto, margin: 0pt)\n";

/// Render a Typst diagram to SVG by shelling out to `typst compile`.
///
/// The user's source is wrapped in a minimal document preamble
/// ([`TYPST_SVG_PREAMBLE`]) so that the produced SVG is tightly cropped.
///
/// If `command` is `None`, the binary is located via `which::which("typst")`.
pub(crate) fn render_typst_svg(
    command: Option<&str>,
    src: &str,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("typst")?,
    };

    let input = format!("{TYPST_SVG_PREAMBLE}{src}");

    let mut child = Command::new(path)
        .args(["compile", "--format", "svg", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().ok_or("failed to open typst stdin")?;
        stdin.write_all(input.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Output format for the Draw.io CLI renderer.
#[allow(dead_code)]
pub(crate) enum DrawioFormat {
    Svg,
    Pdf,
    Png,
}

/// Render a Draw.io diagram by shelling out to the `drawio` binary.
///
/// If `command` is `None`, the binary is located via `which::which("drawio")`.
/// `page` is an optional 1-based page index (passed as `-p <page>`).
///
/// The `drawio` CLI does not support stdout output, so a temporary file is
/// used to capture the result. Returns the raw output bytes regardless of
/// format; callers that need a `String` (e.g. for SVG) can convert via
/// `String::from_utf8`.
pub(crate) fn render_drawio(
    command: Option<&str>,
    input: &Path,
    page: Option<u32>,
    format: DrawioFormat,
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("drawio")?,
    };

    let (suffix, fmt_flag) = match format {
        DrawioFormat::Svg => (".svg", "svg"),
        DrawioFormat::Pdf => (".pdf", "pdf"),
        DrawioFormat::Png => (".png", "png"),
    };

    let tmp = tempfile::Builder::new().suffix(suffix).tempfile()?;
    let tmp_path = tmp.path().to_path_buf();

    let mut cmd = Command::new(path);
    cmd.arg("-x");
    if matches!(format, DrawioFormat::Svg) {
        cmd.args(["--embed-svg-fonts", "false"]);
    }
    cmd.args(["-f", fmt_flag, "--crop", "-o"]);
    cmd.arg(&tmp_path);
    if let Some(p) = page {
        cmd.args(["-p", &p.to_string()]);
    }
    cmd.arg(input);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd
        .output()
        .map_err(|e| format!("failed to run drawio: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }

    let data =
        fs::read(&tmp_path).map_err(|e| format!("failed to read drawio {fmt_flag} output: {e}"))?;
    Ok(data)
}

/// Render a D2 diagram to SVG by shelling out to the `d2` binary.
///
/// If `command` is `None`, the binary is located via `which::which("d2")`.
/// Uses `--no-xml-tag` to strip the `<?xml ...?>` header for cleaner
/// embedding in HTML and Typst.
pub(crate) fn render_d2_svg(
    command: Option<&str>,
    src: &str,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("d2")?,
    };

    let mut child = Command::new(path)
        .args(["--no-xml-tag", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().ok_or("failed to open d2 stdin")?;
        stdin.write_all(src.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

// ============================================================================
// PNG rendering functions (available for future use)
// ============================================================================

/// Render a PlantUML diagram to PNG by shelling out to the `plantuml` binary.
#[allow(dead_code)]
pub(crate) fn render_plantuml_png(
    command: Option<&str>,
    src: &[u8],
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("plantuml")?,
    };

    let mut child = Command::new(path)
        .args(["-tpng", "-p", "-Djava.awt.headless=true"])
        .env("JAVA_OPTS", "-Djava.awt.headless=true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or("failed to open plantuml stdin")?;
        stdin.write_all(src)?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Render a Mermaid diagram to PNG by shelling out to `mmdc`.
///
/// Unlike SVG rendering, PNG does not have a built-in fallback.
/// If `mmdc` is not available this function returns an error.
#[allow(dead_code)]
pub(crate) fn render_mermaid_png(
    command: Option<&str>,
    src: &str,
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("mmdc")?,
    };

    let tmp = tempfile::Builder::new().suffix(".png").tempfile()?;
    let tmp_path = tmp.path().to_path_buf();

    let mut child = Command::new(path)
        .args(["-i", "-", "-o"])
        .arg(&tmp_path)
        .args(["-e", "png", "-q"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().ok_or("failed to open mmdc stdin")?;
        stdin.write_all(src.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        let data =
            fs::read(&tmp_path).map_err(|e| format!("failed to read mmdc PNG output: {e}"))?;
        Ok(data)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Render a D2 diagram to PNG by shelling out to the `d2` binary.
#[allow(dead_code)]
pub(crate) fn render_d2_png(
    command: Option<&str>,
    src: &str,
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("d2")?,
    };

    let tmp = tempfile::Builder::new().suffix(".png").tempfile()?;
    let tmp_path = tmp.path().to_path_buf();

    let mut child = Command::new(path)
        .args(["-", &tmp_path.to_string_lossy()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().ok_or("failed to open d2 stdin")?;
        stdin.write_all(src.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        let data = fs::read(&tmp_path).map_err(|e| format!("failed to read d2 PNG output: {e}"))?;
        Ok(data)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Render a Typst diagram to PNG by shelling out to `typst compile`.
#[allow(dead_code)]
pub(crate) fn render_typst_png(
    command: Option<&str>,
    src: &str,
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("typst")?,
    };

    let input = format!("{TYPST_SVG_PREAMBLE}{src}");

    let tmp = tempfile::Builder::new().suffix(".png").tempfile()?;
    let tmp_path = tmp.path().to_path_buf();

    let output = Command::new(path)
        .args(["compile", "--format", "png", "-"])
        .arg(&tmp_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .ok_or(std::io::Error::other("failed to open typst stdin"))?
                .write_all(input.as_bytes())?;
            child.wait_with_output()
        })?;

    if output.status.success() {
        let data =
            fs::read(&tmp_path).map_err(|e| format!("failed to read typst PNG output: {e}"))?;
        Ok(data)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

// ============================================================================
// Image-to-diagram AST transformation
// ============================================================================

/// Diagram file extensions and their corresponding types.
const DIAGRAM_EXTENSIONS: &[(&str, DiagramType)] = &[
    ("puml", DiagramType::PlantUml),
    ("plantuml", DiagramType::PlantUml),
    ("mmd", DiagramType::Mermaid),
    ("mermaid", DiagramType::Mermaid),
    ("typ", DiagramType::Typst),
    ("drawio", DiagramType::DrawIo),
    ("d2", DiagramType::D2),
];

/// Detect whether an image destination path refers to a diagram file.
///
/// The `dest` string may contain a fragment (e.g. `diagram.drawio#1`);
/// the fragment is stripped before checking the extension.
fn diagram_type_for_path(dest: &str) -> Option<DiagramType> {
    let path_part = dest.split('#').next().unwrap_or(dest);
    let ext = path_part.rsplit('.').next()?;
    DIAGRAM_EXTENSIONS
        .iter()
        .find(|(e, _)| e.eq_ignore_ascii_case(ext))
        .map(|(_, dt)| *dt)
}

/// Extract the page index from a `.drawio` image destination fragment.
///
/// Given `diagram.drawio#2`, returns `Some(2)`.  Returns `None` if
/// there is no fragment or the fragment is not a valid page number.
fn drawio_page_from_dest(dest: &str) -> Option<u32> {
    let (_, fragment) = dest.split_once('#')?;
    fragment.parse::<u32>().ok()
}

/// If `paragraph_ref` contains exactly one child and it is an `Image` node,
/// return that image's `NodeRef`.
pub(crate) fn paragraph_has_sole_image(arena: &Arena, paragraph_ref: NodeRef) -> Option<NodeRef> {
    let mut children = arena[paragraph_ref].children(arena);
    let first = children.next()?;
    if children.next().is_some() {
        return None; // more than one child
    }
    if matches!(arena[first].kind_data(), KindData::Image(_)) {
        Some(first)
    } else {
        None
    }
}

/// Walk the AST and replace `Paragraph > Image(.puml/.mmd/.drawio)` with a
/// `Diagram` node containing the file content (or pre-rendered SVG for
/// Draw.io diagrams).
///
/// An image is eligible when:
/// - Its destination ends with a known diagram extension (`.puml`, `.mmd`,
///   `.drawio`, etc.)
/// - It is the only meaningful child of its parent `Paragraph`
///
/// The parent `Paragraph` is replaced by a `Diagram` `LeafBlock`.
pub(crate) fn transform_diagram_images(
    arena: &mut Arena,
    doc_ref: NodeRef,
    source: &str,
    source_dir: &Path,
    base_dir: &Path,
) -> anyhow::Result<()> {
    // Phase 1: collect eligible (paragraph, image, type, resolved_path, drawio_page) tuples.
    // The image NodeRef is stored so we can copy its attributes (e.g. {width=50%})
    // onto the replacement Diagram node in phase 2.
    let mut candidates: Vec<(NodeRef, NodeRef, DiagramType, PathBuf, Option<u32>)> = Vec::new();

    for child in arena[doc_ref].children(arena).collect::<Vec<_>>() {
        if !matches!(arena[child].kind_data(), KindData::Paragraph(_)) {
            continue;
        }
        let Some(image_ref) = paragraph_has_sole_image(arena, child) else {
            continue;
        };
        let image = as_kind_data!(arena[image_ref], Image);
        let dest = image.destination_str(source);
        let Some(diagram_type) = diagram_type_for_path(dest) else {
            continue;
        };

        // Strip fragment before path resolution (e.g. `diagram.drawio#1` → `diagram.drawio`).
        let path_part = dest.split('#').next().unwrap_or(dest);
        let drawio_page = if diagram_type == DiagramType::DrawIo {
            drawio_page_from_dest(dest)
        } else {
            None
        };

        // Resolve the file path relative to the source directory.
        let file_path = if path_part.starts_with('/') {
            base_dir.join(path_part.trim_start_matches('/'))
        } else {
            source_dir.join(path_part)
        };
        let file_path = file_path.canonicalize().with_context(|| {
            format!(
                "failed to resolve diagram image `{dest}` relative to {}",
                source_dir.display()
            )
        })?;
        if !file_path.starts_with(base_dir) {
            anyhow::bail!(
                "diagram image `{dest}` resolves to {} which is outside the base directory {}",
                file_path.display(),
                base_dir.display()
            );
        }
        candidates.push((child, image_ref, diagram_type, file_path, drawio_page));
    }

    // Phase 2: replace each paragraph with a Diagram node.
    for (paragraph_ref, image_ref, diagram_type, file_path, drawio_page) in candidates {
        let diagram_node = if diagram_type == DiagramType::DrawIo {
            // Draw.io diagrams store the resolved path for deferred
            // rendering — the actual SVG/PDF conversion happens at render
            // time so that each renderer can choose the appropriate format.
            arena.new_node(Diagram::from_drawio(file_path, drawio_page))
        } else {
            let content = fs::read_to_string(&file_path)
                .with_context(|| format!("failed to read diagram file {}", file_path.display()))?;
            arena.new_node(Diagram::from_file(diagram_type, content))
        };
        if let Some(pos) = arena[paragraph_ref].pos() {
            arena[diagram_node].set_pos(pos);
        }
        let hbl = as_type_data!(arena, paragraph_ref, Block).has_blank_previous_line();
        as_type_data_mut!(arena, diagram_node, Block).set_blank_previous_line(hbl);
        // Propagate user attributes (e.g. {width=50%}) from the original
        // Image node to the replacement Diagram node so that renderers can
        // apply them to the output element.
        let attrs: Vec<_> = arena[image_ref]
            .attributes()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (key, value) in attrs {
            arena[diagram_node].attributes_mut().insert(key, value);
        }
        doc_ref.replace_child(arena, paragraph_ref, diagram_node);
    }

    Ok(())
}

// ============================================================================
// Theme application helpers
// ============================================================================

/// Apply a PlantUML theme to a diagram source.
///
/// The theme content is inserted right after the first `@start*` line
/// (e.g. `@startuml`, `@startmindmap`).  Returns the source unchanged
/// when `theme` is `None`.
pub(crate) fn apply_plantuml_theme(src: &str, theme: Option<&str>) -> String {
    let Some(content) = theme else {
        return src.to_string();
    };
    if content.is_empty() {
        return src.to_string();
    }
    // Insert after the first line (@startuml, @startsalt, etc.)
    if let Some(pos) = src.find('\n') {
        let mut out = String::with_capacity(src.len() + content.len() + 1);
        out.push_str(&src[..pos + 1]);
        out.push_str(content);
        if !content.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&src[pos + 1..]);
        out
    } else {
        // Single-line source (unusual) — append theme.
        format!("{src}\n{content}")
    }
}

/// Apply a Mermaid theme to a diagram source.
///
/// The theme content (typically a `%%{init: ...}%%` directive) is prepended
/// before the diagram source.  If the source already contains `%%{init`,
/// the theme is skipped to avoid conflicts.  Returns the source unchanged
/// when `theme` is `None`.
pub(crate) fn apply_mermaid_theme(src: &str, theme: Option<&str>) -> String {
    let Some(content) = theme else {
        return src.to_string();
    };
    if content.is_empty() {
        return src.to_string();
    }
    // Don't clobber an existing init directive.
    if src.contains("%%{init") {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len() + content.len() + 1);
    out.push_str(content);
    if !content.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(src);
    out
}

/// Apply a D2 theme to a diagram source.
///
/// The theme content is prepended before the diagram source.  Returns
/// the source unchanged when `theme` is `None`.
pub(crate) fn apply_d2_theme(src: &str, theme: Option<&str>) -> String {
    let Some(content) = theme else {
        return src.to_string();
    };
    if content.is_empty() {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len() + content.len() + 1);
    out.push_str(content);
    if !content.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(src);
    out
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rushdown::matches_extension_kind;
    use rushdown::parser;
    use rushdown::text::BasicReader;

    fn test_parser() -> parser::Parser {
        parser::Parser::with_extensions(
            parser::Options {
                attributes: true,
                ..Default::default()
            },
            parser::gfm(parser::GfmOptions::default())
                .and(diagram_parser_extension(DiagramParserOptions::default())),
        )
    }

    fn parse(source: &str) -> (Arena, NodeRef) {
        let parser = test_parser();
        let mut reader = BasicReader::new(source);
        parser.parse(&mut reader)
    }

    #[test]
    fn mermaid_code_block_becomes_diagram_node() {
        let source = "```mermaid\ngraph LR\n  A --> B\n```\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1, "expected one Diagram node");
        assert!(
            matches_extension_kind!(&arena, children[0], Diagram),
            "expected Diagram extension node"
        );

        let diagram = as_extension_data!(&arena, children[0], Diagram);
        assert_eq!(diagram.diagram_type(), DiagramType::Mermaid);
    }

    #[test]
    fn plantuml_code_block_becomes_diagram_node() {
        let source = "```plantuml\n@startuml\nAlice -> Bob\n@enduml\n```\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1, "expected one Diagram node");
        assert!(matches_extension_kind!(&arena, children[0], Diagram));

        let diagram = as_extension_data!(&arena, children[0], Diagram);
        assert_eq!(diagram.diagram_type(), DiagramType::PlantUml);
    }

    #[test]
    fn non_diagram_code_block_unchanged() {
        let source = "```rust\nfn main() {}\n```\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1);
        assert!(
            matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
            "rust code block should remain a CodeBlock"
        );
    }

    #[test]
    fn disabled_mermaid_leaves_code_block() {
        let opts = DiagramParserOptions {
            mermaid_enabled: false,
            plantuml_enabled: true,
            typst_enabled: true,
            d2_enabled: true,
        };
        let parser = parser::Parser::with_extensions(
            parser::Options::default(),
            parser::gfm(parser::GfmOptions::default()).and(diagram_parser_extension(opts)),
        );
        let source = "```mermaid\ngraph LR\n```\n";
        let mut reader = BasicReader::new(source);
        let (arena, doc_ref) = parser.parse(&mut reader);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert!(
            matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
            "mermaid code block should remain when mermaid is disabled"
        );
    }

    #[test]
    fn mermaid_builtin_rendering() {
        let svg = render_mermaid_svg("flowchart LR; A-->B-->C").unwrap();
        assert!(
            svg.contains("<svg"),
            "expected SVG output from mermaid-rs-renderer"
        );
    }

    #[test]
    fn typst_code_block_becomes_diagram_node() {
        let source = "```typst\n#rect(width: 50pt, height: 30pt)\n```\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1, "expected one Diagram node");
        assert!(
            matches_extension_kind!(&arena, children[0], Diagram),
            "expected Diagram extension node"
        );

        let diagram = as_extension_data!(&arena, children[0], Diagram);
        assert_eq!(diagram.diagram_type(), DiagramType::Typst);
    }

    #[test]
    fn disabled_typst_leaves_code_block() {
        let opts = DiagramParserOptions {
            mermaid_enabled: true,
            plantuml_enabled: true,
            typst_enabled: false,
            d2_enabled: true,
        };
        let parser = parser::Parser::with_extensions(
            parser::Options::default(),
            parser::gfm(parser::GfmOptions::default()).and(diagram_parser_extension(opts)),
        );
        let source = "```typst\n#rect(width: 50pt, height: 30pt)\n```\n";
        let mut reader = BasicReader::new(source);
        let (arena, doc_ref) = parser.parse(&mut reader);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert!(
            matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
            "typst code block should remain when typst is disabled"
        );
    }

    #[test]
    fn typst_svg_rendering() {
        // Skip if typst is not on PATH.
        if which::which("typst").is_err() {
            eprintln!("skipping typst_svg_rendering: typst not found on PATH");
            return;
        }
        let svg = render_typst_svg(None, "#rect(width: 50pt, height: 30pt, fill: blue)").unwrap();
        assert!(
            svg.contains("<svg"),
            "expected SVG output from typst compile"
        );
    }

    #[test]
    fn diagram_type_for_path_detects_drawio() {
        assert_eq!(
            diagram_type_for_path("arch.drawio"),
            Some(DiagramType::DrawIo)
        );
        assert_eq!(
            diagram_type_for_path("arch.DRAWIO"),
            Some(DiagramType::DrawIo)
        );
    }

    #[test]
    fn diagram_type_for_path_strips_fragment() {
        assert_eq!(
            diagram_type_for_path("arch.drawio#1"),
            Some(DiagramType::DrawIo)
        );
        assert_eq!(
            diagram_type_for_path("arch.drawio#42"),
            Some(DiagramType::DrawIo)
        );
        // Non-drawio with fragment should still work.
        assert_eq!(
            diagram_type_for_path("seq.puml#something"),
            Some(DiagramType::PlantUml)
        );
    }

    #[test]
    fn drawio_page_extraction() {
        assert_eq!(drawio_page_from_dest("arch.drawio#1"), Some(1));
        assert_eq!(drawio_page_from_dest("arch.drawio#42"), Some(42));
        assert_eq!(drawio_page_from_dest("arch.drawio"), None);
        assert_eq!(drawio_page_from_dest("arch.drawio#"), None);
        assert_eq!(drawio_page_from_dest("arch.drawio#abc"), None);
    }

    #[test]
    fn drawio_svg_rendering() {
        // Skip if drawio is not on PATH.
        if which::which("drawio").is_err() {
            eprintln!("skipping drawio_svg_rendering: drawio not found on PATH");
            return;
        }
        // Use the project's test asset.
        let asset = std::path::Path::new("doc/assets/Docbake High Level.drawio");
        if !asset.exists() {
            eprintln!("skipping drawio_svg_rendering: test asset not found");
            return;
        }
        let bytes = render_drawio(None, asset, Some(1), DrawioFormat::Svg).unwrap();
        let svg = String::from_utf8(bytes).expect("SVG output should be valid UTF-8");
        assert!(svg.contains("<svg"), "expected SVG output from drawio CLI");
    }

    #[test]
    fn d2_code_block_becomes_diagram_node() {
        let source = "```d2\nx -> y: hello\n```\n";
        let (arena, doc_ref) = parse(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1, "expected one Diagram node");
        assert!(
            matches_extension_kind!(&arena, children[0], Diagram),
            "expected Diagram extension node"
        );

        let diagram = as_extension_data!(&arena, children[0], Diagram);
        assert_eq!(diagram.diagram_type(), DiagramType::D2);
    }

    #[test]
    fn disabled_d2_leaves_code_block() {
        let opts = DiagramParserOptions {
            mermaid_enabled: true,
            plantuml_enabled: true,
            typst_enabled: true,
            d2_enabled: false,
        };
        let parser = parser::Parser::with_extensions(
            parser::Options::default(),
            parser::gfm(parser::GfmOptions::default()).and(diagram_parser_extension(opts)),
        );
        let source = "```d2\nx -> y: hello\n```\n";
        let mut reader = BasicReader::new(source);
        let (arena, doc_ref) = parser.parse(&mut reader);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert!(
            matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
            "d2 code block should remain when d2 is disabled"
        );
    }

    #[test]
    fn d2_svg_rendering() {
        // Skip if d2 is not on PATH.
        if which::which("d2").is_err() {
            eprintln!("skipping d2_svg_rendering: d2 not found on PATH");
            return;
        }
        let svg = render_d2_svg(None, "x -> y: hello world").unwrap();
        assert!(svg.contains("<svg"), "expected SVG output from d2 CLI");
    }

    #[test]
    fn diagram_type_for_path_detects_d2() {
        assert_eq!(diagram_type_for_path("network.d2"), Some(DiagramType::D2));
        assert_eq!(diagram_type_for_path("network.D2"), Some(DiagramType::D2));
    }

    /// Parser that includes both the code-block attribute extension (priority
    /// 50) and the diagram extension (priority 100), matching production order.
    fn parse_with_attrs(source: &str) -> (Arena, NodeRef) {
        let parser = parser::Parser::with_extensions(
            parser::Options {
                attributes: true,
                ..Default::default()
            },
            parser::gfm(parser::GfmOptions::default())
                .and(crate::code_block_attribute::code_block_attribute_parser_extension())
                .and(diagram_parser_extension(DiagramParserOptions::default())),
        );
        let mut reader = BasicReader::new(source);
        parser.parse(&mut reader)
    }

    #[test]
    fn as_code_suppresses_mermaid_diagram() {
        let source = "```mermaid {.as-code}\ngraph LR\n  A --> B\n```\n";
        let (arena, doc_ref) = parse_with_attrs(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1);
        assert!(
            matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
            "mermaid block with .as-code should remain a CodeBlock, got {:?}",
            arena[children[0]].kind_data().kind_name()
        );
    }

    #[test]
    fn as_code_suppresses_d2_diagram() {
        let source = "```d2 {.as-code}\nx -> y: hello\n```\n";
        let (arena, doc_ref) = parse_with_attrs(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1);
        assert!(
            matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
            "d2 block with .as-code should remain a CodeBlock"
        );
    }

    #[test]
    fn as_code_with_multiple_classes() {
        let source = "```mermaid {.as-code .example}\ngraph LR\n```\n";
        let (arena, doc_ref) = parse_with_attrs(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert!(
            matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
            "mermaid block with .as-code among multiple classes should remain a CodeBlock"
        );
    }

    #[test]
    fn mermaid_without_as_code_still_becomes_diagram() {
        let source = "```mermaid {#my-diagram}\ngraph LR\n  A --> B\n```\n";
        let (arena, doc_ref) = parse_with_attrs(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1);
        assert!(
            matches_extension_kind!(&arena, children[0], Diagram),
            "mermaid block without .as-code should become a Diagram"
        );
    }

    #[test]
    fn as_code_suppresses_typst_diagram() {
        let source = "```typst {.as-code}\n#rect(width: 50pt)\n```\n";
        let (arena, doc_ref) = parse_with_attrs(source);

        let children: Vec<_> = arena[doc_ref].children(&arena).collect();
        assert_eq!(children.len(), 1);
        assert!(
            matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
            "typst block with .as-code should remain a CodeBlock"
        );
    }
}
