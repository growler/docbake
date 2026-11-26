use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use docx_rs::*;
use resvg::{tiny_skia, usvg};
use rushdown::ast::{Arena, NodeRef};
use rushdown::{as_extension_data, as_kind_data};

use crate::cache::Cache;

use super::extent::{extent, length};
use super::params::{DocxParams, Page, Twips};
use super::{DocxRenderer, RenderContext};

/// EMU per twip (1 twip = 914400 / 1440 EMU).
const EMU_PER_TWIP: u64 = 635;

/// EMU per half-point (1 half-point = 914400 / 144 EMU).
const EMU_PER_HALF_POINT: u64 = 6350;

/// The font size in half-points of `1em` when the `Normal` style has no size.
const DEFAULT_EM_HALF_POINTS: u32 = 20;

/// The pixels per inch of an image file.
const IMAGE_PPI: u32 = 96;

/// The `width` and the `height` attribute of an image or a diagram, in EMU.
/// `None` is an attribute that is not set or is `auto`.
type SizeAttributes = (Option<u64>, Option<u64>);

/// A diagram PNG render request.
///
/// Holds the renderer input, which also gives the artifact cache key and
/// tag.  The data is owned, so a job can go to another thread.
pub(super) enum DiagramJob {
    /// Themed Mermaid source.
    Mermaid(String),
    /// Themed PlantUML source.
    PlantUml(String),
    /// Themed D2 source.
    D2(String),
    /// Typst source.
    Typst(String),
    /// Draw.io file, optional 1-based page, and the cache key made from them.
    DrawIo {
        path: PathBuf,
        page: Option<u32>,
        key: Vec<u8>,
    },
}

impl DiagramJob {
    /// Artifact cache key.
    pub(super) fn cache_key(&self) -> &[u8] {
        match self {
            Self::Mermaid(src) | Self::PlantUml(src) | Self::D2(src) | Self::Typst(src) => {
                src.as_bytes()
            }
            Self::DrawIo { key, .. } => key,
        }
    }

    /// Artifact cache tag.
    pub(super) fn cache_tag(&self) -> &'static str {
        match self {
            Self::Mermaid(_) => "mermaid.png",
            Self::PlantUml(_) => "plantuml.png",
            Self::D2(_) => "d2.png",
            Self::Typst(_) => "typst.png",
            Self::DrawIo { .. } => "drawio.png",
        }
    }

    /// The pixels per inch of the PNG that the renderer makes: 144 for
    /// `typst compile`, 192 for `d2` and 96 for the other renderers.
    pub(super) fn ppi(&self) -> u32 {
        match self {
            Self::Typst(_) => 144,
            Self::D2(_) => 192,
            Self::Mermaid(_) | Self::PlantUml(_) | Self::DrawIo { .. } => 96,
        }
    }

    /// Render the diagram to PNG through the artifact cache.
    pub(super) fn render_cached(&self, cache: &Cache) -> Result<Vec<u8>> {
        crate::diagram::cached_render_bytes(cache, self.cache_key(), self.cache_tag(), || {
            self.render()
        })
    }

    fn render(&self) -> Result<Vec<u8>> {
        match self {
            Self::Mermaid(src) => crate::diagram::render_mermaid_png(None, src)
                .map_err(|e| anyhow!("mermaid PNG rendering: {e}")),
            Self::PlantUml(src) => crate::diagram::render_plantuml_png(None, src.as_bytes())
                .map_err(|e| anyhow!("plantuml PNG rendering: {e}")),
            Self::D2(src) => crate::diagram::render_d2_png(None, src)
                .map_err(|e| anyhow!("D2 PNG rendering: {e}")),
            Self::Typst(src) => crate::diagram::render_typst_png(None, src)
                .map_err(|e| anyhow!("Typst PNG rendering: {e}")),
            Self::DrawIo { path, page, .. } => {
                crate::diagram::render_drawio(None, path, *page, crate::diagram::DrawioFormat::Png)
                    .map_err(|e| anyhow!("Draw.io PNG rendering: {e}"))
            }
        }
    }
}

