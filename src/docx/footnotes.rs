//! Word footnotes.
//!
//! Each footnote reference in the body gives a new Word footnote in
//! `word/footnotes.xml`, also when two references have the same label. The
//! renderer writes the footnote definition at each reference. The blocks
//! of the definition become flat paragraphs with the `FootnoteText` style:
//!
//! - A code block gives one `CodeBlock` paragraph for each line.
//! - The paragraphs of block quotes, fenced divs, admonitions, definition
//!   lists and captions become `FootnoteText` paragraphs. An admonition
//!   without a title heading starts with its default title.
//! - A list item starts with the bullet of its level or with its number.
//!   A task item has its box character in place of the bullet or after
//!   the number.
//! - A table gives one paragraph for each row, with a tab between cells.
//! - An image gives its alt text. A footnote reference in a footnote gives
//!   nothing.
//! - Diagrams, thematic breaks and hidden blocks give nothing.

use std::io::Write as _;

use anyhow::{anyhow, Result};
use docx_rs::xml::writer::EmitterConfig;
use docx_rs::*;
use rushdown::ast::{KindData, NodeRef};
use rushdown::{as_extension_data, as_kind_data};
use rushdown_definition_list::{DefinitionList, Term, TermDefinition};
use rushdown_fenced_div::FencedDiv;
use rushdown_footnote::FootnoteReference;

use crate::caption::{Caption, CaptionText};

use super::admonition::default_title;
use super::inline::run_child;
use super::numbering::LIST_LEVELS;
use super::{DocxRenderer, RenderContext};

/// The paragraph style of the footnote text.
const FOOTNOTE_TEXT: &str = "FootnoteText";

/// The start of the footnotes part: the separator and the continuation
/// separator footnotes.
const PART_START: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" "#,
    r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
    r#"<w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr>"#,
    r#"<w:r><w:separator/></w:r></w:p></w:footnote>"#,
    r#"<w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr>"#,
    r#"<w:r><w:continuationSeparator/></w:r></w:p></w:footnote>"#,
);

/// The end of the footnotes part.
const PART_END: &str = "</w:footnotes>";

/// The footnote mark and a space. The first paragraph of each footnote
/// starts with them.
const MARK: &str = concat!(
    r#"<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r>"#,
    r#"<w:r><w:t xml:space="preserve"> </w:t></w:r>"#,
);

/// The footnote properties of `word/settings.xml`: the IDs of the
/// separator footnotes.
pub(super) const FOOTNOTE_PROPERTIES: &str =
    r#"<w:footnotePr><w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr>"#;

/// The text of `word/footnotes.xml`, written while the renderer walks the
/// body.
#[derive(Default)]
pub(super) struct FootnotesPart {
    /// The part text. It is empty until the first footnote.
    xml: Vec<u8>,
    /// The ID of the last footnote.
    last_id: usize,
    /// True while the open footnote has no paragraph.
    mark_pending: bool,
}

