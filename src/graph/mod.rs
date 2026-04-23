pub(crate) mod build;
pub(crate) mod resolve;
pub(crate) mod types;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use rushdown::ast::{KindData, NodeRef};
use serde_json::{Map as JsonMap, Value as JsonValue};

pub(crate) use self::types::{DocumentOccurrence, IncludeEntry, NodeLocation};
use crate::cli::BrokenLinkMode;
use crate::conditional::RenderProfile;
use crate::DebugDumper;

#[derive(Debug, Clone, Copy)]
pub(crate) struct PrepareOptions {
    pub(crate) transform_diagram_images: bool,
}

impl Default for PrepareOptions {
    fn default() -> Self {
        Self {
            transform_diagram_images: true,
        }
    }
}

#[derive(Debug)]
pub(crate) struct DocumentGraph {
    pub(crate) root_id: usize,
    pub(crate) documents: Vec<DocumentOccurrence>,
    pub(super) rendered_nodes: Vec<(usize, NodeRef, usize)>,
    pub(super) occurrences_by_path: HashMap<PathBuf, Vec<usize>>,
    pub(super) first_heading_by_doc: HashMap<usize, String>,
    pub(super) anchor_targets: HashMap<(usize, String), String>,
    pub(super) link_targets: HashMap<NodeLocation, String>,
    pub(super) image_targets: HashMap<NodeLocation, String>,
    pub(super) asset_targets: HashSet<PathBuf>,
}

impl DocumentGraph {
    pub(crate) fn root_document(&self) -> &DocumentOccurrence {
        &self.documents[self.root_id]
    }

    pub(crate) fn document(&self, doc_id: usize) -> &DocumentOccurrence {
        &self.documents[doc_id]
    }

    pub(crate) fn document_mut(&mut self, doc_id: usize) -> &mut DocumentOccurrence {
        &mut self.documents[doc_id]
    }

    pub(crate) fn include_entries(
        &self,
        doc_id: usize,
        node_ref: NodeRef,
    ) -> Option<&[IncludeEntry]> {
        self.document(doc_id).include_entries(node_ref)
    }

    pub(crate) fn link_target(&self, doc_id: usize, node_ref: NodeRef) -> Option<&str> {
        self.link_targets
            .get(&NodeLocation::new(doc_id, node_ref))
            .map(String::as_str)
    }

    pub(crate) fn image_target(&self, doc_id: usize, node_ref: NodeRef) -> Option<&str> {
        self.image_targets
            .get(&NodeLocation::new(doc_id, node_ref))
            .map(String::as_str)
    }

    pub(crate) fn asset_targets(&self) -> &HashSet<PathBuf> {
        &self.asset_targets
    }

    pub(crate) fn first_heading_id(&self, doc_id: usize) -> Option<&str> {
        self.first_heading_by_doc.get(&doc_id).map(String::as_str)
    }

    pub(crate) fn anchor_target(&self, doc_id: usize, fragment: &str) -> Option<&str> {
        self.anchor_targets
            .get(&(doc_id, fragment.to_string()))
            .map(String::as_str)
    }

    pub(crate) fn resolve_markdown_target(
        &self,
        source_doc_id: usize,
        target_path: &Path,
    ) -> Result<usize> {
        if self.document(source_doc_id).path == target_path {
            return Ok(source_doc_id);
        }

        let matches = self.occurrences_by_path.get(target_path).ok_or_else(|| {
            anyhow!(
                "markdown link target {} is not included in the final document",
                target_path.display()
            )
        })?;
        match matches.as_slice() {
            [doc_id] => Ok(*doc_id),
            [] => bail!(
                "markdown link target {} is not included in the final document",
                target_path.display()
            ),
            _ => bail!(
                "markdown link target {} is ambiguous because it is included multiple times",
                target_path.display()
            ),
        }
    }