impl<'a> DocxRenderer<'a> {
    /// Build the PNG render request for a diagram node.
    ///
    /// Returns `None` for a Draw.io diagram without a file path.
    pub(super) fn diagram_job(
        &self,
        arena: &Arena,
        source: &str,
        node_ref: NodeRef,
    ) -> Option<DiagramJob> {
        let diagram = as_extension_data!(arena, node_ref, crate::diagram::Diagram);
        let src = crate::diagram::collect_diagram_source(diagram, source);

        let job = match diagram.diagram_type() {
            crate::diagram::DiagramType::Mermaid => DiagramJob::Mermaid(
                crate::diagram::apply_mermaid_theme(&src, self.diagram_themes.mermaid.as_deref()),
            ),
            crate::diagram::DiagramType::PlantUml => DiagramJob::PlantUml(
                crate::diagram::apply_plantuml_theme(&src, self.diagram_themes.plantuml.as_deref()),
            ),
            crate::diagram::DiagramType::D2 => DiagramJob::D2(crate::diagram::apply_d2_theme(
                &src,
                self.diagram_themes.d2.as_deref(),
            )),
            crate::diagram::DiagramType::Typst => DiagramJob::Typst(src),
            crate::diagram::DiagramType::DrawIo => {
                let path = diagram.drawio_path()?;
                let page = diagram.drawio_page();
                DiagramJob::DrawIo {
                    key: crate::diagram::drawio_cache_key(path, page),
                    path: path.to_path_buf(),
                    page,
                }
            }
        };
        Some(job)
    }

    /// Gives the `width` and the `height` attribute of the diagram
    /// `node_ref` in the document `path`. `max_width` is the length of a
    /// `100%` width. An error names the diagram: the file as the document
    /// gives it, or the kind of a fenced diagram.
    pub(super) fn diagram_size(
        &self,
        arena: &Arena,
        source: &str,
        path: &Path,
        node_ref: NodeRef,
        max_width: u64,
    ) -> Result<SizeAttributes> {
        let diagram = as_extension_data!(arena, node_ref, crate::diagram::Diagram);
        self.size_attributes(arena, source, path, node_ref, max_width, || {
            if let Some(destination) = diagram.destination() {
                return format!("diagram `{destination}`");
            }
            let kind = match diagram.diagram_type() {
                crate::diagram::DiagramType::Mermaid => "mermaid",
                crate::diagram::DiagramType::PlantUml => "plantuml",
                crate::diagram::DiagramType::Typst => "typst",
                crate::diagram::DiagramType::DrawIo => "drawio",
                crate::diagram::DiagramType::D2 => "d2",
            };
            format!("{kind} diagram")
        })
    }

    /// Render a diagram to PNG and embed it as an inline image in its own
    /// paragraph. The paragraph has the paragraph style of the context, or
    /// `ImageBlock` when the context has none. The `width` and `height`
    /// attributes set the extent, and the extent fits in the text height
    /// and in the width of the paragraph style. An invalid attribute is an
    /// error, and the diagram is then not rendered: this function and
    /// [`Self::prefetch_diagrams`] check the attributes before a render.
    pub(super) fn render_diagram(
        &self,
        docx: Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Docx> {
        let path = &context.graph.document(context.doc_id).path;
        let style = context.paragraph_style.unwrap_or("ImageBlock");
        let max_width = self.style_width(style);
        let size = self.diagram_size(context.arena, context.source, path, node_ref, max_width)?;
        let Some(job) = self.diagram_job(context.arena, context.source, node_ref) else {
            return Ok(docx);
        };

        match job.render_cached(self.cache) {
            Ok(png_data) => {
                if let Some((pic, px)) = self.pic_from_png(png_data) {
                    let pic = self.fit_to_area(pic, px, job.ppi(), size, max_width);
                    let para = Paragraph::new()
                        .style(style)
                        .add_run(Run::new().add_image(pic));
                    return Ok(docx.add_paragraph(para));
                }
                self.logger
                    .log("warning: failed to create DOCX image from diagram PNG");
                Ok(docx)
            }
            Err(e) => {
                self.logger
                    .log(format!("warning: diagram rendering failed: {e}"));
                let para = Paragraph::new()
                    .style("DiagramError")
                    .add_run(Run::new().add_text(format!("[Diagram rendering error: {e}]")));
                Ok(docx.add_paragraph(para))
            }
        }
    }

    /// Decode the document logo (the front-matter `logo` file or the theme
    /// logo). An SVG logo is rasterised to PNG. The logo has its pixel size
    /// at 96 DPI.
    ///
    /// Returns `None` when the document has no logo, the logo does not
    /// decode, or the logo has a zero width or height.
    pub(super) fn theme_logo(&self) -> Option<Pic> {
        let (bytes, ext) = self.logo_asset.as_ref()?;
        let ext = ext.to_ascii_lowercase();

        let (pic, px) = if ext == "svg" {
            let png = self.svg_to_png(bytes)?;
            self.pic_from_png(png)?
        } else if ext == "png" {
            self.pic_from_png(bytes.clone())?
        } else {
            self.pic_from_raster(bytes)?
        };
        if px.0 == 0 || px.1 == 0 {
            self.logger
                .log("warning: logo has a zero width or height, skipping");
            return None;
        }
        let max = u64::from(u32::MAX);
        let (cx, cy) = extent(px, IMAGE_PPI, None, None, (max, max));
        Some(pic.size(cx, cy))
    }

    pub(super) fn svg_to_png(&self, svg_data: &[u8]) -> Option<Vec<u8>> {
        let opt = usvg::Options::default();
        let tree = usvg::Tree::from_data(svg_data, &opt).ok()?;
        let size = tree.size().to_int_size();
        let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())?;
        let mut pmut = pixmap.as_mut();
        resvg::render(&tree, tiny_skia::Transform::default(), &mut pmut);
        pixmap.encode_png().ok()
    }