impl FootnotesPart {
    /// Starts a footnote and gives its ID. The IDs start at 1.
    fn start(&mut self) -> Result<usize> {
        if self.xml.is_empty() {
            self.xml.extend_from_slice(PART_START.as_bytes());
        }
        self.last_id += 1;
        write!(self.xml, r#"<w:footnote w:id="{}">"#, self.last_id)?;
        self.mark_pending = true;
        Ok(self.last_id)
    }

    /// Writes a paragraph with the style `style` and the children `runs`
    /// to the open footnote.
    fn paragraph(&mut self, style: &str, runs: &[ParagraphChild]) -> Result<()> {
        write!(
            self.xml,
            r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr>"#
        )?;
        if std::mem::take(&mut self.mark_pending) {
            self.xml.extend_from_slice(MARK.as_bytes());
        }
        let config = EmitterConfig {
            write_document_declaration: false,
            perform_escaping: false,
            perform_indent: false,
            line_separator: "".into(),
        };
        let mut writer = config.create_writer(&mut self.xml);
        for run in runs {
            writer = run.build_to(writer)?;
        }
        self.xml.extend_from_slice(b"</w:p>");
        Ok(())
    }

    /// Ends the open footnote. A footnote without a paragraph gets a
    /// paragraph with the footnote mark.
    fn end(&mut self) -> Result<()> {
        if self.mark_pending {
            self.paragraph(FOOTNOTE_TEXT, &[])?;
        }
        self.xml.extend_from_slice(b"</w:footnote>");
        Ok(())
    }

    /// Gives the part, or `None` if the document has no footnotes.
    pub(super) fn finish(mut self) -> Option<Vec<u8>> {
        if self.xml.is_empty() {
            return None;
        }
        self.xml.extend_from_slice(PART_END.as_bytes());
        Some(self.xml)
    }
}

impl<'a> DocxRenderer<'a> {
    /// Render a footnote reference: write its definition as a new footnote
    /// and give the reference run. A reference in a footnote gives no run.
    pub(super) fn render_footnote_reference(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Vec<Run>> {
        if context.footnote {
            return Ok(Vec::new());
        }
        let arena = context.arena;
        let footnote = as_extension_data!(arena, node_ref, FootnoteReference);
        let label = footnote.label().str(context.source);
        let definition = context
            .graph
            .document(context.doc_id)
            .footnote_definition(&label)
            .ok_or_else(|| anyhow!("missing footnote definition for `{label}`"))?;

        let id = self.footnotes.borrow_mut().start()?;
        let context = RenderContext {
            heading_shift: 0,
            paragraph_style: None,
            footnote: true,
            ..*context
        };
        for child in arena[definition.node_ref].children(arena) {
            self.render_footnote_block(&context, child)?;
        }
        self.footnotes.borrow_mut().end()?;

        // docx-rs writes the footnote content only to its own footnotes
        // part, which the package replaces. The placeholder paragraph
        // stops docx-rs from printing to stderr for a footnote without
        // content. A paragraph made without `Paragraph::new()` takes no
        // paragraph ID. Cost: each reference holds a placeholder of about
        // 2.3 KB, and docx-rs clones it into the discarded part.
        let placeholder = Paragraph {
            id: String::new(),
            children: Vec::new(),
            property: ParagraphProperty::new(),
            has_numbering: false,
        };
        let footnote = Footnote {
            id,
            content: vec![placeholder],
        };
        Ok(vec![Run::new().add_footnote_reference(footnote)])
    }

    /// Write a paragraph to the open footnote.
    fn footnote_paragraph(&self, style: &str, runs: &[ParagraphChild]) -> Result<()> {
        self.footnotes.borrow_mut().paragraph(style, runs)
    }

    /// Render a block of a footnote definition to footnote paragraphs.
    fn render_footnote_block(&self, context: &RenderContext<'_>, node_ref: NodeRef) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        if !crate::conditional::node_is_visible(arena, node_ref, source, &self.profile)? {
            return Ok(());
        }
        match arena[node_ref].kind_data() {
            KindData::Paragraph(_) | KindData::Heading(_) => {
                let runs = self.collect_child_runs(context, node_ref)?;
                self.footnote_paragraph(FOOTNOTE_TEXT, &runs)?;
            }
            KindData::CodeBlock(code_block) => {
                let mut code_text = String::new();
                for line in code_block.value().iter(source) {
                    code_text.push_str(&line);
                }
                for line in code_text.lines() {
                    self.footnote_paragraph("CodeBlock", &[run_child(Run::new().add_text(line))])?;
                }
            }
            KindData::Blockquote(_) => {
                for child in arena[node_ref].children(arena) {
                    self.render_footnote_block(context, child)?;
                }
            }
            KindData::List(_) => {
                self.render_footnote_list(context, node_ref, 0)?;
            }
            KindData::Table(_) => {
                self.render_footnote_table(context, node_ref)?;
            }
            _ => {
                if rushdown::matches_extension_kind!(arena, node_ref, Caption) {
                    self.render_footnote_caption(context, node_ref)?;
                } else if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::diagram::Diagram
                ) {
                    self.logger.log(
                        "warning: diagrams in footnotes are not supported in DOCX output, skipping",
                    );
                } else if rushdown::matches_extension_kind!(arena, node_ref, FencedDiv) {
                    let classes = arena[node_ref]
                        .attributes()
                        .get("class")
                        .map(|v| v.str(source))
                        .unwrap_or_default();
                    if let Some(adm_type) = crate::admonition_type(&classes) {
                        let has_title =
                            arena[node_ref].children(arena).next().is_some_and(|first| {
                                matches!(arena[first].kind_data(), KindData::Heading(_))
                            });
                        if !has_title {
                            let title = run_child(Run::new().add_text(default_title(adm_type)));
                            self.footnote_paragraph(FOOTNOTE_TEXT, &[title])?;
                        }
                    }
                    for child in arena[node_ref].children(arena) {
                        self.render_footnote_block(context, child)?;
                    }
                } else if rushdown::matches_extension_kind!(arena, node_ref, DefinitionList) {
                    for child in arena[node_ref].children(arena) {
                        if rushdown::matches_extension_kind!(arena, child, Term) {
                            let runs = self.collect_child_runs(context, child)?;
                            self.footnote_paragraph(FOOTNOTE_TEXT, &runs)?;
                        } else if rushdown::matches_extension_kind!(arena, child, TermDefinition) {
                            for def_child in arena[child].children(arena) {
                                self.render_footnote_block(context, def_child)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Render a list of a footnote definition at the list level `level`.
    /// The first paragraph of an item starts with the bullet of the level,
    /// or with the number of the item in an ordered list. A task item
    /// starts with its box character in place of the bullet, or after the
    /// number. An item that is empty or starts with another block gets a
    /// paragraph with only the marker.
    fn render_footnote_list(
        &self,
        context: &RenderContext<'_>,
        list_ref: NodeRef,
        level: usize,
    ) -> Result<()> {
        let arena = context.arena;
        let list = as_kind_data!(arena[list_ref], List);
        let bullet = &self.params.lists.bullet_list[level.min(LIST_LEVELS - 1)];
        let mut number = list.start().max(1) as usize;
        for item_ref in arena[list_ref].children(arena) {
            let KindData::ListItem(item) = arena[item_ref].kind_data() else {
                continue;
            };
            let task_box = self.task_box(item.task());
            let mut marker = Some(match (list.is_ordered(), task_box) {
                (true, Some(task_box)) => format!("{number}. {task_box} "),
                (true, None) => format!("{number}. "),
                (false, task_box) => format!("{} ", task_box.unwrap_or(bullet)),
            });
            number += 1;
            let starts_with_paragraph = arena[item_ref]
                .children(arena)
                .next()
                .is_some_and(|child| matches!(arena[child].kind_data(), KindData::Paragraph(_)));
            if !starts_with_paragraph {
                let marker = marker.take().unwrap_or_default();
                self.footnote_paragraph(FOOTNOTE_TEXT, &[run_child(Run::new().add_text(marker))])?;
            }
            for child in arena[item_ref].children(arena) {
                match arena[child].kind_data() {
                    KindData::Paragraph(_) if marker.is_some() => {
                        let mut runs = vec![run_child(
                            Run::new().add_text(marker.take().unwrap_or_default()),
                        )];
                        runs.extend(self.collect_child_runs(context, child)?);
                        self.footnote_paragraph(FOOTNOTE_TEXT, &runs)?;
                    }
                    KindData::List(_) => {
                        self.render_footnote_list(context, child, level + 1)?;
                    }
                    _ => {
                        self.render_footnote_block(context, child)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Render a table of a footnote definition: one paragraph for each
    /// row, with a tab between the cells.
    fn render_footnote_table(&self, context: &RenderContext<'_>, table_ref: NodeRef) -> Result<()> {
        let arena = context.arena;
        for part in arena[table_ref].children(arena) {
            if !matches!(
                arena[part].kind_data(),
                KindData::TableHeader(_) | KindData::TableBody(_)
            ) {
                continue;
            }
            for row in arena[part].children(arena) {
                let mut runs = Vec::new();
                let cells = arena[row]
                    .children(arena)
                    .filter(|&cell| matches!(arena[cell].kind_data(), KindData::TableCell(_)));
                for (i, cell) in cells.enumerate() {
                    if i > 0 {
                        runs.push(run_child(Run::new().add_tab()));
                    }
                    runs.extend(self.collect_child_runs(context, cell)?);
                }
                self.footnote_paragraph(FOOTNOTE_TEXT, &runs)?;
            }
        }
        Ok(())
    }

    /// Render a caption of a footnote definition: the caption text, then
    /// the block. A caption of a hidden block gives nothing.
    fn render_footnote_caption(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        for child in arena[node_ref].children(arena) {
            let is_text = rushdown::matches_extension_kind!(arena, child, CaptionText);
            if !is_text
                && !crate::conditional::node_is_visible(
                    arena,
                    child,
                    context.source,
                    &self.profile,
                )?
            {
                return Ok(());
            }
        }
        for child in arena[node_ref].children(arena) {
            if rushdown::matches_extension_kind!(arena, child, CaptionText) {
                let runs = self.collect_child_runs(context, child)?;
                self.footnote_paragraph(FOOTNOTE_TEXT, &runs)?;
            } else {
                self.render_footnote_block(context, child)?;
            }
        }
        Ok(())
    }
}
