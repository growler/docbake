use std::fmt::Write as _;

use anyhow::{bail, Result};
use rushdown::as_extension_data;
use rushdown::ast::{Arena, NodeRef};
use rushdown_footnote::FootnoteDefinition;

use super::source_recovery::footnote_has_inline_body;
use super::{FootnoteLabels, MarkdownFmtRenderer};
use crate::table_source::source_tail_at_pos;

impl MarkdownFmtRenderer {
    pub(super) fn render_footnote_definition(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
        footnote_labels: &FootnoteLabels,
        heading_shift: usize,
    ) -> Result<()> {
        let _footnote = as_extension_data!(arena, node_ref, FootnoteDefinition);
        let label = footnote_labels
            .iter()
            .find_map(|(nr, label)| (*nr == node_ref).then_some(label.as_str()))
            .unwrap_or("footnote");
        let inline_first_line = footnote_has_inline_body(arena, node_ref, source).unwrap_or(true);
        let mut body = String::new();
        for child in arena[node_ref].children(arena) {
            self.render_block(
                &mut body,
                arena,
                child,
                source,
                footnote_labels,
                heading_shift,
            )?;
        }
        let trimmed = body.trim_end();

        if !inline_first_line {
            writeln!(out, "[^{label}]:")?;
            for line in trimmed.lines() {
                if line.is_empty() {
                    out.push('\n');
                } else {
                    writeln!(out, "    {line}")?;
                }
            }
            out.push('\n');
            return Ok(());
        }

        let mut first_line = true;
        for line in trimmed.lines() {
            if first_line {
                writeln!(out, "[^{label}]: {line}")?;
                first_line = false;
            } else if line.is_empty() {
                out.push('\n');
            } else {
                writeln!(out, "    {line}")?;
            }
        }
        if first_line {
            writeln!(out, "[^{label}]:")?;
        }
        out.push('\n');
        Ok(())
    }
}

pub(super) fn collect_footnote_labels(
    arena: &Arena,
    doc_ref: NodeRef,
    source: &str,
) -> Result<FootnoteLabels> {
    let mut nodes = Vec::new();
    collect_footnote_nodes(arena, doc_ref, &mut nodes);

    let mut map = FootnoteLabels::new();
    for node_ref in nodes {
        let Some(label) = footnote_label_from_source_at_node(arena, node_ref, source) else {
            let pos = arena[node_ref].pos().unwrap_or_default();
            bail!("failed to read footnote label from source at byte offset {pos}");
        };
        map.push((node_ref, label));
    }
    Ok(map)
}

fn footnote_label_from_source_at_node(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Option<String> {
    let pos = arena[node_ref].pos()?;
    let tail = source_tail_at_pos(source, pos)?;
    parse_footnote_label_from_line(tail)
}

fn parse_footnote_label_from_line(line: &str) -> Option<String> {
    let line = line.trim_start_matches([' ', '\t']);
    let rest = line.strip_prefix("[^")?;
    let end = rest.find(']')?;
    if end == 0 {
        return None;
    }
    let label = &rest[..end];
    let tail = rest[end + 1..].trim_start_matches([' ', '\t']);
    if !tail.starts_with(':') {
        return None;
    }
    Some(label.to_string())
}

fn collect_footnote_nodes(arena: &Arena, node_ref: NodeRef, out: &mut Vec<NodeRef>) {
    if rushdown::matches_extension_kind!(arena, node_ref, FootnoteDefinition) {
        out.push(node_ref);
        return;
    }
    for child in arena[node_ref].children(arena) {
        collect_footnote_nodes(arena, child, out);
    }
}
