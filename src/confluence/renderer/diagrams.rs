use std::fmt::Write as _;

use anyhow::{anyhow, Result};
use rushdown::as_extension_data;
use rushdown::ast::NodeRef;

use super::types::RenderContext;
use super::xml::xml_attr_escape;
use super::{ConfluenceAttachment, ConfluenceRenderer};

impl<'a> ConfluenceRenderer<'a> {
    pub(super) fn render_diagram(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        let diagram = as_extension_data!(arena, node_ref, crate::diagram::Diagram);
        let src = crate::diagram::collect_diagram_source(diagram, source);
        let attrs = arena[node_ref].attributes();
        let width = attrs.get("width").map(|v| v.str(source).to_string());
        let height = attrs.get("height").map(|v| v.str(source).to_string());

        let svg_bytes = self.render_diagram_to_svg(diagram, &src)?;

        let n = {
            let mut counter = self.diagram_counter.borrow_mut();
            let n = *counter;
            *counter += 1;
            n
        };
        let filename = format!("diagram-{n}.svg");
        let hash = format!("blake3:{}", blake3::hash(&svg_bytes).to_hex());

        self.attachment_names
            .borrow_mut()
            .insert(format!("__diagram_{n}"), filename.clone());

        out.push_str("<ac:image");
        // Diagrams are always block-level SVGs.
        self.write_image_size_attrs(
            out,
            width.as_deref(),
            height.as_deref(),
            true,
            false,
            Some(&svg_bytes),
        );
        writeln!(
            out,
            "><ri:attachment ri:filename=\"{}\" /></ac:image>",
            xml_attr_escape(&filename)
        )?;

        self.attachments.borrow_mut().push(ConfluenceAttachment {
            filename: filename.clone(),
            data: svg_bytes,
            content_hash: hash,
        });
        Ok(())
    }

    /// Render a diagram to SVG bytes for Confluence attachment upload.
    fn render_diagram_to_svg(
        &self,
        diagram: &crate::diagram::Diagram,
        src: &str,
    ) -> Result<Vec<u8>> {
        match diagram.diagram_type() {
            crate::diagram::DiagramType::PlantUml => {
                let themed = crate::diagram::apply_plantuml_theme(
                    src,
                    self.diagram_themes.plantuml.as_deref(),
                );
                crate::diagram::cached_render_bytes(
                    self.cache,
                    themed.as_bytes(),
                    "plantuml.svg",
                    || {
                        crate::diagram::render_plantuml_svg(None, themed.as_bytes())
                            .map_err(|e| anyhow!("{e}"))
                    },
                )
            }
            crate::diagram::DiagramType::Mermaid => {
                let themed = crate::diagram::apply_mermaid_theme(
                    src,
                    self.diagram_themes.mermaid.as_deref(),
                );
                crate::diagram::cached_render_svg(
                    self.cache,
                    themed.as_bytes(),
                    "mermaid.svg",
                    || crate::diagram::render_mermaid_svg_auto(None, &themed),
                )
                .map(|s| s.into_bytes())
            }
            crate::diagram::DiagramType::D2 => {
                let themed = crate::diagram::apply_d2_theme(src, self.diagram_themes.d2.as_deref());
                crate::diagram::cached_render_svg(self.cache, themed.as_bytes(), "d2.svg", || {
                    crate::diagram::render_d2_svg(None, &themed).map_err(|e| anyhow!("{e}"))
                })
                .map(|s| s.into_bytes())
            }
            crate::diagram::DiagramType::DrawIo => {
                if let Some(path) = diagram.drawio_path() {
                    let cache_key = crate::diagram::drawio_cache_key(path, diagram.drawio_page());
                    crate::diagram::cached_render_bytes(
                        self.cache,
                        &cache_key,
                        "drawio.svg",
                        || {
                            crate::diagram::render_drawio(
                                None,
                                path,
                                diagram.drawio_page(),
                                crate::diagram::DrawioFormat::Svg,
                            )
                            .map_err(|e| anyhow!("{e}"))
                        },
                    )
                } else {
                    Err(anyhow!("Draw.io diagram without a file path"))
                }
            }
            crate::diagram::DiagramType::Typst => {
                crate::diagram::cached_render_svg(self.cache, src.as_bytes(), "typst.svg", || {
                    crate::diagram::render_typst_svg(None, src).map_err(|e| anyhow!("{e}"))
                })
                .map(|s| s.into_bytes())
            }
        }
    }
}
