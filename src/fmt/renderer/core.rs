use std::collections::HashSet;
use std::fmt::Write as _;

use anyhow::Result;
use rushdown::as_extension_data;
use rushdown::ast::{Arena, NodeRef};
use rushdown_footnote::FootnoteReference;

use super::footnotes::collect_footnote_labels;
use super::{FootnoteLabels, MarkdownFmtRenderer};
use crate::conditional::RenderProfile;
use crate::graph::PreparedDocument;

#[derive(Clone)]
struct GraphFootnoteEntry {
    label: String,
    doc_id: usize,
    node_ref: NodeRef,
}

impl MarkdownFmtRenderer {
    pub(crate) fn new(paragraph_width: usize, align_tables: bool) -> Self {
        Self {
            paragraph_width,
            align_tables,
            graph_profile: None,
            suppress_link_reference_definitions: false,
            suppress_footnote_definitions: false,
            suppress_include_fenced_divs: false,
        }
    }

    pub(crate) fn for_graph(&self, profile: RenderProfile) -> Self {
        Self {
            paragraph_width: self.paragraph_width,
            align_tables: self.align_tables,
            graph_profile: Some(profile),
            suppress_link_reference_definitions: true,
            suppress_footnote_definitions: true,
            suppress_include_fenced_divs: true,
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
            self.render_block(&mut out, arena, child, source, &footnote_labels, 0)?;
        }
        Ok(out)
    }

    pub(crate) fn render_prepared_document(&self, prepared: &PreparedDocument) -> Result<String> {
        let Some(profile) = self.graph_profile.as_ref() else {
            return Ok(String::new());
        };

        let mut out = String::new();
        let footnote_labels: FootnoteLabels = Vec::new();
        self.render_graph_document(
            &mut out,
            prepared,
            prepared.graph().root_id,
            0,
            &footnote_labels,
            profile,
        )?;

        let footnotes = self.collect_graph_footnotes(prepared, profile)?;
        self.render_graph_footnote_definitions(&mut out, prepared, &footnotes, &footnote_labels)?;
        Ok(out)
    }

    fn render_graph_document(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        doc_id: usize,
        heading_shift: usize,
        footnote_labels: &FootnoteLabels,
        profile: &RenderProfile,
    ) -> Result<()> {
        let doc = prepared.graph().document(doc_id);
        for child in doc.arena[doc.doc_ref].children(&doc.arena) {
            if !crate::conditional::node_is_visible(&doc.arena, child, &doc.source, profile)? {
                continue;
            }
            if let Some(entries) = prepared.graph().include_entries(doc_id, child) {
                for entry in entries {
                    self.render_graph_document(
                        out,
                        prepared,
                        entry.doc_id,
                        heading_shift + entry.heading_shift,
                        footnote_labels,
                        profile,
                    )?;
                }
                continue;
            }
            self.render_block(
                out,
                &doc.arena,
                child,
                &doc.source,
                footnote_labels,
                heading_shift,
            )?;
        }
        Ok(())
    }

    fn collect_graph_footnotes(
        &self,
        prepared: &PreparedDocument,
        profile: &RenderProfile,
    ) -> Result<Vec<GraphFootnoteEntry>> {
        let mut ordered = Vec::new();
        let mut seen = HashSet::<(usize, String)>::new();
        self.collect_graph_footnotes_in_document(
            prepared,
            prepared.graph().root_id,
            profile,
            &mut seen,
            &mut ordered,
        )?;
        Ok(ordered)
    }

    fn collect_graph_footnotes_in_document(
        &self,
        prepared: &PreparedDocument,
        doc_id: usize,
        profile: &RenderProfile,
        seen: &mut HashSet<(usize, String)>,
        ordered: &mut Vec<GraphFootnoteEntry>,
    ) -> Result<()> {
        let doc = prepared.graph().document(doc_id);
        for child in doc.arena[doc.doc_ref].children(&doc.arena) {
            if !crate::conditional::node_is_visible(&doc.arena, child, &doc.source, profile)? {
                continue;
            }
            if let Some(entries) = prepared.graph().include_entries(doc_id, child) {
                for entry in entries {
                    self.collect_graph_footnotes_in_document(
                        prepared,
                        entry.doc_id,
                        profile,
                        seen,
                        ordered,
                    )?;
                }
                continue;
            }
            self.collect_graph_footnotes_in_node(prepared, doc_id, child, profile, seen, ordered)?;
        }
        Ok(())
    }

    #[allow(clippy::only_used_in_recursion)]
    fn collect_graph_footnotes_in_node(
        &self,
        prepared: &PreparedDocument,
        doc_id: usize,
        node_ref: NodeRef,
        profile: &RenderProfile,
        seen: &mut HashSet<(usize, String)>,
        ordered: &mut Vec<GraphFootnoteEntry>,
    ) -> Result<()> {
        let doc = prepared.graph().document(doc_id);
        if !crate::conditional::node_is_visible(&doc.arena, node_ref, &doc.source, profile)? {
            return Ok(());
        }
        if rushdown::matches_extension_kind!(&doc.arena, node_ref, FootnoteReference) {
            let footnote = as_extension_data!(&doc.arena, node_ref, FootnoteReference);
            let label = footnote.label().str(&doc.source).to_string();
            let key = (doc_id, label.clone());
            if seen.insert(key) {
                if let Some(definition) = doc.footnote_definition(&label) {
                    ordered.push(GraphFootnoteEntry {
                        label,
                        doc_id,
                        node_ref: definition.node_ref,
                    });
                }
            }
        }

        for child in doc.arena[node_ref].children(&doc.arena) {
            self.collect_graph_footnotes_in_node(prepared, doc_id, child, profile, seen, ordered)?;
        }
        Ok(())
    }

    fn render_graph_footnote_definitions(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        footnotes: &[GraphFootnoteEntry],
        footnote_labels: &FootnoteLabels,
    ) -> Result<()> {
        if footnotes.is_empty() {
            return Ok(());
        }

        let Some(profile) = self.graph_profile.as_ref() else {
            return Ok(());
        };

        let mut pending = footnotes.to_vec();
        let mut seen: HashSet<(usize, String)> = footnotes
            .iter()
            .map(|entry| (entry.doc_id, entry.label.clone()))
            .collect();

        while !pending.is_empty() {
            let batch = std::mem::take(&mut pending);
            for entry in &batch {
                let doc = prepared.graph().document(entry.doc_id);
                let mut body = String::new();
                for child in doc.arena[entry.node_ref].children(&doc.arena) {
                    self.render_block(
                        &mut body,
                        &doc.arena,
                        child,
                        &doc.source,
                        footnote_labels,
                        0,
                    )?;
                }

                let trimmed = body.trim_end();
                let mut first_line = true;
                for line in trimmed.lines() {
                    if first_line {
                        writeln!(out, "[^{}]: {}", entry.label, line)?;
                        first_line = false;
                    } else if line.is_empty() {
                        out.push('\n');
                    } else {
                        writeln!(out, "    {line}")?;
                    }
                }
                if first_line {
                    writeln!(out, "[^{}]:", entry.label)?;
                }

                for child in doc.arena[entry.node_ref].children(&doc.arena) {
                    self.collect_graph_footnotes_in_node(
                        prepared,
                        entry.doc_id,
                        child,
                        profile,
                        &mut seen,
                        &mut pending,
                    )?;
                }
            }
        }

        Ok(())
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
