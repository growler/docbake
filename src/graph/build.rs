use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use rushdown::ast::{self, Arena, KindData, NodeRef, WalkStatus};
use rushdown::parser::AstTransformer as _;
use rushdown::text::BasicReader;
use rushdown_fenced_div::FencedDiv;

use super::types::{DocumentOccurrence, FootnoteDefinitionEntry, IncludeEntry};
use super::DocumentGraph;
use crate::build::markdown_parser;
use crate::DebugDumper;

pub(crate) fn parse_document_graph(
    base_dir: &Path,
    logger: &DebugDumper,
    source_path: &Path,
) -> Result<DocumentGraph> {
    let mut builder = GraphBuilder::new(base_dir, logger);
    let root_id = builder.build_document(source_path, true)?;
    let mut graph = DocumentGraph {
        root_id,
        documents: builder.documents,
        rendered_nodes: Vec::new(),
        occurrences_by_path: HashMap::new(),
        first_heading_by_doc: HashMap::new(),
        heading_targets: HashMap::new(),
        link_targets: HashMap::new(),
        image_targets: HashMap::new(),
        asset_targets: HashSet::new(),
    };
    graph.build_occurrence_index();
    graph.build_rendered_nodes();
    Ok(graph)
}

struct GraphBuilder {
    base_dir: PathBuf,
    logger: DebugDumper,
    documents: Vec<DocumentOccurrence>,
    stack: Vec<PathBuf>,
}

impl GraphBuilder {
    fn new(base_dir: &Path, logger: &DebugDumper) -> Self {
        Self {
            base_dir: base_dir.to_path_buf(),
            logger: logger.clone(),
            documents: Vec::new(),
            stack: Vec::new(),
        }
    }

    fn build_document(&mut self, path: &Path, keep_front_matter: bool) -> Result<usize> {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("failed to resolve {}", path.display()))?;
        if self.stack.contains(&canonical) {
            let chain = self
                .stack
                .iter()
                .chain(std::iter::once(&canonical))
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(" -> ");
            bail!("include cycle detected: {chain}");
        }
        if !canonical.starts_with(&self.base_dir) {
            bail!(
                "include target {} is outside the project root {}",
                canonical.display(),
                self.base_dir.display()
            );
        }

        self.logger.log(format!("parsing {}", canonical.display()));
        let source = fs::read_to_string(&canonical)
            .with_context(|| format!("failed to read {}", canonical.display()))?;
        let (_front_matter, body) = split_front_matter(&source);
        let parsed_source = if keep_front_matter {
            source
        } else {
            body.to_string()
        };

        let parser = markdown_parser();
        let mut reader = BasicReader::new(&parsed_source);
        let (mut arena, doc_ref) = parser.parse(&mut reader);

        // Transform diagram images (.puml, .mmd, .drawio) into Diagram AST nodes.
        let source_dir = canonical.parent().unwrap_or(&self.base_dir);
        crate::diagram::transform_diagram_images(
            &mut arena,
            doc_ref,
            &parsed_source,
            source_dir,
            &self.base_dir,
        )?;

        // Run caption detection after all diagram transforms are complete,
        // so that Figure: captions can match both code-block diagrams and
        // file-based diagram images (.puml, .mmd).
        {
            let mut reader = BasicReader::new(&parsed_source);
            let mut ctx = rushdown::parser::Context::new();
            crate::caption::CaptionAstTransformer::new().transform(
                &mut arena,
                doc_ref,
                &mut reader,
                &mut ctx,
            );
        }

        let raw_includes = collect_include_specs(&arena, doc_ref, &parsed_source, &canonical)?;
        let footnote_definitions =
            collect_footnote_definitions(&arena, doc_ref, &parsed_source, &canonical)?;

        let doc_id = self.documents.len();
        self.documents.push(DocumentOccurrence {
            id: doc_id,
            path: canonical.clone(),
            source: parsed_source,
            arena,
            doc_ref,
            includes: Vec::new(),
            footnote_definitions,
        });

        self.stack.push(canonical.clone());
        let mut include_map = Vec::new();
        for raw_include in raw_includes {
            let mut entries = Vec::new();
            for raw_entry in raw_include.entries {
                let target = resolve_include_target(
                    &self.base_dir,
                    canonical.parent().unwrap_or(&self.base_dir),
                    &raw_entry.target,
                )?;
                let child_id = self.build_document(&target, false)?;
                entries.push(IncludeEntry {
                    doc_id: child_id,
                    heading_shift: raw_entry.heading_shift,
                    target_path: target,
                });
            }
            include_map.push((raw_include.node_ref, entries));
        }
        self.documents[doc_id].includes = include_map;
        self.stack.pop();

        Ok(doc_id)
    }
}

struct RawIncludeSpec {
    node_ref: NodeRef,
    entries: Vec<RawIncludeEntry>,
}

struct RawIncludeEntry {
    heading_shift: usize,
    target: String,
}

