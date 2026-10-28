use docx_rs::*;

use super::fields::add_field;
use super::media::logo_at_height;
use super::DocxRenderer;

impl<'a> DocxRenderer<'a> {
    pub(super) fn build_default_header(&self, title: &str, logo: Option<&Pic>) -> Header {
        let text_width = self.params.page.text_width() as usize;
        let mut para = Paragraph::new();
        para.property = para
            .property
            .style("Header")
            .add_tab(Tab::new().val(TabValueType::Center).pos(text_width / 2))
            .add_tab(Tab::new().val(TabValueType::Right).pos(text_width));

        if let Some(logo) = logo {
            let pic = logo_at_height(logo, self.params.logo.header_height);
            para = para.add_run(Run::new().add_image(pic));
        }

        para = para
            .add_run(Run::new().add_tab())
            .add_run(Run::new().add_tab());
        para = add_field(
            para,
            InstrText::Unsupported(" TITLE  \\* MERGEFORMAT ".to_string()),
            title,
        );

        Header::new().add_paragraph(para)
    }

    pub(super) fn build_default_footer(&self, version_str: &str) -> Footer {
        let text_width = self.params.page.text_width() as usize;
        let mut para = Paragraph::new();
        para.property = para
            .property
            .style("Footer")
            .add_tab(Tab::new().val(TabValueType::Center).pos(text_width / 2))
            .add_tab(Tab::new().val(TabValueType::Right).pos(text_width));

        para = add_field(
            para,
            InstrText::Unsupported(" DOCPROPERTY Version \\* MERGEFORMAT ".to_string()),
            version_str,
        );
        para = para
            .add_run(Run::new().add_tab())
            .add_run(Run::new().add_tab());
        para = add_field(para, InstrText::PAGE(InstrPAGE::new()), "1");

        Footer::new().add_paragraph(para)
    }
}
