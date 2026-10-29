use std::path::PathBuf;

use anyhow::{anyhow, Result};
use docx_rs::*;
use resvg::{tiny_skia, usvg};
use rushdown::as_extension_data;
use rushdown::ast::{Arena, NodeRef};

use crate::cache::Cache;

use super::params::Twips;
use super::{DocxRenderer, RenderContext};

/// EMU per pixel at 96 DPI (1 inch = 914400 EMU, 1 inch = 96 px).
const EMU_PER_PX: u32 = 9525;

/// Maximum image width in EMU (~6 inches at 96 DPI).
const MAX_IMAGE_WIDTH_EMU: u32 = 5_760_000;

/// EMU per twip (1 twip = 914400 / 1440 EMU).
const EMU_PER_TWIP: u64 = 635;

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

    /// Render a diagram to PNG and embed it as an inline image.
    pub(super) fn render_diagram(
        &self,
        docx: Docx,
        arena: &Arena,
        source: &str,
        node_ref: NodeRef,
    ) -> Result<Docx> {
        let Some(job) = self.diagram_job(arena, source, node_ref) else {
            return Ok(docx);
        };

        match job.render_cached(self.cache) {
            Ok(png_data) => {
                if let Some(pic) = self.pic_from_png(png_data) {
                    let para = Paragraph::new().add_run(Run::new().add_image(pic));
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

    /// Decode the theme logo. An SVG logo is rasterised to PNG.
    ///
    /// Returns `None` when the theme has no logo, the logo does not
    /// decode, or the logo has a zero width or height.
    pub(super) fn theme_logo(&self) -> Option<Pic> {
        let (bytes, ext) = self.logo_asset.as_ref()?;
        let ext = ext.to_ascii_lowercase();

        let pic = if ext == "svg" {
            let png = self.svg_to_png(bytes)?;
            self.pic_from_png(png)?
        } else if ext == "png" {
            self.pic_from_png(bytes.clone())?
        } else {
            self.pic_from_raster(bytes)?
        };
        if pic.size.0 == 0 || pic.size.1 == 0 {
            self.logger
                .log("warning: theme logo has a zero width or height, skipping");
            return None;
        }
        Some(pic)
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

    /// Look up the graph-resolved image target for an `Image` node and
    /// load the file as a `Pic`.
    pub(super) fn resolve_graph_image(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Option<Pic> {
        let target = context.graph.image_target(context.doc_id, node_ref)?;
        self.resolve_image(target)
    }

    /// Resolve an image from the graph-resolved target path and load it as
    /// a `Pic`.
    ///
    /// `target` is the value returned by `graph.image_target(doc_id,
    /// node_ref)` — typically `static/<relative-to-base-dir>`.  We strip
    /// the `static/` prefix and join with `base_dir` to locate the file
    /// on disk.
    ///
    /// Falls back to treating `target` as a raw relative path against
    /// `base_dir` when the prefix is absent.
    fn resolve_image(&self, target: &str) -> Option<Pic> {
        if target.contains("://") || target.is_empty() {
            return None;
        }

        let rel = target.strip_prefix("static/").unwrap_or(target);
        let path = self.base_dir.join(rel);

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

        if data.is_empty() {
            return None;
        }

        // SVG images are not supported by the DOCX `image` crate — skip
        // with a warning.
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

        // PNG data is embedded as is, with the IHDR dimensions.
        // For other raster formats (JPEG, GIF, …) `Pic::new` from
        // docx-rs runs the full `image` crate decoder (which also
        // converts the data to PNG internally).
        if detect_png_dimensions(&data).is_some() {
            self.pic_from_png(data)
        } else {
            self.pic_from_raster(&data)
        }
    }

    /// Create a `Pic` from a non-PNG raster image (JPEG, GIF, etc.).
    ///
    /// Uses `Pic::new` which internally decodes via the `image` crate and
    /// re-encodes to PNG.  We then apply the `MAX_IMAGE_WIDTH_EMU` cap.
    fn pic_from_raster(&self, data: &[u8]) -> Option<Pic> {
        // `Pic::new` panics on unsupported formats — catch that.
        let result = std::panic::catch_unwind(|| Pic::new(data));
        match result {
            Ok(pic) => {
                // Apply our width cap (Pic stores size as (w_emu, h_emu)).
                let (w_emu, h_emu) = pic.size;
                let (final_w, final_h) = if w_emu > MAX_IMAGE_WIDTH_EMU {
                    let scale = MAX_IMAGE_WIDTH_EMU as f64 / w_emu as f64;
                    (MAX_IMAGE_WIDTH_EMU, (h_emu as f64 * scale) as u32)
                } else {
                    (w_emu, h_emu)
                };
                Some(pic.size(final_w, final_h))
            }
            Err(_) => {
                self.logger
                    .log("warning: failed to decode image for DOCX embedding");
                None
            }
        }
    }

    /// Create a `Pic` from raw image bytes with size constraints.
    ///
    /// Data with a PNG IHDR chunk is embedded as is.  Other data goes
    /// through `Pic::new`, which decodes it and converts it to PNG; data
    /// that does not decode gives `None`.
    pub(super) fn pic_from_png(&self, data: Vec<u8>) -> Option<Pic> {
        if data.is_empty() {
            return None;
        }
        let dimensions = detect_png_dimensions(&data);
        let (w_px, h_px) = dimensions.unwrap_or((800, 600));
        let w_emu = w_px * EMU_PER_PX;
        let h_emu = h_px * EMU_PER_PX;

        let (final_w, final_h) = if w_emu > MAX_IMAGE_WIDTH_EMU {
            let scale = MAX_IMAGE_WIDTH_EMU as f64 / w_emu as f64;
            (MAX_IMAGE_WIDTH_EMU, (h_emu as f64 * scale) as u32)
        } else {
            (w_emu, h_emu)
        };

        let pic = match dimensions {
            Some((w_px, h_px)) => Pic::new_with_dimensions(data, w_px, h_px),
            None => {
                // `Pic::new` panics on data it cannot decode — catch that.
                match std::panic::catch_unwind(|| Pic::new(&data)) {
                    Ok(pic) => pic,
                    Err(_) => {
                        self.logger
                            .log("warning: failed to decode image for DOCX embedding");
                        return None;
                    }
                }
            }
        };
        Some(pic.size(final_w, final_h))
    }
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