    /// Load the image of an `Image` node as a `Pic` with its extent.
    ///
    /// The `width` and `height` attributes set the extent, and the extent
    /// fits in the text height and in the `max_width` of the context. An
    /// image in a paragraph with other content and without these
    /// attributes has the height of `1em`. Gives `None` for an image that
    /// does not load. An invalid attribute is an error, and the image file
    /// is then not read.
    pub(super) fn resolve_graph_image(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Option<Pic>> {
        let (arena, source) = (context.arena, context.source);
        let image = as_kind_data!(arena[node_ref], Image);
        let path = &context.graph.document(context.doc_id).path;
        let mut size =
            self.size_attributes(arena, source, path, node_ref, context.max_width, || {
                format!("image `{}`", image.destination().str(source))
            })?;
        if size == (None, None) && crate::html::is_inline_image(arena, node_ref) {
            size.1 = Some(self.em);
        }
        let Some(target) = context.graph.image_target(context.doc_id, node_ref) else {
            return Ok(None);
        };
        let Some((pic, px)) = self.resolve_image(target) else {
            return Ok(None);
        };
        Ok(Some(self.fit_to_area(
            pic,
            px,
            IMAGE_PPI,
            size,
            context.max_width,
        )))
    }

    /// Gives the `width` and the `height` attribute of the node `node_ref`
    /// in the document `path`. A width in `%` is a part of `max_width`, a
    /// height in `%` is a part of the text height. `what` gives the name of
    /// the node for an error message.
    fn size_attributes(
        &self,
        arena: &Arena,
        source: &str,
        path: &Path,
        node_ref: NodeRef,
        max_width: u64,
        what: impl Fn() -> String,
    ) -> Result<SizeAttributes> {
        let attributes = arena[node_ref].attributes();
        let text_height = self.text_area.1;
        let attribute = |name: &str, percent_of: u64| -> Result<Option<u64>> {
            let Some(value) = attributes.get(name) else {
                return Ok(None);
            };
            length(&value.str(source), percent_of, self.em)
                .map_err(|m| anyhow!("{}: {}: {name}: {m}", path.display(), what()))
        };
        Ok((
            attribute("width", max_width)?,
            attribute("height", text_height)?,
        ))
    }

    /// Sets the extent of `pic` from its pixel size `px` at `ppi` and the
    /// attributes `size`. The extent fits in `max_width` and in the text
    /// height.
    fn fit_to_area(
        &self,
        pic: Pic,
        px: (u32, u32),
        ppi: u32,
        size: SizeAttributes,
        max_width: u64,
    ) -> Pic {
        let (width, height) = size;
        let (cx, cy) = extent(px, ppi, width, height, (max_width, self.text_area.1));
        pic.size(cx, cy)
    }

    /// Gives the text width in EMU minus the indent `indent` in twips. The
    /// theme loader makes sure that each style indent and each list level
    /// indent is less than the text width.
    pub(super) fn indented_width(&self, indent: u32) -> u64 {
        self.text_area.0 - twips_to_emu(indent)
    }

    /// Gives the width in EMU of a paragraph with the style `style`: the
    /// text width minus the left indent of the style.
    pub(super) fn style_width(&self, style: &str) -> u64 {
        self.indented_width(self.params.style_indent(style).0)
    }

    /// Gives `context` with the `max_width` of a paragraph with the style
    /// `style`.
    pub(super) fn styled_context<'c>(
        &self,
        context: &RenderContext<'c>,
        style: &str,
    ) -> RenderContext<'c> {
        RenderContext {
            max_width: self.style_width(style),
            ..*context
        }
    }

    /// Resolve an image from the graph-resolved target path and load it as
    /// a `Pic` and its size in pixels.
    ///
    /// `target` is the value returned by `graph.image_target(doc_id,
    /// node_ref)` — typically `static/<relative-to-base-dir>`.  We strip
    /// the `static/` prefix and join with `base_dir` to locate the file
    /// on disk.
    ///
    /// Falls back to treating `target` as a raw relative path against
    /// `base_dir` when the prefix is absent.
    fn resolve_image(&self, target: &str) -> Option<(Pic, (u32, u32))> {
        if target.contains("://") || target.is_empty() {
            return None;
        }

        let rel = target.strip_prefix("static/").unwrap_or(target);
        let path = self.base_dir.join(rel);

        // SVG images are not supported by the DOCX `image` crate — skip
        // with a warning and do not read the file.
        let is_svg = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("svg"))
            .unwrap_or(false);
        if is_svg {
            self.logger.log(format!(
                "warning: SVG images are not supported in DOCX output, skipping {}",
                path.display()
            ));
            return None;
        }

        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                self.logger.log(format!(
                    "warning: cannot read image {}: {e}",
                    path.display()
                ));
                return None;
            }
        };

        self.pic_from_png(data)
    }

    /// Create a `Pic` from a non-PNG raster image (JPEG, GIF, etc.) and give
    /// its size in pixels. The caller sets the extent of the `Pic`.
    ///
    /// Uses `Pic::new` which internally decodes via the `image` crate and
    /// re-encodes to PNG.  The pixel size comes from the PNG header.
    fn pic_from_raster(&self, data: &[u8]) -> Option<(Pic, (u32, u32))> {
        // `Pic::new` panics on unsupported formats — catch that.
        let result = std::panic::catch_unwind(|| Pic::new(data));
        match result {
            Ok(pic) => {
                let px = detect_png_dimensions(&pic.image)?;
                Some((pic, px))
            }
            Err(_) => {
                self.logger
                    .log("warning: failed to decode image for DOCX embedding");
                None
            }
        }
    }

    /// Create a `Pic` from raw image bytes and give its size in pixels. The
    /// caller sets the extent of the `Pic`.
    ///
    /// Data with a PNG IHDR chunk is embedded as is, with the IHDR size.
    /// Other data goes through [`Self::pic_from_raster`]; data that does
    /// not decode gives `None`.
    pub(super) fn pic_from_png(&self, data: Vec<u8>) -> Option<(Pic, (u32, u32))> {
        if data.is_empty() {
            return None;
        }
        match detect_png_dimensions(&data) {
            // The size in `new_with_dimensions` is in pixels and can
            // overflow in EMU; the caller sets the extent.
            Some(px) => Some((Pic::new_with_dimensions(data, 0, 0), px)),
            None => self.pic_from_raster(&data),
        }
    }
}

