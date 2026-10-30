use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use anyhow::Result;
use rushdown::ast::{Arena, KindData, NodeRef};
use rushdown_definition_list::{DefinitionList, TermDefinition};
use rushdown_fenced_div::FencedDiv;

use crate::caption::{Caption, CaptionText};
use crate::graph::DocumentGraph;

use super::admonition::{body_style, fenced_div_admonition};
use super::media::DiagramJob;
use super::DocxRenderer;

impl<'a> DocxRenderer<'a> {
    /// Render the diagrams that the body walk embeds and that the artifact
    /// cache does not hold, in parallel, and store them in the cache.
    ///
    /// The body walk then reads these diagrams from the cache.  An error
    /// of the walk that collects the diagrams stops the build before a
    /// render: an invalid `width` or `height` of a diagram, or an invalid
    /// visibility condition.  The body walk gives the same error.  This
    /// step does not log and ignores render errors: on a cache miss the
    /// walk renders the diagram again and reports the error.  It does
    /// nothing when the cache is disabled.
    pub(super) fn prefetch_diagrams(&self, graph: &DocumentGraph) -> Result<()> {
        if !self.cache.is_enabled() {
            return Ok(());
        }
        let mut jobs = Vec::new();
        self.collect_graph_diagrams(graph, graph.root_id, &mut jobs)?;
        let mut seen = HashSet::new();
        jobs.retain(|job| {
            !self.cache.contains(job.cache_key(), job.cache_tag())
                && seen.insert((job.cache_key().to_vec(), job.cache_tag()))
        });

        let workers = thread::available_parallelism()
            .map_or(1, NonZeroUsize::get)
            .min(jobs.len());
        let cache = self.cache;
        let jobs = &jobs;
        let next = &AtomicUsize::new(0);
        thread::scope(|scope| {
            for _ in 0..workers {
                let _ = thread::Builder::new().spawn_scoped(scope, move || {
                    while let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let _ = job.render_cached(cache);
                    }
                });
            }
        });
        Ok(())
    }

    /// Collect the diagram jobs of a document occurrence with the include
    /// and visibility rules of `render_graph_document`.  An invalid `width`
    /// or `height` of a diagram is an error.
    pub(super) fn collect_graph_diagrams(
        &self,
        graph: &DocumentGraph,
        doc_id: usize,
        jobs: &mut Vec<DiagramJob>,
    ) -> Result<()> {
        let doc = graph.document(doc_id);
        for child in doc.arena[doc.doc_ref].children(&doc.arena) {
            if !crate::conditional::node_is_visible(&doc.arena, child, &doc.source, &self.profile)?
            {
                continue;
            }
            if let Some(entries) = graph.include_entries(doc_id, child) {
                for entry in entries {
                    self.collect_graph_diagrams(graph, entry.doc_id, jobs)?;
                }
                continue;
            }
            self.collect_block_diagrams(&doc.arena, &doc.source, &doc.path, child, None, jobs)?;
        }
        Ok(())
    }

    /// Collect the diagram jobs of a block of the document `path` with the
    /// traversal of `render_block`. `paragraph_style` is the paragraph
    /// style of the render context of the block.
    fn collect_block_diagrams(
        &self,
        arena: &Arena,
        source: &str,
        path: &Path,
        node_ref: NodeRef,
        paragraph_style: Option<&'static str>,
        jobs: &mut Vec<DiagramJob>,
    ) -> Result<()> {
        match arena[node_ref].kind_data() {
            KindData::Blockquote(_) => {
                for child in arena[node_ref].children(arena) {
                    self.collect_block_diagrams(arena, source, path, child, Some("Quote"), jobs)?;
                }
            }
            KindData::List(_) => {
                for item_ref in arena[node_ref].children(arena) {
                    if !matches!(arena[item_ref].kind_data(), KindData::ListItem(_)) {
                        continue;
                    }
                    for child in arena[item_ref].children(arena) {
                        self.collect_block_diagrams(arena, source, path, child, None, jobs)?;
                    }
                }
            }
            KindData::Heading(_)
            | KindData::Paragraph(_)
            | KindData::CodeBlock(_)
            | KindData::Table(_)
            | KindData::ThematicBreak(_) => {}
            _ => {
                if rushdown::matches_extension_kind!(arena, node_ref, Caption) {
                    let block_ref = arena[node_ref]
                        .children(arena)
                        .filter(|&child| {
                            !rushdown::matches_extension_kind!(arena, child, CaptionText)
                        })
                        .next_back();
                    if let Some(br) = block_ref {
                        self.collect_block_diagrams(
                            arena,
                            source,
                            path,
                            br,
                            paragraph_style,
                            jobs,
                        )?;
                    }
                } else if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::diagram::Diagram
                ) {
                    // The width of the diagram paragraph, as in
                    // `render_diagram`.
                    let max_width = self.style_width(paragraph_style.unwrap_or("ImageBlock"));
                    self.diagram_size(arena, source, path, node_ref, max_width)?;
                    if let Some(job) = self.diagram_job(arena, source, node_ref) {
                        jobs.push(job);
                    }
                } else if rushdown::matches_extension_kind!(arena, node_ref, FencedDiv)
                    && crate::conditional::fenced_div_is_visible(
                        arena,
                        node_ref,
                        source,
                        &self.profile,
                    )?
                {
                    let paragraph_style = match fenced_div_admonition(arena, source, node_ref) {
                        Some(adm_type) => Some(body_style(adm_type)),
                        None => paragraph_style,
                    };
                    for child in arena[node_ref].children(arena) {
                        self.collect_block_diagrams(
                            arena,
                            source,
                            path,
                            child,
                            paragraph_style,
                            jobs,
                        )?;
                    }
                } else if rushdown::matches_extension_kind!(arena, node_ref, DefinitionList) {
                    for child in arena[node_ref].children(arena) {
                        if !rushdown::matches_extension_kind!(arena, child, TermDefinition) {
                            continue;
                        }
                        for def_child in arena[child].children(arena) {
                            self.collect_block_diagrams(
                                arena,
                                source,
                                path,
                                def_child,
                                Some("DefinitionBody"),
                                jobs,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
