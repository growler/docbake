use anyhow::Result;
use docx_rs::*;
use rushdown::ast::{Arena, KindData, NodeRef, TextQualifier};
use rushdown_footnote::FootnoteReference;

use super::{DocxRenderer, RenderContext};

impl<'a> DocxRenderer<'a> {
    /// Render an inline node to paragraph children: runs and links. In a
    /// footnote, an image gives its alt text.
    pub(super) fn render_inline_runs(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Vec<ParagraphChild>> {
        let arena = context.arena;
        let source = context.source;
        let runs = match arena[node_ref].kind_data() {
            KindData::Text(text) => {
                let content = text.str(source);
                let mut runs = vec![run_child(Run::new().add_text(content))];
                if text.has_qualifiers(TextQualifier::HARD_LINE_BREAK) {
                    runs.push(run_child(Run::new().add_break(BreakType::TextWrapping)));
                } else if text.has_qualifiers(TextQualifier::SOFT_LINE_BREAK) {
                    runs.push(run_child(Run::new().add_text(" ")));
                }
                runs
            }
            KindData::CodeSpan(cs) => {
                let content = cs.str(source);
                vec![run_child(
                    Run::new()
                        .add_text(content.into_owned())
                        .style("InlineCode"),
                )]
            }
            KindData::Emphasis(_) => {
                let mut runs = self.collect_child_runs(context, node_ref)?;
                update_runs(&mut runs, RunProperty::italic);
                runs
            }
            KindData::Strong(_) => {
                let mut runs = self.collect_child_runs(context, node_ref)?;
                update_runs(&mut runs, RunProperty::bold);
                runs
            }
            KindData::Strikethrough(_) => {
                let mut runs = self.collect_child_runs(context, node_ref)?;
                update_runs(&mut runs, RunProperty::strike);
                runs
            }
            KindData::Link(_) => self.render_link(context, node_ref)?,
            KindData::Image(_) => {
                let pic = if context.footnote {
                    None
                } else {
                    self.resolve_graph_image(context, node_ref)?
                };
                if let Some(pic) = pic {
                    vec![run_child(Run::new().add_image(pic))]
                } else {
                    let alt = Self::collect_inline_text(arena, source, node_ref);
                    if alt.is_empty() {
                        vec![]
                    } else {
                        vec![run_child(Run::new().add_text(&alt))]
                    }
                }
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FootnoteReference) =>
            {
                self.render_footnote_reference(context, node_ref)?
                    .into_iter()
                    .map(run_child)
                    .collect()
            }
            _ => self.collect_child_runs(context, node_ref)?,
        };
        Ok(runs)
    }

    /// Collect the paragraph children of all children of a node.
    pub(super) fn collect_child_runs(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Vec<ParagraphChild>> {
        let arena = context.arena;
        let mut runs = Vec::new();
        for child in arena[node_ref].children(arena) {
            runs.extend(self.render_inline_runs(context, child)?);
        }
        Ok(runs)
    }

    /// Collect the plain text of an inline subtree as runs with the
    /// character style `style`. A footnote reference gives its reference
    /// run between the text runs. A link gives its plain text in a
    /// `w:hyperlink`. The text without references and links is one run.
    pub(super) fn collect_text_runs(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
        style: Option<&str>,
    ) -> Result<Vec<ParagraphChild>> {
        let mut text = String::new();
        let mut runs = Vec::new();
        self.push_text_runs(context, node_ref, style, &mut text, &mut runs)?;
        if !text.is_empty() || runs.is_empty() {
            runs.push(run_child(text_run(text, style)));
        }
        Ok(runs)
    }

    /// Add the plain text of an inline subtree to `text`. At a footnote
    /// reference or a link, move `text` to a run in `runs`, then add the
    /// reference run or the link.
    fn push_text_runs(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
        style: Option<&str>,
        text: &mut String,
        runs: &mut Vec<ParagraphChild>,
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
                            runs.push(run_child(text_run(std::mem::take(text), style)));
                        }
                        runs.extend(reference.into_iter().map(run_child));
                    }
                }
                KindData::Link(_) => {
                    if !text.is_empty() {
                        runs.push(run_child(text_run(std::mem::take(text), style)));
                    }
                    let mut link_text = String::new();
                    let mut link_runs = Vec::new();
                    self.push_text_runs(context, child, style, &mut link_text, &mut link_runs)?;
                    if !link_text.is_empty() {
                        link_runs.push(run_child(text_run(link_text, style)));
                    }
                    runs.extend(self.render_link_with(context, child, link_runs));
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

/// Gives `run` as a paragraph child.
pub(super) fn run_child(run: Run) -> ParagraphChild {
    ParagraphChild::Run(Box::new(run))
}

/// Changes the run properties of each run in `children` with `f`, also in
/// links.
fn update_runs(children: &mut [ParagraphChild], f: fn(RunProperty) -> RunProperty) {
    for child in children {
        match child {
            ParagraphChild::Run(run) => run.run_property = f(std::mem::take(&mut run.run_property)),
            ParagraphChild::Hyperlink(link) => update_runs(&mut link.children, f),
            _ => {}
        }
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