/// Gives the width and the height of the text area of `page` in EMU.
pub(super) fn text_area(page: &Page) -> (u64, u64) {
    (
        u64::from(page.text_width()) * EMU_PER_TWIP,
        u64::from(page.text_height()) * EMU_PER_TWIP,
    )
}

/// Gives `twips` in EMU.
pub(super) fn twips_to_emu(twips: u32) -> u64 {
    u64::from(twips) * EMU_PER_TWIP
}

/// Gives `1em` in EMU: the font size of the `Normal` style, or 10 pt when
/// the style has no size.
pub(super) fn em(params: &DocxParams) -> u64 {
    let half_points = params
        .styles
        .get("Normal")
        .and_then(|style| style.size)
        .map_or(DEFAULT_EM_HALF_POINTS, |size| size.0);
    u64::from(half_points) * EMU_PER_HALF_POINT
}

/// Gives a copy of the logo `pic` with the height `height`. The width
/// keeps the aspect ratio of `pic`.
pub(super) fn logo_at_height(pic: &Pic, height: Twips) -> Pic {
    let (w, h) = pic.size;
    let height_emu = u64::from(height.0) * EMU_PER_TWIP;
    let width_emu = height_emu * u64::from(w) / u64::from(h.max(1));
    pic.clone().size(
        u32::try_from(width_emu).unwrap_or(u32::MAX),
        height_emu as u32,
    )
}

/// Detect PNG image dimensions from the IHDR chunk.
fn detect_png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 {
        return None;
    }
    if &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((width, height))
}
