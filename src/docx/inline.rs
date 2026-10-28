use docx_rs::*;
use rushdown::ast::{Arena, KindData, NodeRef, TextQualifier};

use super::{DocxRenderer, RenderContext};

impl<'a> DocxRenderer<'a> {
    /// Render an inline node to one or more `Run`s.
    pub(super) fn render_inline_runs(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Vec<Run> {
        let arena = context.arena;
        let source = context.source;
        match arena[node_ref].kind_data() {
            KindData::Text(text) => {
                let content = text.str(source);
                let mut runs = vec![Run::new().add_text(content)];
                if text.has_qualifiers(TextQualifier::HARD_LINE_BREAK) {
                    runs.push(Run::new().add_break(BreakType::TextWrapping));
                } else if text.has_qualifiers(TextQualifier::SOFT_LINE_BREAK) {
                    runs.push(Run::new().add_text(" "));
                }
                runs
            }
            KindData::CodeSpan(cs) => {
                let content = cs.str(source);
                vec![Run::new()
                    .add_text(content.into_owned())
                    .style("InlineCode")]
            }
            KindData::Emphasis(_) => {
                let children_runs = self.collect_child_runs(context, node_ref);
                children_runs.into_iter().map(|r| r.italic()).collect()
            }
            KindData::Strong(_) => {
                let children_runs = self.collect_child_runs(context, node_ref);
                children_runs.into_iter().map(|r| r.bold()).collect()
            }
            KindData::Strikethrough(_) => {
                let children_runs = self.collect_child_runs(context, node_ref);
                children_runs.into_iter().map(|r| r.strike()).collect()
            }
            KindData::Link(_) => {
                let text = Self::collect_inline_text(arena, source, node_ref);
                vec![Run::new().add_text(text).style("Hyperlink")]
            }
            KindData::Image(_) => {
                if let Some(pic) = self.resolve_graph_image(context, node_ref) {
                    vec![Run::new().add_image(pic)]
                } else {
                    let alt = Self::collect_inline_text(arena, source, node_ref);
                    if alt.is_empty() {
                        vec![]
                    } else {
                        vec![Run::new().add_text(&alt)]
                    }
                }
            }
            _ => self.collect_child_runs(context, node_ref),
        }
    }

    /// Collect `Run`s from all children of a node.
    fn collect_child_runs(&self, context: &RenderContext<'_>, node_ref: NodeRef) -> Vec<Run> {
        let arena = context.arena;
        let mut runs = Vec::new();
        for child in arena[node_ref].children(arena) {
            runs.extend(self.render_inline_runs(context, child));
        }
        runs
    }

    /// Recursively collect plain text from an inline subtree.
    pub(super) fn collect_inline_text(arena: &Arena, source: &str, node_ref: NodeRef) -> String {
        let mut buf = String::new();
        for child in arena[node_ref].children(arena) {
            match arena[child].kind_data() {
                KindData::Text(text) => {
                    buf.push_str(text.str(source));
                    if text.has_qualifiers(TextQualifier::SOFT_LINE_BREAK) {
                        buf.push(' ');
                    } else if text.has_qualifiers(TextQualifier::HARD_LINE_BREAK) {
                        buf.push('\n');
                    }
                }
                KindData::CodeSpan(cs) => {
                    buf.push_str(&cs.str(source));
                }
                _ => {
                    buf.push_str(&Self::collect_inline_text(arena, source, child));
                }
            }
        }
        buf
    }
}
