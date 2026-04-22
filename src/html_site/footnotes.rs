use std::collections::HashMap;

use anyhow::{bail, Result};
use rushdown::as_extension_data;
use rushdown::ast::{Arena, NodeRef};
use rushdown_footnote::FootnoteReference;

use crate::graph::{DocumentGraph, NodeLocation};

#[derive(Clone)]
pub(super) struct FootnoteEntry {
    pub label: String,
    pub index: usize,
    pub reference_indices: Vec<usize>,
}

#[derive(Clone, Default)]
pub(super) struct FootnotePlan {
    pub refs: HashMap<NodeLocation, (usize, usize)>,
    pub ordered: Vec<FootnoteEntry>,
}

pub(super) fn build_page_footnote_plan(
    graph: &DocumentGraph,
    doc_id: usize,
) -> Result<FootnotePlan> {
    let doc = graph.document(doc_id);
    let mut refs = HashMap::new();
    let mut ordered = Vec::<FootnoteEntry>::new();
    let mut by_label = HashMap::<String, usize>::new();
    let mut next_ref_index = 1usize;

    collect_footnote_refs(
        &doc.arena,
        doc.doc_ref,
        graph,
        doc_id,
        &mut refs,
        &mut ordered,
        &mut by_label,
        &mut next_ref_index,
    )?;

    Ok(FootnotePlan { refs, ordered })
}

#[allow(clippy::too_many_arguments)]
fn collect_footnote_refs(
    arena: &Arena,
    node_ref: NodeRef,
    graph: &DocumentGraph,
    doc_id: usize,
    refs: &mut HashMap<NodeLocation, (usize, usize)>,
    ordered: &mut Vec<FootnoteEntry>,
    by_label: &mut HashMap<String, usize>,
    next_ref_index: &mut usize,
) -> Result<()> {
    // Skip include blocks.
    if graph.include_entries(doc_id, node_ref).is_some() {
        return Ok(());
    }

    let doc = graph.document(doc_id);
    if rushdown::matches_extension_kind!(arena, node_ref, FootnoteReference) {
        let footnote = as_extension_data!(arena, node_ref, FootnoteReference);
        let label = footnote.label().str(&doc.source).to_string();
        if doc.footnote_definition(&label).is_none() {
            bail!(
                "missing footnote definition for `{label}` in {}",
                doc.path.display()
            );
        }
        let index = if let Some(&idx) = by_label.get(&label) {
            idx
        } else {
            let index = ordered.len() + 1;
            ordered.push(FootnoteEntry {
                label: label.clone(),
                index,
                reference_indices: Vec::new(),
            });
            by_label.insert(label, index);
            index
        };
        let ref_index = *next_ref_index;
        *next_ref_index += 1;
        ordered[index - 1].reference_indices.push(ref_index);
        refs.insert(NodeLocation::new(doc_id, node_ref), (index, ref_index));
        return Ok(());
    }

    for child in arena[node_ref].children(arena) {
        collect_footnote_refs(
            arena,
            child,
            graph,
            doc_id,
            refs,
            ordered,
            by_label,
            next_ref_index,
        )?;
    }
    Ok(())
}