fn collect_include_specs(
    arena: &Arena,
    doc_ref: NodeRef,
    source: &str,
    path: &Path,
) -> Result<Vec<RawIncludeSpec>> {
    let mut specs = Vec::new();

    fn is_include_div(arena: &Arena, node_ref: NodeRef, source: &str) -> bool {
        rushdown::matches_extension_kind!(arena, node_ref, FencedDiv)
            && arena[node_ref]
                .attributes()
                .get("class")
                .map(|v| v.str(source))
                .is_some_and(|classes| classes.split_whitespace().any(|c| c == "include"))
    }

    fn visit(
        arena: &Arena,
        node_ref: NodeRef,
        doc_ref: NodeRef,
        source: &str,
        path: &Path,
        in_include: bool,
        specs: &mut Vec<RawIncludeSpec>,
    ) -> Result<()> {
        let is_include = is_include_div(arena, node_ref, source);
        if is_include {
            if in_include {
                bail!(
                    "nested :::include blocks are not allowed in {}",
                    path.display()
                );
            }
            if arena[node_ref].parent() != Some(doc_ref) {
                bail!(
                    ":::include blocks must be top-level document blocks in {}",
                    path.display()
                );
            }
            specs.push(extract_include_spec(arena, node_ref, source, path)?);
        }
        for child in arena[node_ref].children(arena) {
            visit(
                arena,
                child,
                doc_ref,
                source,
                path,
                in_include || is_include,
                specs,
            )?;
        }
        Ok(())
    }

    visit(arena, doc_ref, doc_ref, source, path, false, &mut specs)?;
    Ok(specs)
}

fn extract_include_spec(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
    path: &Path,
) -> Result<RawIncludeSpec> {
    let mut entries = Vec::new();
    for child in arena[node_ref].children(arena) {
        let KindData::Heading(heading) = arena[child].kind_data() else {
            bail!(
                "include block in {} may only contain markdown heading links",
                path.display()
            );
        };
        if !arena[child].attributes().is_empty() {
            bail!(
                "include entry headings may not have attributes in {}",
                path.display()
            );
        }

        let mut children = arena[child].children(arena);
        let first = children
            .next()
            .ok_or_else(|| anyhow!("include entry heading is empty in {}", path.display()))?;
        if children.next().is_some() {
            bail!(
                "include entry heading must contain exactly one markdown link in {}",
                path.display()
            );
        }

        let KindData::Link(link) = arena[first].kind_data() else {
            bail!(
                "include entry heading must contain exactly one markdown link in {}",
                path.display()
            );
        };
        if arena[first].children(arena).next().is_none() {
            bail!("include entry link must have a label in {}", path.display());
        }

        let target = link.destination_str(source);
        if target.contains('#') {
            bail!(
                "include targets may not contain fragments in {}",
                path.display()
            );
        }
        entries.push(RawIncludeEntry {
            heading_shift: heading.level().saturating_sub(1) as usize,
            target: target.to_string(),
        });
    }

    Ok(RawIncludeSpec { node_ref, entries })
}

fn resolve_include_target(base_dir: &Path, base: &Path, target: &str) -> Result<PathBuf> {
    super::resolve::resolve_base_path(base_dir, base, target)
}

fn split_front_matter(input: &str) -> (Option<String>, &str) {
    let Some(_rest) = input.strip_prefix("---") else {
        return (None, input);
    };
    let Some(mut offset) = consume_line_ending(input, 3) else {
        return (None, input);
    };

    while offset < input.len() {
        let line_end = input[offset..]
            .find('\n')
            .map_or(input.len(), |index| offset + index + 1);
        let line = input[offset..line_end].trim_end_matches(['\r', '\n']);
        if line == "---" {
            let front_matter = input[..line_end].to_string();
            return (Some(front_matter), &input[line_end..]);
        }
        offset = line_end;
    }
    (None, input)
}

fn consume_line_ending(input: &str, prefix_len: usize) -> Option<usize> {
    let suffix = input.get(prefix_len..)?;
    if suffix.starts_with("\r\n") {
        Some(prefix_len + 2)
    } else if suffix.starts_with('\n') {
        Some(prefix_len + 1)
    } else {
        None
    }
}

fn collect_footnote_definitions(
    arena: &Arena,
    doc_ref: NodeRef,
    markdown: &str,
    path: &Path,
) -> Result<HashMap<String, FootnoteDefinitionEntry>> {
    let mut definition_nodes = Vec::new();
    let _ = ast::walk(arena, doc_ref, &mut |arena: &Arena,
                                            node_ref: NodeRef,
                                            entering: bool|
     -> std::result::Result<
        WalkStatus,
        std::io::Error,
    > {
        if entering
            && rushdown::matches_extension_kind!(
                arena,
                node_ref,
                rushdown_footnote::FootnoteDefinition
            )
        {
            definition_nodes.push(node_ref);
            return Ok(WalkStatus::SkipChildren);
        }
        Ok(WalkStatus::Continue)
    });

    let labels = parse_footnote_definition_labels(markdown);
    if definition_nodes.len() != labels.len() {
        bail!(
            "failed to align footnote definitions in {}: parsed {} nodes but found {} labels",
            path.display(),
            definition_nodes.len(),
            labels.len()
        );
    }

    let mut map = HashMap::new();
    for (label, node_ref) in labels.into_iter().zip(definition_nodes) {
        map.insert(label, FootnoteDefinitionEntry { node_ref });
    }
    Ok(map)
}

static FOOTNOTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\[\^([^\]]+)\]:").expect("valid footnote label regex"));

fn parse_footnote_definition_labels(markdown: &str) -> Vec<String> {
    FOOTNOTE_RE
        .captures_iter(markdown)
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .collect()
}
