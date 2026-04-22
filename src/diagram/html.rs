use super::*;

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
    writer: rd_html::Writer,
    has_mermaid_diagram: ContextKey<BoolValue>,
}

impl<W: TextWrite> DiagramHtmlRenderer<W> {
    fn new(
        html_opts: rd_html::Options,
        options: DiagramHtmlRendererOptions,
        reg: Rc<RefCell<ContextKeyRegistry>>,
    ) -> Self {
        let has_mermaid_diagram = reg
            .borrow_mut()
            .get_or_create::<BoolValue>(HAS_MERMAID_DIAGRAM);
        Self {
            _phantom: std::marker::PhantomData,
            options,
            writer: rd_html::Writer::with_options(html_opts),
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
    writer: rd_html::Writer,
    mermaid: MermaidHtmlMode,
    has_mermaid_diagram: ContextKey<BoolValue>,
}

impl<W: TextWrite> DiagramPostRenderHook<W> {
    fn new(
        html_opts: rd_html::Options,
        options: DiagramHtmlRendererOptions,
        reg: Rc<RefCell<ContextKeyRegistry>>,
    ) -> Self {
        let has_mermaid_diagram = reg
            .borrow_mut()
            .get_or_create::<BoolValue>(HAS_MERMAID_DIAGRAM);
        Self {
            _phantom: std::marker::PhantomData,
            writer: rd_html::Writer::with_options(html_opts),
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
