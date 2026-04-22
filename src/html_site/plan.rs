use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::graph::resolve::heading_text;
use crate::graph::{DocumentGraph, PreparedDocument};

/// Information about a single page in the multi-page output.
#[derive(Debug, Clone)]
pub(crate) struct PageInfo {
    /// Document occurrence ID in the graph.
    pub doc_id: usize,
    /// Output path relative to the document output root (e.g. `builder/index.html`).
    pub output_path: PathBuf,
    /// Human-readable title (first heading text of the page).
    pub title: String,
    /// Index of the parent page in `MultiPagePlan::pages`, or `None` for root.
    pub parent_index: Option<usize>,
    /// Indices of direct child pages.
    pub children_indices: Vec<usize>,
    /// Index of the previous page in DFS order.
    pub prev_index: Option<usize>,
    /// Index of the next page in DFS order.
    pub next_index: Option<usize>,
}

/// Pre-computed plan for all pages.
#[derive(Debug)]
pub(crate) struct MultiPagePlan {
    pub pages: Vec<PageInfo>,
    /// Map from doc_id -> page index for quick lookup.
    doc_id_to_page: HashMap<usize, usize>,
}

impl MultiPagePlan {
    /// Look up the [`PageInfo`] for a given document occurrence ID.
    pub(crate) fn page_for_doc(&self, doc_id: usize) -> Option<&PageInfo> {
        self.doc_id_to_page
            .get(&doc_id)
            .map(|&idx| &self.pages[idx])
    }

    pub(super) fn page_index_for_doc(&self, doc_id: usize) -> Option<usize> {
        self.doc_id_to_page.get(&doc_id).copied()
    }
}

/// Build the multi-page plan from a prepared document.
///
/// The page tree mirrors the single-page heading hierarchy: the cumulative
/// heading shift of each include determines its depth.  A page with shift 0
/// is a top-level peer; a page with shift N nests under the most recent page
/// with shift < N, exactly like headings nest in the single-page TOC.
pub(crate) fn build_multi_page_plan(
    prepared: &PreparedDocument,
    root_source_dir: &Path,
    base_dir: &Path,
) -> Result<MultiPagePlan> {
    let graph = prepared.graph();
    let dfs_order = graph.page_order_with_shift();

    // Build flat page list (tree links filled in the second pass).
    let mut pages: Vec<PageInfo> = dfs_order
        .iter()
        .enumerate()
        .map(|(i, &(doc_id, _shift))| -> Result<PageInfo> {
            let doc = graph.document(doc_id);
            let output_path = compute_output_path(&doc.path, root_source_dir, base_dir)?;
            let title = graph
                .first_heading_id(doc_id)
                .and_then(|_| first_heading_text(graph, doc_id))
                .unwrap_or_else(|| {
                    output_path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default()
                });
            let prev_index = if i > 0 { Some(i - 1) } else { None };
            let next_index = if i + 1 < dfs_order.len() {
                Some(i + 1)
            } else {
                None
            };

            Ok(PageInfo {
                doc_id,
                output_path,
                title,
                parent_index: None,
                children_indices: Vec::new(),
                prev_index,
                next_index,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    // Build the tree using a stack of (page_index, effective_level).
    // A page at shift N nests under the most recent page at shift < N,
    // replicating the single-page heading nesting algorithm.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for (i, &(_doc_id, shift)) in dfs_order.iter().enumerate() {
        while let Some(&(_, top_shift)) = stack.last() {
            if top_shift < shift {
                break;
            }
            stack.pop();
        }
        if let Some(&(parent_idx, _)) = stack.last() {
            pages[i].parent_index = Some(parent_idx);
            pages[parent_idx].children_indices.push(i);
        }
        stack.push((i, shift));
    }

    let mut doc_id_to_page = HashMap::new();
    for (i, page) in pages.iter().enumerate() {
        doc_id_to_page.insert(page.doc_id, i);
    }

    Ok(MultiPagePlan {
        pages,
        doc_id_to_page,
    })
}

/// Compute the output `.html` path for a document relative to the root
/// source directory.
fn compute_output_path(
    doc_path: &Path,
    root_source_dir: &Path,
    base_dir: &Path,
) -> Result<PathBuf> {
    if let Ok(relative) = doc_path.strip_prefix(root_source_dir) {
        return Ok(relative.with_extension("html"));
    }

    let relative = doc_path.strip_prefix(base_dir).with_context(|| {
        format!(
            "document {} is outside both root source directory {} and base directory {}",
            doc_path.display(),
            root_source_dir.display(),
            base_dir.display()
        )
    })?;
    Ok(relative.with_extension("html"))
}

/// Get the plain-text title of a document's first heading.
fn first_heading_text(graph: &DocumentGraph, doc_id: usize) -> Option<String> {
    let doc = graph.document(doc_id);
    for child in doc.arena[doc.doc_ref].children(&doc.arena) {
        if let rushdown::ast::KindData::Heading(_) = doc.arena[child].kind_data() {
            let text = heading_text(&doc.arena, child, &doc.source);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}
