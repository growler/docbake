mod blocks;
mod core;
mod footnotes;
mod inline;
mod source_recovery;
mod tables;

use rushdown::ast::NodeRef;

use crate::conditional::RenderProfile;

type FootnoteLabels = Vec<(NodeRef, String)>;

pub(crate) const HARD_BREAK_SPACES_MARKER: &str = "@@FMT_HARD_BREAK_SPACE@@";
pub(crate) const PRESERVE_BLANK_RUN_MARKER: &str = "@@FMT_PRESERVE_BLANK_RUN@@";

pub(crate) struct MarkdownFmtRenderer {
    paragraph_width: usize,
    align_tables: bool,
    graph_profile: Option<RenderProfile>,
    suppress_link_reference_definitions: bool,
    suppress_footnote_definitions: bool,
    suppress_include_fenced_divs: bool,
}
