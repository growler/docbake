use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use anyhow::Result;
use rushdown::ast::{Arena, KindData, NodeRef};
use rushdown_fenced_div::FencedDiv;

use crate::caption::{Caption, CaptionText};
use crate::graph::DocumentGraph;

use super::media::DiagramJob;
use super::DocxRenderer;

impl<'a> DocxRenderer<'a> {
    /// Render the diagrams that the body walk embeds and that the artifact
    /// cache does not hold, in parallel, and store them in the cache.
    ///
    /// The body walk then reads these diagrams from the cache.  This step
    /// does not log and ignores all errors: on a cache miss the walk renders
    /// the diagram again and reports the error.  It does nothing when the
    /// cache is disabled.
    pub(super) fn prefetch_diagrams(&self, graph: &DocumentGraph) {
        if !self.cache.is_enabled() {
            return;
        }
        let mut jobs = Vec::new();
        if self
            .collect_graph_diagrams(graph, graph.root_id, &mut jobs)
            .is_err()
        {
            return;
        }
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
    }

    /// Collect the diagram jobs of a document occurrence with the include
    /// and visibility rules of `render_graph_document`.
    fn collect_graph_diagrams(
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
            self.collect_block_diagrams(&doc.arena, &doc.source, child, jobs)?;
        }
        Ok(())
    }

    /// Collect the diagram jobs of a block with the traversal of
    /// `render_block`.
    fn collect_block_diagrams(
        &self,
        arena: &Arena,
        source: &str,
        node_ref: NodeRef,
        jobs: &mut Vec<DiagramJob>,
    ) -> Result<()> {
        match arena[node_ref].kind_data() {
            KindData::Blockquote(_) => {
                for child in arena[node_ref].children(arena) {
                    self.collect_block_diagrams(arena, source, child, jobs)?;
                }
            }
            KindData::List(_) => {
                for item_ref in arena[node_ref].children(arena) {
                    if !matches!(arena[item_ref].kind_data(), KindData::ListItem(_)) {
                        continue;
                    }
                    for child in arena[item_ref].children(arena) {
                        self.collect_block_diagrams(arena, source, child, jobs)?;
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
                        self.collect_block_diagrams(arena, source, br, jobs)?;
                    }
                } else if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::diagram::Diagram
                ) {
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
                    for child in arena[node_ref].children(arena) {
                        self.collect_block_diagrams(arena, source, child, jobs)?;
                    }
                }
            }
        }
        Ok(())
    }
}
