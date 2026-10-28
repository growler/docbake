use anyhow::Result;
use docx_rs::*;
use rushdown::ast::{KindData, NodeRef};

use super::{DocxRenderer, RenderContext};

/// Admonition colour scheme: (border, title_fill, body_fill).
/// Inspired by the MS Office styling captured from office.docx.
fn admonition_colors(adm_type: &str) -> (&'static str, &'static str, &'static str) {
    match adm_type {
        // green
        "tip" => ("8DD873", "B3E5A1", "D9F2D0"),
        // blue
        "note" => ("6CB4EE", "A6D1F2", "D4E9F9"),
        // red-orange
        "warning" => ("E8895C", "F2C4A6", "F9E2D4"),
        // orange
        "caution" => ("E8B95C", "F2DCA6", "F9EED4"),
        // dark red
        "important" => ("E86C6C", "F2ADAD", "F9D6D6"),
        // grey-blue
        "related" => ("8CA1B8", "BCCBDA", "DDE5ED"),
        _ => ("8CA1B8", "BCCBDA", "DDE5ED"),
    }
}

impl<'a> DocxRenderer<'a> {
    /// Render an admonition block with coloured borders and background.
    ///
    /// - Title paragraph: darker background, bold text, coloured border.
    /// - Body paragraphs: lighter background, same coloured border.
    /// - Adjacent paragraphs with identical borders merge into a
    ///   continuous box in Word and Google Docs.
    pub(super) fn render_admonition(
        &self,
        mut docx: Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
        adm_type: &str,
    ) -> Result<Docx> {
        let arena = context.arena;
        let source = context.source;
        let children: Vec<_> = arena[node_ref].children(arena).collect();

        let (title, skip_first) = if let Some(&first) = children.first() {
            if matches!(arena[first].kind_data(), KindData::Heading(_)) {
                let title = Self::collect_inline_text(arena, source, first);
                (title, true)
            } else {
                (admonition_default_title(adm_type).to_string(), false)
            }
        } else {
            (admonition_default_title(adm_type).to_string(), false)
        };

        let (border_color, title_fill, body_fill) = admonition_colors(adm_type);

        let make_borders = |color: &str| {
            ParagraphBorders::with_empty()
                .set(
                    ParagraphBorder::new(ParagraphBorderPosition::Top)
                        .val(BorderType::Single)
                        .size(4)
                        .space(1)
                        .color(color),
                )
                .set(
                    ParagraphBorder::new(ParagraphBorderPosition::Bottom)
                        .val(BorderType::Single)
                        .size(4)
                        .space(1)
                        .color(color),
                )
                .set(
                    ParagraphBorder::new(ParagraphBorderPosition::Left)
                        .val(BorderType::Single)
                        .size(4)
                        .space(4)
                        .color(color),
                )
                .set(
                    ParagraphBorder::new(ParagraphBorderPosition::Right)
                        .val(BorderType::Single)
                        .size(4)
                        .space(4)
                        .color(color),
                )
        };

        let adm_borders = make_borders(border_color);

        // ── Title paragraph ─────────────────────────────────────────
        let title_run = Run::new().add_text(&title).bold().size(22);
        let mut title_para = Paragraph::new()
            .add_run(title_run)
            .line_spacing(LineSpacing::new().before(120).after(60).line(240));
        title_para.property = title_para
            .property
            .shading(Shading::new().fill(title_fill))
            .set_borders(adm_borders.clone());
        docx = docx.add_paragraph(title_para);

        // ── Body children ───────────────────────────────────────────
        // We need to apply the body styling to each body paragraph.
        // For simple paragraphs we can set borders/shading directly.
        // For complex blocks (tables, lists, code) we render them
        // normally (they break out of the box visually, which is
        // acceptable — same limitation as Google Docs import).
        let body = if skip_first {
            &children[1..]
        } else {
            &children[..]
        };
        let body_borders = adm_borders;
        for &child in body {
            match arena[child].kind_data() {
                KindData::Paragraph(_) => {
                    let mut para = Paragraph::new();
                    for inline in arena[child].children(arena) {
                        match arena[inline].kind_data() {
                            KindData::Image(_) => {
                                if let Some(pic) = self.resolve_graph_image(context, inline) {
                                    para = para.add_run(Run::new().add_image(pic));
                                }
                            }
                            _ => {
                                let runs = self.render_inline_runs(context, inline);
                                for run in runs {
                                    para = para.add_run(run);
                                }
                            }
                        }
                    }
                    para.property = para
                        .property
                        .shading(Shading::new().fill(body_fill))
                        .set_borders(body_borders.clone());
                    docx = docx.add_paragraph(para);
                }
                _ => {
                    // Complex blocks (lists, tables, code, nested divs):
                    // render normally without the admonition box styling.
                    docx = self.render_block(docx, context, child)?;
                }
            }
        }

        Ok(docx)
    }
}

/// Default admonition title for a given type.
fn admonition_default_title(adm_type: &str) -> &'static str {
    match adm_type {
        "note" => "Note",
        "warning" => "Warning",
        "important" => "Important",
        "caution" => "Caution",
        "tip" => "Tip",
        "related" => "Related",
        _ => "Note",
    }
}
