use docx_rs::*;

use super::{DocxRenderer, ACCENT_ORANGE};

impl<'a> DocxRenderer<'a> {
    pub(super) fn build_default_header(&self, title: &str) -> Header {
        let header_border = ParagraphBorders::with_empty().set(
            ParagraphBorder::new(ParagraphBorderPosition::Bottom)
                .val(BorderType::Single)
                .size(4)
                .space(1)
                .color(ACCENT_ORANGE),
        );

        let text_width = self.params.page.text_width() as usize;
        let mut para = Paragraph::new();
        para.property = para
            .property
            .style("Header")
            .set_borders(header_border)
            .add_tab(Tab::new().val(TabValueType::Center).pos(text_width / 2))
            .add_tab(Tab::new().val(TabValueType::Right).pos(text_width));

        let run_style = Run::new().color(ACCENT_ORANGE).size(18);

        para = para
            .add_run(run_style.clone().add_tab())
            .add_run(run_style.clone().add_tab())
            .add_run(
                run_style
                    .clone()
                    .add_field_char(FieldCharType::Begin, false),
            )
            .add_run(run_style.clone().add_instr_text(InstrText::Unsupported(
                " TITLE  \\* MERGEFORMAT ".to_string(),
            )))
            .add_run(
                run_style
                    .clone()
                    .add_field_char(FieldCharType::Separate, false),
            );

        if !title.is_empty() {
            para = para.add_run(run_style.clone().add_text(title));
        }

        para = para.add_run(run_style.add_field_char(FieldCharType::End, false));

        Header::new().add_paragraph(para)
    }

    pub(super) fn build_default_footer(&self, version_str: &str) -> Footer {
        let footer_border = ParagraphBorders::with_empty().set(
            ParagraphBorder::new(ParagraphBorderPosition::Top)
                .val(BorderType::Single)
                .size(4)
                .space(1)
                .color(ACCENT_ORANGE),
        );

        let text_width = self.params.page.text_width() as usize;
        let mut para = Paragraph::new();
        para.property = para
            .property
            .style("Footer")
            .set_borders(footer_border)
            .add_tab(Tab::new().val(TabValueType::Center).pos(text_width / 2))
            .add_tab(Tab::new().val(TabValueType::Right).pos(text_width));

        let run_style = Run::new().color(ACCENT_ORANGE).size(18);

        para = para
            .add_run(
                run_style
                    .clone()
                    .add_field_char(FieldCharType::Begin, false),
            )
            .add_run(run_style.clone().add_instr_text(InstrText::Unsupported(
                " DOCPROPERTY Version \\* MERGEFORMAT ".to_string(),
            )))
            .add_run(
                run_style
                    .clone()
                    .add_field_char(FieldCharType::Separate, false),
            );

        if !version_str.is_empty() {
            para = para.add_run(run_style.clone().add_text(version_str));
        }

        para = para
            .add_run(run_style.clone().add_field_char(FieldCharType::End, false))
            .add_run(run_style.clone().add_tab())
            .add_run(run_style.clone().add_tab())
            .add_run(
                run_style
                    .clone()
                    .add_field_char(FieldCharType::Begin, false),
            )
            .add_run(
                run_style
                    .clone()
                    .add_instr_text(InstrText::PAGE(InstrPAGE::new())),
            )
            .add_run(
                run_style
                    .clone()
                    .add_field_char(FieldCharType::Separate, false),
            )
            .add_run(run_style.clone().add_text("1"))
            .add_run(run_style.add_field_char(FieldCharType::End, false));

        Footer::new().add_paragraph(para)
    }
}
