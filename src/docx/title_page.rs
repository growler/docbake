use docx_rs::*;

use super::fields::add_field;
use super::media::logo_at_height;
use super::DocxRenderer;

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
    pub(super) fn add_title_page(
        &self,
        mut docx: Docx,
        fields: &TitleFields<'_>,
        logo: Option<&Pic>,
    ) -> Docx {
        if let Some(logo) = logo {
            let pic = logo_at_height(logo, self.params.logo.title_height);
            docx = docx.add_paragraph(Paragraph::new().add_run(Run::new().add_image(pic)));
        }

        docx = docx.add_paragraph(add_field(
            Paragraph::new().style("Title"),
            InstrText::Unsupported(" TITLE  \\* MERGEFORMAT ".to_string()),
            fields.title,
        ));

        let version = (!fields.version.is_empty()).then(|| format!("Version {}", fields.version));
        for (style, text) in [
            ("Subtitle", fields.subtitle),
            ("DocumentVersion", version.as_deref().unwrap_or_default()),
            ("DocumentAuthor", fields.author),
            ("DocumentDate", fields.date),
        ] {
            if !text.is_empty() {
                docx = docx.add_paragraph(
                    Paragraph::new()
                        .style(style)
                        .add_run(Run::new().add_text(text)),
                );
            }
        }

        docx = docx.add_paragraph(Paragraph::new().add_run(Run::new().add_break(BreakType::Page)));

        docx
    }
}
