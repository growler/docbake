mod blocks;
mod core;
mod footnotes;
mod inline;
mod source_recovery;
mod tables;

use rushdown::ast::NodeRef;

type FootnoteLabels = Vec<(NodeRef, String)>;

pub(crate) const HARD_BREAK_SPACES_MARKER: &str = "@@FMT_HARD_BREAK_SPACE@@";

pub(crate) struct MarkdownFmtRenderer {
    paragraph_width: usize,
    align_tables: bool,
}
