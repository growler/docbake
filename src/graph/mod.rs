mod build;
pub(crate) mod resolve;
pub(crate) mod types;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use rushdown::ast::{KindData, NodeRef};
use serde_json::{Map as JsonMap, Value as JsonValue};

pub(crate) use self::types::{DocumentOccurrence, IncludeEntry, NodeLocation};
use crate::cli::BrokenLinkMode;
use crate::DebugDumper;

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

    pub(crate) fn anchor_count(&self) -> usize {
        self.anchor_targets.len()
    }

    pub(crate) fn link_count(&self) -> usize {
        self.link_targets.len()
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
}

#[derive(Debug)]
pub(crate) struct PreparedDocument {
    graph: DocumentGraph,
}

impl PreparedDocument {
    pub(crate) fn graph(&self) -> &DocumentGraph {
        &self.graph
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
    let canonical_root = base_dir
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", base_dir.display()))?;
    let mut graph = build::parse_document_graph(&canonical_root, logger, source_path)?;
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
mod tests {
    use std::fs;
    use std::path::Path;

    use rushdown::ast::KindData;
    use rushdown::text::BasicReader;
    use rushdown_fenced_div::FencedDiv;
    use tempfile::TempDir;

    use super::*;
    use crate::build::markdown_parser;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    #[test]
    fn rewrites_markdown_links_and_assets_across_document_graph() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\n:::include\n# [Part](part.md)\n:::\n",
        );
        write(
            &root.join("doc/part.md"),
            "# Part\n\nSee [root](index.md) and ![Cat](../asset.svg).\n",
        );
        write(&root.join("asset.svg"), "<svg/>");

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();
        let part = root.join("doc/part.md").canonicalize().unwrap();
        let part_doc_id = graph.resolve_markdown_target(graph.root_id, &part).unwrap();

        assert_eq!(graph.first_heading_id(graph.root_id), Some("root"));
        assert_eq!(graph.first_heading_id(part_doc_id), Some("part"));
        assert!(graph.link_targets.values().any(|value| value == "#root"));
        assert!(graph
            .image_targets
            .values()
            .any(|value| value == "static/asset.svg"));
    }

