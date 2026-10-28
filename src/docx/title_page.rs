use docx_rs::*;

use super::{DocxRenderer, ACCENT_ORANGE};

/// Title page values read from the document metadata.
pub(super) struct TitleFields<'m> {
    pub(super) title: &'m str,
    pub(super) subtitle: &'m str,
    pub(super) author: &'m str,
    pub(super) version: String,
    pub(super) date: &'m str,
}

impl<'m> TitleFields<'m> {
    /// Read the title page values from the document metadata.
    pub(super) fn from_metadata(metadata: &'m serde_json::Map<String, serde_json::Value>) -> Self {
        let title = metadata
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let subtitle = metadata
            .get("subtitle")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let author = metadata
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let version_str = metadata
            .get("version")
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => String::new(),
            })
            .unwrap_or_default();
        let date = metadata
            .get("date")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        Self {
            title,
            subtitle,
            author,
            version: version_str,
            date,
        }
    }
}

impl<'a> DocxRenderer<'a> {
    /// Add the title page: logo, title, subtitle, version, author, date, and a page break.
    pub(super) fn add_title_page(&self, mut docx: Docx, fields: &TitleFields<'_>) -> Docx {
        let cal_fonts = RunFonts::new()
            .ascii("Calibri")
            .hi_ansi("Calibri")
            .east_asia("Calibri")
            .cs("Calibri");

        if let Some(logo_pic) = self.resolve_front_logo() {
            docx = docx.add_paragraph(Paragraph::new().add_run(Run::new().add_image(logo_pic)));
        }

        for _ in 0..5 {
            let spacer = Paragraph::new().line_spacing(LineSpacing::new().after(0).line(240));
            docx = docx.add_paragraph(spacer);
        }

        let title_run = Run::new()
            .add_text(fields.title)
            .size(60)
            .bold()
            .fonts(cal_fonts.clone());
        let title_border = ParagraphBorders::with_empty().set(
            ParagraphBorder::new(ParagraphBorderPosition::Bottom)
                .val(BorderType::Single)
                .size(4)
                .space(1)
                .color(ACCENT_ORANGE),
        );
        let mut title_para = Paragraph::new()
            .add_run(title_run)
            .align(AlignmentType::Right)
            .line_spacing(LineSpacing::new().after(200).line(240))
            .color(ACCENT_ORANGE);
        title_para.property = title_para.property.set_borders(title_border);
        docx = docx.add_paragraph(title_para);

        if !fields.subtitle.is_empty() {
            let sub_run = Run::new()
                .add_text(fields.subtitle)
                .size(36)
                .color("666666")
                .fonts(cal_fonts.clone());
            docx = docx.add_paragraph(
                Paragraph::new()
                    .add_run(sub_run)
                    .align(AlignmentType::Center)
                    .line_spacing(LineSpacing::new().after(200).line(240)),
            );
        }

        if !fields.version.is_empty() {
            let ver_run = Run::new()
                .add_text("Version ")
                .size(28)
                .color("888888")
                .fonts(cal_fonts.clone());
            let ver_val_run = Run::new()
                .add_text(&fields.version)
                .size(28)
                .color("888888")
                .fonts(cal_fonts.clone());
            docx = docx.add_paragraph(
                Paragraph::new()
                    .add_run(ver_run)
                    .add_run(ver_val_run)
                    .align(AlignmentType::Right)
                    .line_spacing(LineSpacing::new().after(120).line(240)),
            );
        }

        if !fields.author.is_empty() {
            let author_run = Run::new()
                .add_text(fields.author)
                .size(24)
                .color("666666")
                .fonts(cal_fonts.clone());
            docx = docx.add_paragraph(
                Paragraph::new()
                    .add_run(author_run)
                    .align(AlignmentType::Center)
                    .line_spacing(LineSpacing::new().after(120).line(240)),
            );
        }

        if !fields.date.is_empty() {
            let date_run = Run::new()
                .add_text(fields.date)
                .size(24)
                .color("888888")
                .fonts(cal_fonts);
            docx = docx.add_paragraph(
                Paragraph::new()
                    .add_run(date_run)
                    .align(AlignmentType::Center)
                    .line_spacing(LineSpacing::new().after(120).line(240)),
            );
        }

        docx = docx.add_paragraph(
            Paragraph::new()
                .add_run(Run::new().add_break(BreakType::Page))
                .line_spacing(LineSpacing::new().after(0).line(240)),
        );

        docx
    }
}
