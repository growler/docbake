use anyhow::Result;
use docx_rs::*;
use rushdown::ast::{Arena, KindData, NodeRef, TextQualifier};
use rushdown_footnote::FootnoteReference;

use super::{DocxRenderer, RenderContext};

impl<'a> DocxRenderer<'a> {
    /// Render an inline node to one or more `Run`s. In a footnote, an
    /// image gives its alt text.
    pub(super) fn render_inline_runs(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Vec<Run>> {
        let arena = context.arena;
        let source = context.source;
        let runs = match arena[node_ref].kind_data() {
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
                let children_runs = self.collect_child_runs(context, node_ref)?;
                children_runs.into_iter().map(|r| r.italic()).collect()
            }
            KindData::Strong(_) => {
                let children_runs = self.collect_child_runs(context, node_ref)?;
                children_runs.into_iter().map(|r| r.bold()).collect()
            }
            KindData::Strikethrough(_) => {
                let children_runs = self.collect_child_runs(context, node_ref)?;
                children_runs.into_iter().map(|r| r.strike()).collect()
            }
            KindData::Link(_) => self.collect_text_runs(context, node_ref, Some("Hyperlink"))?,
            KindData::Image(_) => {
                let pic = if context.footnote {
                    None
                } else {
                    self.resolve_graph_image(context, node_ref)?
                };
                if let Some(pic) = pic {
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
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FootnoteReference) =>
            {
                self.render_footnote_reference(context, node_ref)?
            }
            _ => self.collect_child_runs(context, node_ref)?,
        };
        Ok(runs)
    }

    /// Collect `Run`s from all children of a node.
    pub(super) fn collect_child_runs(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Vec<Run>> {
        let arena = context.arena;
        let mut runs = Vec::new();
        for child in arena[node_ref].children(arena) {
            runs.extend(self.render_inline_runs(context, child)?);
        }
        Ok(runs)
    }

    /// Collect the plain text of an inline subtree as runs with the
    /// character style `style`. A footnote reference gives its reference
    /// run between the text runs. The text without references is one run.
    pub(super) fn collect_text_runs(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
        style: Option<&str>,
    ) -> Result<Vec<Run>> {
        let mut text = String::new();
        let mut runs = Vec::new();
        self.push_text_runs(context, node_ref, style, &mut text, &mut runs)?;
        if !text.is_empty() || runs.is_empty() {
            runs.push(text_run(text, style));
        }
        Ok(runs)
    }

    /// Add the plain text of an inline subtree to `text`. At a footnote
    /// reference, move `text` to a run in `runs`, then add the reference
    /// run.
    fn push_text_runs(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
        style: Option<&str>,
        text: &mut String,
        runs: &mut Vec<Run>,
    ) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        for child in arena[node_ref].children(arena) {
            match arena[child].kind_data() {
                KindData::Text(t) => {
                    text.push_str(t.str(source));
                    if t.has_qualifiers(TextQualifier::SOFT_LINE_BREAK) {
                        text.push(' ');
                    } else if t.has_qualifiers(TextQualifier::HARD_LINE_BREAK) {
                        text.push('\n');
                    }
                }
                KindData::CodeSpan(cs) => {
                    text.push_str(&cs.str(source));
                }
                KindData::Extension(_)
                    if rushdown::matches_extension_kind!(arena, child, FootnoteReference) =>
                {
                    let reference = self.render_footnote_reference(context, child)?;
                    if !reference.is_empty() {
                        if !text.is_empty() {
                            runs.push(text_run(std::mem::take(text), style));
                        }
                        runs.extend(reference);
                    }
                }
                _ => self.push_text_runs(context, child, style, text, runs)?,
            }
        }
        Ok(())
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

/// A run with the text `text` and the character style `style`.
fn text_run(text: String, style: Option<&str>) -> Run {
    let run = Run::new().add_text(text);
    match style {
        Some(style) => run.style(style),
        None => run,
    }
}