    pub(crate) fn rendered_nodes_preorder(
        &self,
    ) -> impl Iterator<Item = (usize, NodeRef, usize)> + '_ {
        self.rendered_nodes.iter().copied()
    }

    pub(crate) fn rendered_nodes_preorder_visible(
        &self,
        profile: &RenderProfile,
    ) -> Result<Vec<(usize, NodeRef, usize)>> {
        let mut nodes = Vec::new();
        self.collect_document_children_visible(self.root_id, 0, profile, &mut nodes)?;
        Ok(nodes)
    }

    /// Return all document IDs in DFS pre-order together with their
    /// cumulative heading shift (the sum of `heading_shift` values along
    /// the include path from the root).  This mirrors the effective heading
    /// level each page would have in the single-page flattened document.
    pub(crate) fn page_order_with_shift(&self) -> Vec<(usize, usize)> {
        let mut order = Vec::new();
        self.collect_page_order_with_shift(self.root_id, 0, &mut order);
        order
    }

    fn collect_page_order_with_shift(
        &self,
        doc_id: usize,
        cumulative_shift: usize,
        out: &mut Vec<(usize, usize)>,
    ) {
        out.push((doc_id, cumulative_shift));
        let doc = self.document(doc_id);
        for (_node_ref, entries) in &doc.includes {
            for entry in entries {
                self.collect_page_order_with_shift(
                    entry.doc_id,
                    cumulative_shift + entry.heading_shift,
                    out,
                );
            }
        }
    }

    pub(super) fn build_rendered_nodes(&mut self) {
        let mut nodes = Vec::new();
        self.collect_document_children(self.root_id, 0, &mut nodes);
        self.rendered_nodes = nodes;
    }

    pub(super) fn build_occurrence_index(&mut self) {
        self.occurrences_by_path.clear();
        for doc in &self.documents {
            self.occurrences_by_path
                .entry(doc.path.clone())
                .or_default()
                .push(doc.id);
        }
    }

    fn collect_document_children(
        &self,
        doc_id: usize,
        heading_shift: usize,
        out: &mut Vec<(usize, NodeRef, usize)>,
    ) {
        let doc = self.document(doc_id);
        for child in doc.arena[doc.doc_ref].children(&doc.arena) {
            self.collect_rendered_node(doc_id, child, heading_shift, out);
        }
    }

    fn collect_document_children_visible(
        &self,
        doc_id: usize,
        heading_shift: usize,
        profile: &RenderProfile,
        out: &mut Vec<(usize, NodeRef, usize)>,
    ) -> Result<()> {
        let doc = self.document(doc_id);
        for child in doc.arena[doc.doc_ref].children(&doc.arena) {
            self.collect_rendered_node_visible(doc_id, child, heading_shift, profile, out)?;
        }
        Ok(())
    }

    fn collect_rendered_node(
        &self,
        doc_id: usize,
        node_ref: NodeRef,
        heading_shift: usize,
        out: &mut Vec<(usize, NodeRef, usize)>,
    ) {
        if let Some(entries) = self.include_entries(doc_id, node_ref) {
            for entry in entries {
                self.collect_document_children(
                    entry.doc_id,
                    heading_shift + entry.heading_shift,
                    out,
                );
            }
            return;
        }

        out.push((doc_id, node_ref, heading_shift));
        let doc = self.document(doc_id);
        for child in doc.arena[node_ref].children(&doc.arena) {
            self.collect_rendered_node(doc_id, child, heading_shift, out);
        }
    }

    fn collect_rendered_node_visible(
        &self,
        doc_id: usize,
        node_ref: NodeRef,
        heading_shift: usize,
        profile: &RenderProfile,
        out: &mut Vec<(usize, NodeRef, usize)>,
    ) -> Result<()> {
        let doc = self.document(doc_id);
        if !crate::conditional::node_is_visible(&doc.arena, node_ref, &doc.source, profile)? {
            return Ok(());
        }

        if let Some(entries) = self.include_entries(doc_id, node_ref) {
            for entry in entries {
                self.collect_document_children_visible(
                    entry.doc_id,
                    heading_shift + entry.heading_shift,
                    profile,
                    out,
                )?;
            }
            return Ok(());
        }

        out.push((doc_id, node_ref, heading_shift));
        for child in doc.arena[node_ref].children(&doc.arena) {
            self.collect_rendered_node_visible(doc_id, child, heading_shift, profile, out)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct PreparedDocument {
    graph: DocumentGraph,
}

impl PreparedDocument {
    pub(crate) fn graph(&self) -> &DocumentGraph {
        &self.graph
    }

    pub(crate) fn graph_mut(&mut self) -> &mut DocumentGraph {
        &mut self.graph
    }

    pub(crate) fn asset_targets(&self) -> &HashSet<PathBuf> {
        self.graph.asset_targets()
    }

    pub(crate) fn document_metadata(&self) -> JsonMap<String, JsonValue> {
        self.graph.root_document().document_metadata()
    }

    pub(crate) fn dump_graph(&self) -> String {
        let mut out = String::new();
        for doc in &self.graph.documents {
            out.push_str(&format!("Document[{}] {}\n", doc.id, doc.path.display()));

            let mut includes = doc.includes.iter().collect::<Vec<_>>();
            includes.sort_by_key(|(node_ref, _)| node_ref.to_string());
            if includes.is_empty() {
                out.push_str("  includes: none\n");
            } else {
                out.push_str("  includes:\n");
                for (node_ref, entries) in includes {
                    for entry in entries {
                        out.push_str(&format!(
                            "    {} -> Document[{}] shift={} target={}\n",
                            node_ref,
                            entry.doc_id,
                            entry.heading_shift,
                            entry.target_path.display()
                        ));
                    }
                }
            }

            let headings = self
                .graph
                .rendered_nodes_preorder()
                .filter(|(rendered_doc_id, node_ref, _)| {
                    *rendered_doc_id == doc.id
                        && matches!(doc.arena[*node_ref].kind_data(), KindData::Heading(_))
                })
                .map(|(_, node_ref, shift)| {
                    let id = doc.arena[node_ref]
                        .attributes()
                        .get("id")
                        .map(|value| value.str(&doc.source).to_string())
                        .unwrap_or_default();
                    format!(
                        "    {} shift={} id={} text={}",
                        node_ref,
                        shift,
                        id,
                        resolve::heading_text(&doc.arena, node_ref, &doc.source)
                    )
                })
                .collect::<Vec<_>>();
            if headings.is_empty() {
                out.push_str("  headings: none\n");
            } else {
                out.push_str("  headings:\n");
                for heading in headings {
                    out.push_str(&heading);
                    out.push('\n');
                }
            }

            let mut rewrites = self
                .graph
                .link_targets
                .iter()
                .filter(|(location, _)| location.doc_id == doc.id)
                .map(|(location, target)| format!("    {} => {}", location.node, target))
                .collect::<Vec<_>>();
            rewrites.sort();
            let mut image_rewrites = self
                .graph
                .image_targets
                .iter()
                .filter(|(location, _)| location.doc_id == doc.id)
                .map(|(location, target)| format!("    {} => {}", location.node, target))
                .collect::<Vec<_>>();
            image_rewrites.sort();
            if rewrites.is_empty() && image_rewrites.is_empty() {
                out.push_str("  rewrites: none\n");
            } else {
                out.push_str("  rewrites:\n");
                for rewrite in rewrites.into_iter().chain(image_rewrites) {
                    out.push_str(&rewrite);
                    out.push('\n');
                }
            }

            out.push('\n');
        }
        out
    }

    pub(crate) fn parse_dump(&self) -> Result<String> {
        use rushdown::ast;

        let mut out = String::new();
        for doc in &self.graph.documents {
            out.push_str(&format!(
                "== Document[{}] {} ==\n",
                doc.id,
                doc.path.display()
            ));
            ast::pretty_print(&mut out, &doc.arena, doc.doc_ref, &doc.source)
                .context("failed to pretty-print AST")?;
            out.push('\n');
        }
        Ok(out)
    }
}

pub(crate) fn prepare_document(
    logger: &DebugDumper,
    base_dir: &Path,
    source_path: &Path,
    document_name: Option<&str>,
    broken_links: BrokenLinkMode,
) -> Result<PreparedDocument> {
    prepare_document_with_options(
        logger,
        base_dir,
        source_path,
        document_name,
        broken_links,
        PrepareOptions {
            transform_diagram_images: true,
        },
    )
}

pub(crate) fn prepare_document_with_options(
    logger: &DebugDumper,
    base_dir: &Path,
    source_path: &Path,
    document_name: Option<&str>,
    broken_links: BrokenLinkMode,
    options: PrepareOptions,
) -> Result<PreparedDocument> {
    let canonical_root = base_dir
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", base_dir.display()))?;
    let mut graph = if options.transform_diagram_images {
        build::parse_document_graph(&canonical_root, logger, source_path)?
    } else {
        build::parse_document_graph_with_options(
            &canonical_root,
            logger,
            source_path,
            build::ParseOptions {
                transform_diagram_images: false,
            },
        )?
    };
    let diagnostics = resolve::resolve_document_graph(&canonical_root, &mut graph)?;

    if !diagnostics.is_empty() {
        for d in &diagnostics {
            eprintln!("{}", d.display_relative(&canonical_root));
        }
        if broken_links == BrokenLinkMode::Error {
            let mut msg = String::new();
            for d in &diagnostics {
                msg.push_str(&d.display_relative(&canonical_root));
                msg.push('\n');
            }
            msg.push_str(&format!(
                "aborting due to {} broken link{}",
                diagnostics.len(),
                if diagnostics.len() == 1 { "" } else { "s" }
            ));
            bail!("{msg}");
        }
    }

    if let Some(name) = document_name {
        logger.log(format!("prepared document `{name}`"));
    }
    Ok(PreparedDocument { graph })
}

#[cfg(test)]
mod tests;
