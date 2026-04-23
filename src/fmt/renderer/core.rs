use anyhow::Result;
use rushdown::ast::{Arena, NodeRef};

use super::footnotes::collect_footnote_labels;
use super::MarkdownFmtRenderer;

impl MarkdownFmtRenderer {
    pub(crate) fn new(paragraph_width: usize, align_tables: bool) -> Self {
        Self {
            paragraph_width,
            align_tables,
        }
    }

    pub(crate) fn render_document(
        &self,
        arena: &Arena,
        doc_ref: NodeRef,
        source: &str,
    ) -> Result<String> {
        let mut out = String::new();
        let footnote_labels = collect_footnote_labels(arena, doc_ref, source)?;
        for child in arena[doc_ref].children(arena) {
            self.render_block(&mut out, arena, child, source, &footnote_labels)?;
        }
        Ok(out)
    }

    pub(super) fn wrap_paragraph(&self, paragraph: &str) -> String {
        let text = paragraph.trim();
        if text.is_empty() {
            return String::new();
        }
        if text.contains('\n') {
            return text.to_string();
        }

        let mut out = Vec::new();
        let mut current = String::new();

        for word in text.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
                continue;
            }

            let next_len = current.chars().count() + 1 + word.chars().count();
            if next_len <= self.paragraph_width {
                current.push(' ');
                current.push_str(word);
            } else {
                out.push(current);
                current = word.to_string();
            }
        }

        if !current.is_empty() {
            out.push(current);
        }

        out.join("\n")
    }
}