    #[test]
    fn allows_multiple_top_level_include_blocks() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            ":::include\n# [Part](part.md)\n:::\n\n:::include\n# [Other](other.md)\n:::\n",
        );
        write(
            &root.join("doc/part.md"),
            ":::include\n# [Inner](inner.md)\n:::\n",
        );
        write(&root.join("doc/other.md"), "# Other\n");
        write(&root.join("doc/inner.md"), "# Inner\n");

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        assert_eq!(prepared.graph().documents.len(), 4);
    }

    #[test]
    fn rejects_non_top_level_include_blocks() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "> :::include\n> # [Part](part.md)\n> :::\n",
        );
        write(&root.join("doc/part.md"), "# Part\n");

        let error = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap_err();
        assert!(error.to_string().contains("top-level document blocks"));
    }

    #[test]
    fn rewrites_self_links_for_each_include_occurrence() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\n:::include\n# [Part A](part.md)\n# [Part B](part.md)\n:::\n",
        );
        write(
            &root.join("doc/part.md"),
            "# Shared\n\n[self](part.md#shared)\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();
        let part_path = root.join("doc/part.md").canonicalize().unwrap();
        let doc_ids = graph.occurrences_by_path.get(&part_path).unwrap();
        assert_eq!(doc_ids.len(), 2);
        assert_eq!(graph.first_heading_id(doc_ids[0]), Some("shared"));
        assert_eq!(graph.first_heading_id(doc_ids[1]), Some("shared-1"));

        let second_doc = graph.document(doc_ids[1]);
        let second_link = second_doc.arena[second_doc.doc_ref]
            .children(&second_doc.arena)
            .find(|node_ref| {
                matches!(
                    second_doc.arena[*node_ref].kind_data(),
                    KindData::Paragraph(_)
                )
            })
            .and_then(|paragraph_ref| {
                second_doc.arena[paragraph_ref]
                    .children(&second_doc.arena)
                    .find(|node_ref| {
                        matches!(second_doc.arena[*node_ref].kind_data(), KindData::Link(_))
                    })
            })
            .unwrap();
        assert_eq!(
            graph.link_target(doc_ids[1], second_link),
            Some("#shared-1")
        );
    }

    #[test]
    fn rewrites_fragment_links_to_final_heading_ids() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\n:::include\n# [A](a.md)\n# [B](b.md)\n:::\n\n[go](b.md#section)\n",
        );
        write(&root.join("doc/a.md"), "# Section\n");
        write(&root.join("doc/b.md"), "# Section\n");

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        assert!(prepared
            .graph()
            .link_targets
            .values()
            .any(|value| value == "#section-1"));
    }

    #[test]
    fn rejects_ambiguous_markdown_links_for_repeated_includes() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\n:::include\n# [Part A](part.md)\n# [Part B](part.md)\n:::\n\n[go](part.md)\n",
        );
        write(&root.join("doc/part.md"), "# Shared\n");

        let error = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap_err();
        assert!(error.to_string().contains("ambiguous"));
    }

    #[test]
    fn strips_front_matter_with_crlf_line_endings() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\n:::include\n# [Part](part.md)\n:::\n",
        );
        write(
            &root.join("doc/part.md"),
            "---\r\ntitle: Part\r\n---\r\n# Part\r\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let part_path = root.join("doc/part.md").canonicalize().unwrap();
        let part_doc_id = prepared
            .graph()
            .resolve_markdown_target(prepared.graph().root_id, &part_path)
            .unwrap();
        let part_doc = prepared.graph().document(part_doc_id);
        let first_child = part_doc.arena[part_doc.doc_ref]
            .children(&part_doc.arena)
            .next()
            .unwrap();
        assert!(matches!(
            part_doc.arena[first_child].kind_data(),
            KindData::Heading(_)
        ));
    }

    #[test]
    fn renders_typst_footnotes_from_definition_nodes() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\nText[^note]\n\n[^note]: See [root](index.md)\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let rendered = crate::render_typst::TypstRenderer::new(
            root.to_path_buf(),
            Default::default(),
            None,
            None,
            None,
        )
        .render_document(&prepared, &DebugDumper::new(false))
        .unwrap();

        assert!(rendered.contains("#footnote["));
        assert!(rendered.contains("#link(<root>)"));
    }

    #[test]
    fn prepare_document_preserves_definition_lists() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(&root.join("doc/index.md"), "Term\n: Definition\n");

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect::<Vec<_>>();

        assert_eq!(children.len(), 1);
        assert!(rushdown::matches_extension_kind!(
            &root_doc.arena,
            children[0],
            rushdown_definition_list::DefinitionList
        ));

        let list_children = root_doc.arena[children[0]]
            .children(&root_doc.arena)
            .collect::<Vec<_>>();
        assert_eq!(list_children.len(), 2);
        assert!(rushdown::matches_extension_kind!(
            &root_doc.arena,
            list_children[0],
            rushdown_definition_list::Term
        ));
        assert!(rushdown::matches_extension_kind!(
            &root_doc.arena,
            list_children[1],
            rushdown_definition_list::TermDefinition
        ));
    }

    #[test]
    fn renders_typst_definition_lists() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "Alpha\n: First\n\nBeta\n: Second\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let rendered = crate::render_typst::TypstRenderer::new(
            root.to_path_buf(),
            Default::default(),
            None,
            None,
            None,
        )
        .render_document(&prepared, &DebugDumper::new(false))
        .unwrap();

        assert!(rendered.contains("#terms("));
        assert!(rendered.contains("terms.item(["));
        assert!(rendered.contains("Alpha"));
        assert!(rendered.contains("Second"));
    }

    #[test]
    fn parser_recognizes_include_block() {
        let source = ":::include\n# [Part](part.md)\n:::\n";
        let parser = markdown_parser();
        let mut reader = BasicReader::new(source);
        let (arena, doc_ref) = parser.parse(&mut reader);
        let children = arena[doc_ref].children(&arena).collect::<Vec<_>>();
        assert_eq!(children.len(), 1);
        assert!(rushdown::matches_extension_kind!(
            &arena,
            children[0],
            FencedDiv
        ));
        let class = arena[children[0]]
            .attributes()
            .get("class")
            .map(|v| v.str(source))
            .unwrap_or_default();
        assert_eq!(class, "include");
        let include_children = arena[children[0]].children(&arena).collect::<Vec<_>>();
        assert_eq!(include_children.len(), 1);
        assert!(matches!(
            arena[include_children[0]].kind_data(),
            KindData::Heading(_)
        ));
    }

    // -- Caption integration tests --

    #[test]
    fn caption_gets_auto_generated_id() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\nTable: My table\n\n| A | B |\n|---|---|\n| 1 | 2 |\n",
        );
        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        // The caption should have an auto-generated ID in anchor_targets.
        assert!(
            graph.anchor_targets.values().any(|v| v == "my-table"),
            "expected 'my-table' in anchor_targets, got: {:?}",
            graph.anchor_targets
        );
    }

    #[test]
    fn caption_with_explicit_id() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\nTable: My table {#rfc-table}\n\n| A | B |\n|---|---|\n| 1 | 2 |\n",
        );
        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        assert!(
            graph.anchor_targets.values().any(|v| v == "rfc-table"),
            "expected 'rfc-table' in anchor_targets, got: {:?}",
            graph.anchor_targets
        );
    }

    #[test]
    fn caption_id_deduplication_with_headings() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Overview\n\nTable: Overview\n\n| A |\n|---|\n| 1 |\n",
        );
        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        // Both the heading "Overview" and the caption "Overview" exist.
        // The heading gets "overview", the caption should get "overview-1".
        let ids: Vec<&String> = graph.anchor_targets.values().collect();
        assert!(
            ids.contains(&&"overview".to_string()),
            "heading id missing, got: {:?}",
            ids
        );
        assert!(
            ids.contains(&&"overview-1".to_string()),
            "de-duplicated caption id missing, got: {:?}",
            ids
        );
    }

    #[test]
    fn caption_link_rewriting_resolves() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\nTable: Standards\n\n| A |\n|---|\n| 1 |\n\n[See table](#standards)\n",
        );
        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        // The link target `#standards` should resolve.
        assert!(
            graph.link_targets.values().any(|v| v == "#standards"),
            "expected '#standards' link target, got: {:?}",
            graph.link_targets
        );
    }

    #[test]
    fn caption_in_included_document() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\n:::include\n# [Part](part.md)\n:::\n",
        );
        write(
            &root.join("doc/part.md"),
            "# Part\n\nTable: Part table\n\n| A |\n|---|\n| 1 |\n",
        );
        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        assert!(
            graph.anchor_targets.values().any(|v| v == "part-table"),
            "expected 'part-table' in anchor_targets, got: {:?}",
            graph.anchor_targets
        );
    }

    /// Helper: collect all resolved link targets for a given document occurrence
    /// by walking the rendered-node list. Returns
    /// `Vec<(source_destination_text, rewritten_target)>`.
    fn collect_link_targets(graph: &DocumentGraph, doc_id: usize) -> Vec<(String, String)> {
        let doc = graph.document(doc_id);
        let mut result = Vec::new();
        for (nid, node_ref, _) in graph.rendered_nodes_preorder() {
            if nid != doc_id {
                continue;
            }
            if let KindData::Link(link) = doc.arena[node_ref].kind_data() {
                if let Some(target) = graph.link_target(doc_id, node_ref) {
                    result.push((
                        link.destination_str(&doc.source).to_string(),
                        target.to_string(),
                    ));
                }
            }
        }
        result
    }

    /// Comprehensive test for link resolution across a multi-file document
    /// hierarchy with subdirectories, nested includes, and various link forms.
    ///
    /// Document tree:
    /// ```text
    /// doc/
    ///   index.md          — root, includes arch/ and guide/
    ///   arch/
    ///     index.md        — includes overview.md and detail.md
    ///     overview.md     — links to detail.md, ../guide/setup.md
    ///     detail.md       — links to overview.md, has duplicate heading text
    ///   guide/
    ///     setup.md        — links to ../arch/overview.md via absolute path,
    ///                       links to teardown.md (sibling), links back to root
    ///     teardown.md     — links to setup.md#steps
    /// ```
    #[test]
    fn resolves_links_across_multi_file_hierarchy() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Root document: includes arch and guide subtrees.
        write(
            &root.join("doc/index.md"),
            "\
# Project

:::include
# [Architecture](arch/index.md)
# [Guide](guide/setup.md)
## [Teardown](guide/teardown.md)
:::

[See architecture](arch/index.md)
",
        );

        // arch/index.md: includes overview and detail.
        write(
            &root.join("doc/arch/index.md"),
            "\
# Architecture

:::include
## [Overview](overview.md)
## [Detail](detail.md)
:::
",
        );

        // arch/overview.md: links to sibling detail.md and cross-directory
        // guide/setup.md.
        write(
            &root.join("doc/arch/overview.md"),
            "\
# System Overview

See [detail](detail.md) and [setup](../guide/setup.md).
",
        );

        // arch/detail.md: has two headings with the same text to exercise
        // local_seen de-duplication; links to overview.md with a fragment.
        write(
            &root.join("doc/arch/detail.md"),
            "\
# Implementation

## Notes

First notes section.

## Notes

Second notes section.

See [overview heading](overview.md#system-overview).
",
        );

        // guide/setup.md: uses absolute path link to arch/overview.md,
        // relative link to sibling teardown.md, and link back to root without
        // fragment (should resolve to first heading of root).
        write(
            &root.join("doc/guide/setup.md"),
            "\
# Setup

## Steps

Follow [architecture overview](/doc/arch/overview.md).

Also see [teardown](teardown.md) and [project root](../index.md).
",
        );

        // guide/teardown.md: links to setup.md with a fragment.
        write(
            &root.join("doc/guide/teardown.md"),
            "\
# Teardown

See [setup steps](setup.md#steps).
",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        // --- Verify heading IDs ---

        // Root document.
        assert_eq!(
            graph.first_heading_id(graph.root_id),
            Some("project"),
            "root heading should be 'project'"
        );

        // Resolve each included file to its doc_id.
        let arch_idx_path = root.join("doc/arch/index.md").canonicalize().unwrap();
        let overview_path = root.join("doc/arch/overview.md").canonicalize().unwrap();
        let detail_path = root.join("doc/arch/detail.md").canonicalize().unwrap();
        let setup_path = root.join("doc/guide/setup.md").canonicalize().unwrap();
        let teardown_path = root.join("doc/guide/teardown.md").canonicalize().unwrap();

        let arch_idx_id = graph
            .resolve_markdown_target(graph.root_id, &arch_idx_path)
            .unwrap();
        let overview_id = graph
            .resolve_markdown_target(arch_idx_id, &overview_path)
            .unwrap();
        let detail_id = graph
            .resolve_markdown_target(arch_idx_id, &detail_path)
            .unwrap();
        let setup_id = graph
            .resolve_markdown_target(graph.root_id, &setup_path)
            .unwrap();
        let teardown_id = graph
            .resolve_markdown_target(graph.root_id, &teardown_path)
            .unwrap();

        assert_eq!(graph.first_heading_id(arch_idx_id), Some("architecture"));
        assert_eq!(graph.first_heading_id(overview_id), Some("system-overview"));
        assert_eq!(graph.first_heading_id(detail_id), Some("implementation"));
        assert_eq!(graph.first_heading_id(setup_id), Some("setup"));
        assert_eq!(graph.first_heading_id(teardown_id), Some("teardown"));

        // --- Verify duplicate heading de-duplication (local_seen fix) ---

        // detail.md has two "## Notes" headings.  The first should get
        // source_anchor "notes" → final_id "notes", the second should get
        // source_anchor "notes-1" → final_id "notes-1".
        assert_eq!(
            graph.anchor_target(detail_id, "notes"),
            Some("notes"),
            "first 'Notes' heading should have source_anchor 'notes' mapping to final_id 'notes'"
        );
        assert_eq!(
            graph.anchor_target(detail_id, "notes-1"),
            Some("notes-1"),
            "second 'Notes' heading should have source_anchor 'notes-1' mapping to final_id 'notes-1'"
        );

        // --- Verify cross-file link resolution ---

        // Root: [See architecture](arch/index.md) → #architecture
        let root_links = collect_link_targets(graph, graph.root_id);
        let arch_link = root_links
            .iter()
            .find(|(src, _)| src == "arch/index.md")
            .expect("root should have a link to arch/index.md");
        assert_eq!(
            arch_link.1, "#architecture",
            "link to arch/index.md should resolve to #architecture"
        );

        // overview.md: [detail](detail.md) → first heading of detail.md
        let overview_links = collect_link_targets(graph, overview_id);
        let detail_link = overview_links
            .iter()
            .find(|(src, _)| src == "detail.md")
            .expect("overview should have a link to detail.md");
        assert_eq!(
            detail_link.1, "#implementation",
            "link to detail.md without fragment should resolve to its first heading"
        );

        // overview.md: [setup](../guide/setup.md) → first heading of setup.md
        let setup_link = overview_links
            .iter()
            .find(|(src, _)| src == "../guide/setup.md")
            .expect("overview should have a link to ../guide/setup.md");
        assert_eq!(
            setup_link.1, "#setup",
            "cross-directory relative link to setup.md should resolve to #setup"
        );

        // detail.md: [overview heading](overview.md#system-overview) → #system-overview
        let detail_links = collect_link_targets(graph, detail_id);
        let overview_link = detail_links
            .iter()
            .find(|(src, _)| src == "overview.md#system-overview")
            .expect("detail should have a link to overview.md#system-overview");
        assert_eq!(
            overview_link.1, "#system-overview",
            "fragment link to overview.md#system-overview should resolve correctly"
        );

        // setup.md: [architecture overview](/doc/arch/overview.md) → absolute path
        let setup_links = collect_link_targets(graph, setup_id);
        let abs_link = setup_links
            .iter()
            .find(|(src, _)| src == "/doc/arch/overview.md")
            .expect("setup should have an absolute link to /doc/arch/overview.md");
        assert_eq!(
            abs_link.1, "#system-overview",
            "absolute path link should resolve to #system-overview"
        );

        // setup.md: [teardown](teardown.md) → #teardown
        let teardown_link = setup_links
            .iter()
            .find(|(src, _)| src == "teardown.md")
            .expect("setup should have a link to teardown.md");
        assert_eq!(
            teardown_link.1, "#teardown",
            "sibling relative link to teardown.md should resolve to #teardown"
        );

        // setup.md: [project root](../index.md) → #project (first heading of root)
        let root_link = setup_links
            .iter()
            .find(|(src, _)| src == "../index.md")
            .expect("setup should have a link to ../index.md");
        assert_eq!(
            root_link.1, "#project",
            "link to root index.md without fragment should resolve to #project"
        );

        // teardown.md: [setup steps](setup.md#steps) → #steps
        let teardown_links = collect_link_targets(graph, teardown_id);
        let steps_link = teardown_links
            .iter()
            .find(|(src, _)| src == "setup.md#steps")
            .expect("teardown should have a link to setup.md#steps");
        assert_eq!(
            steps_link.1, "#steps",
            "fragment link to setup.md#steps should resolve correctly"
        );
    }

    // -- Diagram image tests --

    #[test]
    fn puml_image_becomes_diagram_node() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n![Sequence](seq.puml)\n",
        );
        write(
            &root.join("doc/seq.puml"),
            "@startuml\nAlice -> Bob\n@enduml\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children: Vec<_> = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect();

        // Should have [Heading, Diagram] — not [Heading, Paragraph(Image)].
        assert_eq!(children.len(), 2, "expected Heading + Diagram");
        assert!(
            rushdown::matches_extension_kind!(
                &root_doc.arena,
                children[1],
                crate::diagram::Diagram
            ),
            "expected Diagram extension node, got {:?}",
            root_doc.arena[children[1]].kind_data().kind_name()
        );
        let diagram =
            rushdown::as_extension_data!(&root_doc.arena, children[1], crate::diagram::Diagram);
        assert_eq!(
            diagram.diagram_type(),
            crate::diagram::DiagramType::PlantUml
        );
        assert!(
            diagram.file_source().unwrap().contains("Alice -> Bob"),
            "diagram file_source should contain the PlantUML content"
        );
    }

    #[test]
    fn mmd_image_becomes_diagram_node() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(&root.join("doc/index.md"), "# Doc\n\n![Flow](flow.mmd)\n");
        write(&root.join("doc/flow.mmd"), "flowchart LR\n  A --> B\n");

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children: Vec<_> = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect();

        assert_eq!(children.len(), 2);
        assert!(rushdown::matches_extension_kind!(
            &root_doc.arena,
            children[1],
            crate::diagram::Diagram
        ),);
        let diagram =
            rushdown::as_extension_data!(&root_doc.arena, children[1], crate::diagram::Diagram);
        assert_eq!(diagram.diagram_type(), crate::diagram::DiagramType::Mermaid);
    }

    #[test]
    fn typ_image_becomes_diagram_node() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n![Diagram](diagram.typ)\n",
        );
        write(
            &root.join("doc/diagram.typ"),
            "#rect(width: 50pt, height: 30pt)\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children: Vec<_> = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect();

        assert_eq!(children.len(), 2, "expected Heading + Diagram");
        assert!(
            rushdown::matches_extension_kind!(
                &root_doc.arena,
                children[1],
                crate::diagram::Diagram
            ),
            "expected Diagram extension node, got {:?}",
            root_doc.arena[children[1]].kind_data().kind_name()
        );
        let diagram =
            rushdown::as_extension_data!(&root_doc.arena, children[1], crate::diagram::Diagram);
        assert_eq!(diagram.diagram_type(), crate::diagram::DiagramType::Typst);
        assert!(
            diagram.file_source().unwrap().contains("rect"),
            "diagram file_source should contain the Typst content"
        );
    }

    #[test]
    fn figure_caption_with_puml_image() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\nFigure: My sequence\n\n![Sequence](seq.puml)\n",
        );
        write(
            &root.join("doc/seq.puml"),
            "@startuml\nAlice -> Bob\n@enduml\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children: Vec<_> = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect();

        // Should have [Heading, Caption(CaptionText + Diagram)].
        assert_eq!(children.len(), 2, "expected Heading + Caption");
        assert!(
            rushdown::matches_extension_kind!(
                &root_doc.arena,
                children[1],
                crate::caption::Caption
            ),
            "expected Caption extension node, got {:?}",
            root_doc.arena[children[1]].kind_data().kind_name()
        );
    }

    #[test]
    fn non_diagram_image_left_unchanged() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(&root.join("doc/index.md"), "# Doc\n\n![Photo](photo.png)\n");
        write(&root.join("doc/photo.png"), "fake png");

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children: Vec<_> = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect();

        // Should have [Heading, Paragraph(Image)] — not transformed.
        assert_eq!(children.len(), 2);
        assert!(
            matches!(
                root_doc.arena[children[1]].kind_data(),
                KindData::Paragraph(_)
            ),
            "non-diagram image should remain in a Paragraph"
        );
    }

    #[test]
    fn mixed_content_paragraph_not_transformed() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\nSee ![Sequence](seq.puml) here.\n",
        );
        write(
            &root.join("doc/seq.puml"),
            "@startuml\nAlice -> Bob\n@enduml\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children: Vec<_> = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect();

        // Image is not the sole child → paragraph stays.
        assert_eq!(children.len(), 2);
        assert!(
            matches!(
                root_doc.arena[children[1]].kind_data(),
                KindData::Paragraph(_)
            ),
            "paragraph with mixed content should not be transformed"
        );
    }

    #[test]
    fn puml_image_not_in_asset_targets() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n![Sequence](seq.puml)\n",
        );
        write(
            &root.join("doc/seq.puml"),
            "@startuml\nAlice -> Bob\n@enduml\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();

        // The .puml file should NOT appear in asset_targets (it was consumed
        // by the diagram transform, not meant to be copied as a static asset).
        let assets = prepared.asset_targets();
        let puml_path = root.join("doc/seq.puml").canonicalize().unwrap();
        assert!(
            !assets.contains(&puml_path),
            "diagram source file should not be in asset_targets: {:?}",
            assets
        );
    }

    #[test]
    fn figure_caption_with_regular_image() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\nFigure: A figure\n\n![figure](photo.png)\n",
        );
        write(&root.join("doc/photo.png"), "fake png");

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children: Vec<_> = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect();

        // Should have [Heading, Caption(CaptionText + Paragraph(Image))].
        assert_eq!(children.len(), 2, "expected Heading + Caption");
        assert!(
            rushdown::matches_extension_kind!(
                &root_doc.arena,
                children[1],
                crate::caption::Caption
            ),
            "expected Caption extension node wrapping the image, got {:?}",
            root_doc.arena[children[1]].kind_data().kind_name()
        );
    }

    #[test]
    fn figure_caption_with_regular_image_renders_typst() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\nFigure: A nice photo\n\n![Photo](photo.png)\n",
        );
        write(&root.join("doc/photo.png"), "fake png");

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let rendered = crate::render_typst::TypstRenderer::new(
            root.to_path_buf(),
            Default::default(),
            None,
            None,
            None,
        )
        .render_document(&prepared, &DebugDumper::new(false))
        .unwrap();
        assert!(
            rendered.contains("#figure(caption:"),
            "Typst output should contain #figure(caption: ...)"
        );
        assert!(
            rendered.contains("A nice photo"),
            "Typst output should contain the caption text"
        );
        assert!(
            rendered.contains("#image("),
            "Typst output should contain #image(...) inside the figure"
        );
    }

    // -- Draw.io diagram image tests --

    #[test]
    fn drawio_image_becomes_diagram_node() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        // Copy the real .drawio asset into the temp dir so that path
        // resolution (canonicalize) succeeds during parsing.
        let asset_src = Path::new("doc/assets/Docbake High Level.drawio");
        if !asset_src.exists() {
            eprintln!("skipping drawio_image_becomes_diagram_node: test asset not found");
            return;
        }
        fs::create_dir_all(root.join("doc")).unwrap();
        fs::copy(asset_src, root.join("doc/diagram.drawio")).unwrap();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n![Architecture](diagram.drawio)\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children: Vec<_> = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect();

        // Should have [Heading, Diagram] — not [Heading, Paragraph(Image)].
        assert_eq!(children.len(), 2, "expected Heading + Diagram");
        assert!(
            rushdown::matches_extension_kind!(
                &root_doc.arena,
                children[1],
                crate::diagram::Diagram
            ),
            "expected Diagram extension node, got {:?}",
            root_doc.arena[children[1]].kind_data().kind_name()
        );
        let diagram =
            rushdown::as_extension_data!(&root_doc.arena, children[1], crate::diagram::Diagram);
        assert_eq!(diagram.diagram_type(), crate::diagram::DiagramType::DrawIo);
        // Draw.io rendering is deferred — the AST stores the path, not the
        // pre-rendered SVG.
        assert!(
            diagram.drawio_path().is_some(),
            "diagram should have a drawio_path for deferred rendering"
        );
        assert!(
            diagram.file_source().is_none(),
            "diagram file_source should be None for deferred Draw.io diagrams"
        );
    }

    #[test]
    fn drawio_image_with_page_fragment() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let asset_src = Path::new("doc/assets/Docbake High Level.drawio");
        if !asset_src.exists() {
            eprintln!("skipping drawio_image_with_page_fragment: test asset not found");
            return;
        }
        fs::create_dir_all(root.join("doc")).unwrap();
        fs::copy(asset_src, root.join("doc/diagram.drawio")).unwrap();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n![Page 1](diagram.drawio#1)\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children: Vec<_> = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect();

        assert_eq!(children.len(), 2, "expected Heading + Diagram");
        assert!(
            rushdown::matches_extension_kind!(
                &root_doc.arena,
                children[1],
                crate::diagram::Diagram
            ),
            "expected Diagram extension node"
        );
        let diagram =
            rushdown::as_extension_data!(&root_doc.arena, children[1], crate::diagram::Diagram);
        assert_eq!(diagram.diagram_type(), crate::diagram::DiagramType::DrawIo);
        assert!(
            diagram.drawio_path().is_some(),
            "diagram should have a drawio_path for deferred rendering"
        );
        assert_eq!(
            diagram.drawio_page(),
            Some(1),
            "diagram should store page number from fragment"
        );
    }

    #[test]
    fn drawio_image_not_in_asset_targets() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let asset_src = Path::new("doc/assets/Docbake High Level.drawio");
        if !asset_src.exists() {
            eprintln!("skipping drawio_image_not_in_asset_targets: test asset not found");
            return;
        }
        fs::create_dir_all(root.join("doc")).unwrap();
        fs::copy(asset_src, root.join("doc/diagram.drawio")).unwrap();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n![Architecture](diagram.drawio)\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();

        // The .drawio file should NOT appear in asset_targets.
        let assets = prepared.asset_targets();
        let drawio_path = root.join("doc/diagram.drawio").canonicalize().unwrap();
        assert!(
            !assets.contains(&drawio_path),
            "diagram source file should not be in asset_targets: {:?}",
            assets
        );
    }

    #[test]
    fn figure_caption_with_drawio_image() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let asset_src = Path::new("doc/assets/Docbake High Level.drawio");
        if !asset_src.exists() {
            eprintln!("skipping figure_caption_with_drawio_image: test asset not found");
            return;
        }
        fs::create_dir_all(root.join("doc")).unwrap();
        fs::copy(asset_src, root.join("doc/diagram.drawio")).unwrap();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\nFigure: Architecture overview\n\n![Architecture](diagram.drawio)\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children: Vec<_> = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect();

        // Should have [Heading, Caption(CaptionText + Diagram)].
        assert_eq!(children.len(), 2, "expected Heading + Caption");
        assert!(
            rushdown::matches_extension_kind!(
                &root_doc.arena,
                children[1],
                crate::caption::Caption
            ),
            "expected Caption extension node, got {:?}",
            root_doc.arena[children[1]].kind_data().kind_name()
        );
    }

    #[test]
    fn d2_image_becomes_diagram_node() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n![Network](network.d2)\n",
        );
        write(&root.join("doc/network.d2"), "x -> y: hello\n");

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children: Vec<_> = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect();

        assert_eq!(children.len(), 2, "expected Heading + Diagram");
        assert!(
            rushdown::matches_extension_kind!(
                &root_doc.arena,
                children[1],
                crate::diagram::Diagram
            ),
            "expected Diagram extension node, got {:?}",
            root_doc.arena[children[1]].kind_data().kind_name()
        );
        let diagram =
            rushdown::as_extension_data!(&root_doc.arena, children[1], crate::diagram::Diagram);
        assert_eq!(diagram.diagram_type(), crate::diagram::DiagramType::D2);
        assert!(
            diagram.file_source().unwrap().contains("x -> y"),
            "diagram file_source should contain the D2 content"
        );
    }

    #[test]
    fn d2_image_not_in_asset_targets() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n![Network](network.d2)\n",
        );
        write(&root.join("doc/network.d2"), "x -> y: hello\n");

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();

        // The .d2 file should NOT appear in asset_targets (it was consumed
        // by the diagram transform, not meant to be copied as a static asset).
        let assets = prepared.asset_targets();
        let d2_path = root.join("doc/network.d2").canonicalize().unwrap();
        assert!(
            !assets.contains(&d2_path),
            "diagram source file should not be in asset_targets: {:?}",
            assets
        );
    }

    #[test]
    fn figure_caption_with_d2_image() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\nFigure: Network diagram\n\n![Network](network.d2)\n",
        );
        write(&root.join("doc/network.d2"), "x -> y: hello\n");

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let root_doc = prepared.graph().root_document();
        let children: Vec<_> = root_doc.arena[root_doc.doc_ref]
            .children(&root_doc.arena)
            .collect();

        // Should have [Heading, Caption(CaptionText + Diagram)].
        assert_eq!(children.len(), 2, "expected Heading + Caption");
        assert!(
            rushdown::matches_extension_kind!(
                &root_doc.arena,
                children[1],
                crate::caption::Caption
            ),
            "expected Caption extension node, got {:?}",
            root_doc.arena[children[1]].kind_data().kind_name()
        );
    }

    // -- Code block auto-ID tests --

    #[test]
    fn code_block_auto_id_from_description() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n```rust Basic function\nfn basic() {}\n```\n",
        );
        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        assert!(
            graph.anchor_targets.values().any(|v| v == "basic-function"),
            "expected 'basic-function' in anchor_targets, got: {:?}",
            graph.anchor_targets
        );
    }

    #[test]
    fn code_block_auto_id_with_attributes() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n```python HTTP client {.hl}\nimport requests\n```\n",
        );
        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        assert!(
            graph.anchor_targets.values().any(|v| v == "http-client"),
            "expected 'http-client' in anchor_targets, got: {:?}",
            graph.anchor_targets
        );
    }

    #[test]
    fn code_block_explicit_id_preserved() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n```rust Foo {#my-code}\nfn foo() {}\n```\n",
        );
        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        assert!(
            graph.anchor_targets.values().any(|v| v == "my-code"),
            "expected 'my-code' in anchor_targets, got: {:?}",
            graph.anchor_targets
        );
    }

    #[test]
    fn code_block_no_description_no_id() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n```rust\nfn main() {}\n```\n",
        );
        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        // Only the heading should have an ID, not the code block.
        assert_eq!(
            graph.anchor_targets.len(),
            1,
            "expected only the heading target, got: {:?}",
            graph.anchor_targets
        );
    }

    #[test]
    fn code_block_id_dedup_with_heading() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Basic function\n\n```rust Basic function\nfn basic() {}\n```\n",
        );
        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        let ids: Vec<&String> = graph.anchor_targets.values().collect();
        assert!(
            ids.contains(&&"basic-function".to_string()),
            "expected 'basic-function' in anchor_targets, got: {:?}",
            ids
        );
        assert!(
            ids.contains(&&"basic-function-1".to_string()),
            "expected 'basic-function-1' in anchor_targets, got: {:?}",
            ids
        );
    }

    #[test]
    fn code_block_id_dedup_between_code_blocks() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n```rust Example\nfn a() {}\n```\n\n```python Example\ndef b(): pass\n```\n",
        );
        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        let ids: Vec<&String> = graph.anchor_targets.values().collect();
        assert!(
            ids.contains(&&"example".to_string()),
            "expected 'example' in anchor_targets, got: {:?}",
            ids
        );
        assert!(
            ids.contains(&&"example-1".to_string()),
            "expected 'example-1' in anchor_targets, got: {:?}",
            ids
        );
    }

    // -------------------------------------------------------------------
    // Broken-link diagnostics
    // -------------------------------------------------------------------

    #[test]
    fn broken_fragment_link_detected_in_error_mode() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\n[broken](#nonexistent)\n",
        );

        let error = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap_err();
        let msg = error.to_string();
        assert!(
            msg.contains("nonexistent"),
            "expected 'nonexistent' in error: {msg}"
        );
        assert!(
            msg.contains("broken link"),
            "expected 'broken link' in error: {msg}"
        );
    }

    #[test]
    fn broken_fragment_link_allowed_in_warn_mode() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\n[broken](#nonexistent)\n",
        );

        // Warn mode: build succeeds despite the broken link.
        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Warn,
        )
        .unwrap();
        // The fallback target is inserted so rendering can proceed.
        assert!(
            prepared
                .graph()
                .link_targets
                .values()
                .any(|v| v == "#nonexistent"),
            "expected fallback target '#nonexistent' in link_targets"
        );
    }

    #[test]
    fn valid_fragment_link_rewritten_to_final_id() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        // The document has two headings "Section" — the second becomes
        // "section-1" after deduplication.  A #section link should resolve
        // to the first heading's final id.
        write(
            &root.join("doc/index.md"),
            "# Root\n\n## Section\n\n## Section\n\n[go](#section)\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        assert!(
            prepared
                .graph()
                .link_targets
                .values()
                .any(|v| v == "#section"),
            "expected rewritten target '#section' in link_targets"
        );
    }

    #[test]
    fn fragment_link_is_file_scoped_not_graph_scoped() {
        // A #fragment link in root should NOT resolve against headings
        // in included files — fragments are file-scoped.
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\n:::include\n# [Part](part.md)\n:::\n\n[go](#part-heading)\n",
        );
        write(&root.join("doc/part.md"), "# Part Heading\n\nContent.\n");

        let error = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap_err();
        let msg = error.to_string();
        assert!(
            msg.contains("part-heading"),
            "expected 'part-heading' in error: {msg}"
        );
    }

    #[test]
    fn multiple_broken_links_all_reported() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\n[a](#aaa)\n\n[b](#bbb)\n\n[c](#ccc)\n",
        );

        let error = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap_err();
        let msg = error.to_string();
        assert!(msg.contains("aaa"), "expected 'aaa' in error: {msg}");
        assert!(msg.contains("bbb"), "expected 'bbb' in error: {msg}");
        assert!(msg.contains("ccc"), "expected 'ccc' in error: {msg}");
        assert!(
            msg.contains("3 broken links"),
            "expected '3 broken links' in error: {msg}"
        );
    }

    #[test]
    fn broken_md_link_collected_as_diagnostic() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\n[missing](missing.md)\n",
        );

        let error = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap_err();
        let msg = error.to_string();
        assert!(
            msg.contains("missing"),
            "expected 'missing' in error: {msg}"
        );
        assert!(
            msg.contains("broken link"),
            "expected 'broken link' in error: {msg}"
        );
    }

    // ----- Explicit ID tracking for images, diagrams, spans, and divs -----

    #[test]
    fn image_explicit_id_tracked() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n![Photo](photo.png){#my-img}\n",
        );
        write(&root.join("doc/photo.png"), "fake-png");

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        assert!(
            graph.anchor_targets.values().any(|v| v == "my-img"),
            "expected 'my-img' in anchor_targets, got: {:?}",
            graph.anchor_targets
        );
    }

    #[test]
    fn link_to_image_id_resolves() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n![Photo](photo.png){#my-img}\n\nSee [the image](#my-img).\n",
        );
        write(&root.join("doc/photo.png"), "fake-png");

        // Should succeed — the fragment #my-img is a known anchor.
        prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
    }

    #[test]
    fn diagram_explicit_id_tracked() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n![Seq](seq.puml){#my-diagram}\n",
        );
        write(
            &root.join("doc/seq.puml"),
            "@startuml\nAlice -> Bob\n@enduml\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        assert!(
            graph.anchor_targets.values().any(|v| v == "my-diagram"),
            "expected 'my-diagram' in anchor_targets, got: {:?}",
            graph.anchor_targets
        );
    }

    #[test]
    fn link_to_diagram_id_resolves() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n![Seq](seq.puml){#my-diagram}\n\nSee [diagram](#my-diagram).\n",
        );
        write(
            &root.join("doc/seq.puml"),
            "@startuml\nAlice -> Bob\n@enduml\n",
        );

        prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
    }

    #[test]
    fn bracketed_span_explicit_id_tracked() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n[important text]{#my-span}\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        assert!(
            graph.anchor_targets.values().any(|v| v == "my-span"),
            "expected 'my-span' in anchor_targets, got: {:?}",
            graph.anchor_targets
        );
    }

    #[test]
    fn link_to_bracketed_span_id_resolves() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n[important text]{#my-span}\n\nSee [ref](#my-span).\n",
        );

        prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
    }

    #[test]
    fn fenced_div_explicit_id_tracked() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n:::{#my-div .note}\nSome content.\n:::\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
        let graph = prepared.graph();

        assert!(
            graph.anchor_targets.values().any(|v| v == "my-div"),
            "expected 'my-div' in anchor_targets, got: {:?}",
            graph.anchor_targets
        );
    }

    #[test]
    fn link_to_fenced_div_id_resolves() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Doc\n\n:::{#my-div .note}\nSome content.\n:::\n\nSee [ref](#my-div).\n",
        );

        prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap();
    }

    #[test]
    fn duplicate_explicit_id_across_element_types_rejected() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        // Heading and image both claim id "foo".
        write(
            &root.join("doc/index.md"),
            "# Doc {#foo}\n\n![Photo](photo.png){#foo}\n",
        );
        write(&root.join("doc/photo.png"), "fake-png");

        let error = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            None,
            BrokenLinkMode::Error,
        )
        .unwrap_err();
        let msg = error.to_string();
        assert!(
            msg.contains("duplicate") && msg.contains("foo"),
            "expected duplicate id error mentioning 'foo', got: {msg}"
        );
    }
}
