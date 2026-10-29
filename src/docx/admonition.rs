use anyhow::Result;
use docx_rs::*;
use rushdown::ast::{KindData, NodeRef};

use super::{DocxRenderer, RenderContext};

/// The styles and the default title of each admonition type: (type, header
/// style ID, body style ID, default title). The theme `docx.yaml` defines
/// the styles. A header style is based on `AdmonitionHeader`. A body style
/// is based on `AdmonitionBody`.
const ADMONITION_STYLES: &[(&str, &str, &str, &str)] = &[
    ("note", "AdmonitionNoteHeader", "AdmonitionNoteBody", "Note"),
    (
        "warning",
        "AdmonitionWarningHeader",
        "AdmonitionWarningBody",
        "Warning",
    ),
    (
        "important",
        "AdmonitionImportantHeader",
        "AdmonitionImportantBody",
        "Important",
    ),
    (
        "caution",
        "AdmonitionCautionHeader",
        "AdmonitionCautionBody",
        "Caution",
    ),
    ("tip", "AdmonitionTipHeader", "AdmonitionTipBody", "Tip"),
    (
        "related",
        "AdmonitionRelatedHeader",
        "AdmonitionRelatedBody",
        "Related",
    ),
];

/// Gives the default title of the admonition type `adm_type`.
pub(super) fn default_title(adm_type: &str) -> &'static str {
    ADMONITION_STYLES
        .iter()
        .find(|(t, ..)| *t == adm_type)
        .expect("ADMONITION_STYLES covers ADMONITION_TYPES")
        .3
}

impl<'a> DocxRenderer<'a> {
    /// Render an admonition block with the styles of its type.
    ///
    /// - Title paragraph: the header style of the type.
    /// - Body paragraphs: the body style of the type.
    /// - The styles carry the fill and the border. Adjacent paragraphs
    ///   with identical borders merge into a continuous box in Word and
    ///   Google Docs.
    /// - Other body blocks (lists, tables, code, diagrams, block quotes)
    ///   keep their own styles and are out of the box.
    pub(super) fn render_admonition(
        &self,
        mut docx: Docx,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
        adm_type: &str,
    ) -> Result<Docx> {
        let arena = context.arena;
        let children: Vec<_> = arena[node_ref].children(arena).collect();
        let &(_, header_style, body_style, default_title) = ADMONITION_STYLES
            .iter()
            .find(|(t, ..)| *t == adm_type)
            .expect("ADMONITION_STYLES covers ADMONITION_TYPES");

        let (title, skip_first) = if let Some(&first) = children.first() {
            if matches!(arena[first].kind_data(), KindData::Heading(_)) {
                let title = self.collect_text_runs(context, first, None)?;
                (title, true)
            } else {
                (vec![Run::new().add_text(default_title)], false)
            }
        } else {
            (vec![Run::new().add_text(default_title)], false)
        };

        // ── Title paragraph ─────────────────────────────────────────
        let mut title_para = Paragraph::new().style(header_style);
        for run in title {
            title_para = title_para.add_run(run);
        }
        docx = docx.add_paragraph(title_para);

        // ── Body children ───────────────────────────────────────────
        let body = if skip_first {
            &children[1..]
        } else {
            &children[..]
        };
        let context = RenderContext {
            paragraph_style: Some(body_style),
            ..*context
        };
        for &child in body {
            docx = self.render_block(docx, &context, child)?;
        }

        Ok(docx)
    }
}

#[cfg(test)]
mod tests {
    use super::ADMONITION_STYLES;
    use crate::theme::{Theme, EMBEDDED_THEME_NAMES};
    use crate::ADMONITION_TYPES;

    #[test]
    fn admonition_styles_cover_each_type() {
        let types: Vec<_> = ADMONITION_STYLES.iter().map(|(t, ..)| *t).collect();
        assert_eq!(types, ADMONITION_TYPES);
        for name in EMBEDDED_THEME_NAMES {
            let params = Theme::Embedded(name).docx_params().unwrap();
            for (t, header, body, _) in ADMONITION_STYLES {
                for (id, base) in [(header, "AdmonitionHeader"), (body, "AdmonitionBody")] {
                    let style = params
                        .styles
                        .get(*id)
                        .unwrap_or_else(|| panic!("{name}: {t}: no style {id}"));
                    assert_eq!(style.based_on.as_deref(), Some(base), "{name}: {id}");
                }
            }
        }
    }
}
