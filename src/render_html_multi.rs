use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::code_block_attribute::{
    code_block_attribute_html_renderer_extension, CodeBlockAttributeHtmlRendererOptions,
};
use crate::diagram::{diagram_html_renderer_extension, DiagramHtmlRendererOptions, DiagramThemes};
use crate::graph::resolve::{heading_text, resolve_base_path};
use crate::graph::{DocumentGraph, NodeLocation, PreparedDocument};
use crate::render_html_common::{
    heading_text_html, html_escape, is_admonition_title_heading, render_image_attributes,
    SuppressDocumentRenderer, SuppressFootnoteDefinitionRenderer,
};
use anyhow::{bail, Context, Result};
use rushdown::as_extension_data;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, Heading, Image, KindData, Link, NodeRef, WalkStatus};
use rushdown::context::{ContextKey, ContextKeyRegistry, NodeRefValue, UsizeValue};
use rushdown::renderer;
use rushdown::renderer::html::{self, Renderer, RendererExtension, RendererExtensionFn};
use rushdown::renderer::{
    BoxRenderNode, NodeRenderer, NodeRendererRegistry, PostRender, PreRender, Render, RenderNode,
    TextWrite,
};
use rushdown::util::{escape_html, escape_url, EscapeUrlOptions};
use rushdown_definition_list::definition_list_html_renderer_extension;
use rushdown_fenced_div::{FencedDiv, FencedDivHtmlRendererOptions};
use rushdown_footnote::FootnoteReference;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Page plan
// ---------------------------------------------------------------------------

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
        if let KindData::Heading(_) = doc.arena[child].kind_data() {
            let text = heading_text(&doc.arena, child, &doc.source);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Navigation link type for the Tera template
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct NavLink {
    pub href: String,
    pub title: String,
}

// ---------------------------------------------------------------------------
// TOC builders
// ---------------------------------------------------------------------------

/// Build a page-level TOC tree (collapsed, no internal headings).
/// Each page is one `<li>` entry; the tree structure follows the include
/// hierarchy. The current page's entry gets class="current".
/// All hrefs are relative to `current_output_path`.
pub(crate) fn build_page_toc(
    plan: &MultiPagePlan,
    current_page: usize,
    current_output_path: &Path,
) -> String {
    let mut html = String::new();
    for page in &plan.pages {
        if page.parent_index.is_none() {
            build_page_toc_node(
                &mut html,
                plan,
                page.doc_id,
                current_page,
                current_output_path,
            );
        }
    }
    html
}

fn build_page_toc_node(
    html: &mut String,
    plan: &MultiPagePlan,
    doc_id: usize,
    current_page: usize,
    current_output_path: &Path,
) {
    let page_idx = plan.doc_id_to_page[&doc_id];
    let page = &plan.pages[page_idx];
    let href = compute_relative_href(current_output_path, &page.output_path);
    let is_current = page_idx == current_page;
    let current_class = if is_current { " class=\"current\"" } else { "" };

    html.push_str(&format!(
        "<li{}><a href=\"{}\">{}</a>\n",
        current_class,
        html_escape(&href),
        html_escape(&page.title),
    ));

    if !page.children_indices.is_empty() {
        html.push_str("<ul>\n");
        for &child_idx in &page.children_indices {
            let child_doc_id = plan.pages[child_idx].doc_id;
            build_page_toc_node(html, plan, child_doc_id, current_page, current_output_path);
        }
        html.push_str("</ul>\n");
    }

    html.push_str("</li>\n");
}

/// Build the expanded TOC: same page-level tree, but the current page's
/// node is expanded to show its internal headings.
/// All hrefs are relative to `current_output_path`.
pub(crate) fn build_page_toc_expanded(
    plan: &MultiPagePlan,
    current_page: usize,
    graph: &DocumentGraph,
    current_output_path: &Path,
) -> String {
    let mut html = String::new();
    html.push_str("<ul>\n");
    for page in &plan.pages {
        if page.parent_index.is_none() {
            build_expanded_toc_node(
                &mut html,
                plan,
                page.doc_id,
                current_page,
                graph,
                current_output_path,
            );
        }
    }
    html.push_str("</ul>\n");
    html
}

fn build_expanded_toc_node(
    html: &mut String,
    plan: &MultiPagePlan,
    doc_id: usize,
    current_page: usize,
    graph: &DocumentGraph,
    current_output_path: &Path,
) {
    let page_idx = plan.doc_id_to_page[&doc_id];
    let page = &plan.pages[page_idx];
    let href = compute_relative_href(current_output_path, &page.output_path);
    let is_current = page_idx == current_page;
    let current_class = if is_current { " class=\"current\"" } else { "" };

    html.push_str(&format!(
        "<li{}><a href=\"{}\">{}</a>\n",
        current_class,
        html_escape(&href),
        html_escape(&page.title),
    ));

    // Render internal headings for this page.
    // Skip the first heading — it duplicates the page-level entry above.
    let all_headings = collect_page_headings(graph, doc_id);
    let headings: Vec<_> = all_headings.into_iter().skip(1).collect();
    let has_headings = !headings.is_empty();
    let has_children = !page.children_indices.is_empty();

    // Open a single <ul> for both internal headings and child pages.
    if has_headings || has_children {
        html.push_str("<ul>\n");
    }

    if has_headings {
        let mut current_level = 0usize;
        let mut li_open = false;
        for (level, id, text) in &headings {
            let level = *level;
            if level > current_level {
                while current_level < level {
                    if current_level > 0 {
                        html.push_str("<ul>\n");
                    }
                    current_level += 1;
                }
            } else if level < current_level {
                if li_open {
                    html.push_str("</li>\n");
                }
                while current_level > level {
                    html.push_str("</ul>\n</li>\n");
                    current_level -= 1;
                }
            } else if li_open {
                html.push_str("</li>\n");
            }
            // Heading links point to the page with a fragment.
            html.push_str(&format!(
                "<li><a href=\"{}#{}\">{}</a>\n",
                html_escape(&href),
                html_escape(id),
                text,
            ));
            li_open = true;
        }
        if li_open {
            html.push_str("</li>\n");
        }
        while current_level > 1 {
            html.push_str("</ul>\n</li>\n");
            current_level -= 1;
        }
    }

    // Recurse into child pages (always rendered; CSS controls visibility).
    for &child_idx in &page.children_indices {
        let child_doc_id = plan.pages[child_idx].doc_id;
        build_expanded_toc_node(
            html,
            plan,
            child_doc_id,
            current_page,
            graph,
            current_output_path,
        );
    }

    if has_headings || has_children {
        html.push_str("</ul>\n");
    }

    html.push_str("</li>\n");
}

/// Build the intra-page TOC: headings within the current page only,
/// excluding the page's own title heading (the first one).
/// Returns a nested `<ul>/<li>/<a>` HTML string suitable for a right-side
/// "on this page" sidebar.
pub(crate) fn build_intra_page_toc(graph: &DocumentGraph, doc_id: usize) -> String {
    let all_headings = collect_page_headings(graph, doc_id);
    // Skip the first heading — it is the page title.
    let headings: Vec<_> = all_headings.into_iter().skip(1).collect();
    if headings.is_empty() {
        return String::new();
    }

    let mut html = String::new();
    html.push_str("<ul>\n");

    let base_level = headings.iter().map(|(l, _, _)| *l).min().unwrap_or(2);
    let mut current_level = 0usize;
    let mut li_open = false;

    for (level, id, text) in &headings {
        // Normalise levels so the shallowest heading is depth 1.
        let level = level.saturating_sub(base_level) + 1;

        if level > current_level {
            while current_level < level {
                if current_level > 0 {
                    html.push_str("<ul>\n");
                }
                current_level += 1;
            }
        } else if level < current_level {
            if li_open {
                html.push_str("</li>\n");
            }
            while current_level > level {
                html.push_str("</ul>\n</li>\n");
                current_level -= 1;
            }
        } else if li_open {
            html.push_str("</li>\n");
        }

        html.push_str(&format!(
            "<li><a href=\"#{}\">{}</a>\n",
            html_escape(id),
            text,
        ));
        li_open = true;
    }

    if li_open {
        html.push_str("</li>\n");
    }
    while current_level > 1 {
        html.push_str("</ul>\n</li>\n");
        current_level -= 1;
    }

    html.push_str("</ul>\n");
    html
}

/// Collect headings for a single page (not recursing into includes).
/// Returns `(level, id, text_html)` tuples.
fn collect_page_headings(graph: &DocumentGraph, doc_id: usize) -> Vec<(usize, String, String)> {
    let doc = graph.document(doc_id);
    let mut headings = Vec::new();
    for child in doc.arena[doc.doc_ref].children(&doc.arena) {
        collect_headings_recursive(graph, doc_id, child, &mut headings);
    }
    headings
}

fn collect_headings_recursive(
    graph: &DocumentGraph,
    doc_id: usize,
    node_ref: NodeRef,
    headings: &mut Vec<(usize, String, String)>,
) {
    let doc = graph.document(doc_id);

    // Skip include fenced divs entirely.
    if graph.include_entries(doc_id, node_ref).is_some() {
        return;
    }

    if let KindData::Heading(heading) = doc.arena[node_ref].kind_data() {
        // Skip headings used as admonition titles.
        if is_admonition_title_heading(&doc.arena, node_ref, &doc.source) {
            return;
        }
        let classes = doc.arena[node_ref]
            .attributes()
            .get("class")
            .map(|v| v.str(&doc.source))
            .unwrap_or_default();
        let skip = classes
            .split_whitespace()
            .any(|c| c == "unlisted" || c == "unnumbered");
        if !skip {
            let id = doc.arena[node_ref]
                .attributes()
                .get("id")
                .map(|v| v.str(&doc.source).to_string())
                .unwrap_or_default();
            let level = heading.level() as usize;
            let text = heading_text_html(&doc.arena, node_ref, &doc.source);
            headings.push((level, id, text));
        }
    }

    for child in doc.arena[node_ref].children(&doc.arena) {
        collect_headings_recursive(graph, doc_id, child, headings);
    }
}

// ---------------------------------------------------------------------------
// Context key for the current doc_id
// ---------------------------------------------------------------------------

const MULTI_CURRENT_DOC_ID: &str = "docbake-multi-current-doc-id";
const MULTI_SKIP_ADMONITION_HEADING: &str = "docbake-multi-skip-admonition-heading";

#[derive(Clone, Copy)]
struct MultiRenderKeys {
    current_doc_id: ContextKey<UsizeValue>,
}

impl MultiRenderKeys {
    fn new(reg: Rc<RefCell<ContextKeyRegistry>>) -> Self {
        let mut reg = reg.borrow_mut();
        Self {
            current_doc_id: reg.get_or_create::<UsizeValue>(MULTI_CURRENT_DOC_ID),
        }
    }
}

// ---------------------------------------------------------------------------
// Footnote plan (per-page scope)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FootnoteEntry {
    label: String,
    index: usize,
    reference_indices: Vec<usize>,
}

#[derive(Clone, Default)]
struct FootnotePlan {
    refs: HashMap<NodeLocation, (usize, usize)>,
    ordered: Vec<FootnoteEntry>,
}

fn build_page_footnote_plan(graph: &DocumentGraph, doc_id: usize) -> Result<FootnotePlan> {
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

// ---------------------------------------------------------------------------
// Rushdown renderer hooks for multi-page rendering
// ---------------------------------------------------------------------------

/// PreRender hook: renders only the current document's own children,
/// skipping :::include fenced divs.
struct MultiPageRenderHook<'a> {
    graph: &'a DocumentGraph,
    doc_id: usize,
    keys: MultiRenderKeys,
}

impl<'a, W: TextWrite> PreRender<W> for MultiPageRenderHook<'a> {
    fn pre_render(
        &self,
        writer: &mut W,
        _source: &str,
        _arena: &Arena,
        _node_ref: NodeRef,
        render: &dyn Render<W>,
        context: &mut renderer::Context,
    ) -> rushdown::Result<()> {
        let doc = self.graph.document(self.doc_id);
        context.insert(self.keys.current_doc_id, self.doc_id);
        for child in doc.arena[doc.doc_ref].children(&doc.arena) {
            // Skip include blocks.
            if self.graph.include_entries(self.doc_id, child).is_some() {
                continue;
            }
            render.render(writer, &doc.source, &doc.arena, child, context)?;
        }
        Ok(())
    }
}

/// Heading renderer — no heading shift in multi-page mode (each page uses
/// original levels).
struct MultiHeadingHtmlRenderer<W: TextWrite> {
    _phantom: std::marker::PhantomData<W>,
    writer: html::Writer,
    skip_heading_key: ContextKey<NodeRefValue>,
}

impl<W: TextWrite> MultiHeadingHtmlRenderer<W> {
    fn new(options: html::Options, reg: Rc<RefCell<ContextKeyRegistry>>) -> Self {
        let skip_heading_key = reg
            .borrow_mut()
            .get_or_create::<NodeRefValue>(MULTI_SKIP_ADMONITION_HEADING);
        Self {
            _phantom: std::marker::PhantomData,
            writer: html::Writer::with_options(options),
            skip_heading_key,
        }
    }
}

impl<W: TextWrite> RenderNode<W> for MultiHeadingHtmlRenderer<W> {
    fn render_node<'a>(
        &self,
        w: &mut W,
        source: &'a str,
        arena: &'a Arena,
        node_ref: NodeRef,
        entering: bool,
        ctx: &mut renderer::Context,
    ) -> rushdown::Result<WalkStatus> {
        let heading = as_kind_data!(arena, node_ref, Heading);
        // Skip headings that were already rendered as admonition titles.
        if let Some(&skip_ref) = ctx.get(self.skip_heading_key) {
            if skip_ref == node_ref {
                if !entering {
                    ctx.remove(self.skip_heading_key);
                }
                return Ok(WalkStatus::SkipChildren);
            }
        }
        let level = heading.level() as usize;
        if entering {
            w.write_str(&format!("<h{level}"))?;
            html::render_attributes(w, source, arena[node_ref].attributes(), None)?;
            self.writer.write_safe_str(w, ">")?;
        } else {
            w.write_str(&format!("</h{level}>\n"))?;
        }
        Ok(WalkStatus::Continue)
    }
}

impl<'cb, W> NodeRenderer<'cb, W> for MultiHeadingHtmlRenderer<W>
where
    W: TextWrite + 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<Heading>(), BoxRenderNode::new(self));
    }
}

/// Link renderer for multi-page: rewrites .md targets to .html relative paths.
struct MultiLinkHtmlRenderer<'a, W: TextWrite> {
    graph: &'a DocumentGraph,
    plan: &'a MultiPagePlan,
    current_page: usize,
    base_dir: &'a Path,
    _phantom: std::marker::PhantomData<W>,
    writer: html::Writer,
    format_options: html::Options,
}

impl<'a, W: TextWrite> RenderNode<W> for MultiLinkHtmlRenderer<'a, W> {
    fn render_node<'b>(
        &self,
        w: &mut W,
        source: &'b str,
        arena: &'b Arena,
        node_ref: NodeRef,
        entering: bool,
        _ctx: &mut renderer::Context,
    ) -> rushdown::Result<WalkStatus> {
        if entering {
            let link = as_kind_data!(arena, node_ref, Link);
            let raw_dest = link.destination_str(source);
            let target = self
                .rewrite_link(node_ref, raw_dest)
                .map_err(|e| rushdown::Error::io(format!("link rewrite error: {e}"), None))?;
            let mut dest = escape_url(
                target.as_bytes(),
                &EscapeUrlOptions {
                    resolves_refs: !matches!(link.link_kind(), rushdown::ast::LinkKind::Auto(_)),
                    ..EscapeUrlOptions::for_url()
                },
            );
            self.writer.write_safe_str(w, "<a href=\"")?;
            if self.format_options.allows_unsafe || !html::is_dangerous_url(&dest) {
                dest = escape_html(dest);
                w.write_str(&String::from_utf8_lossy(&dest))?;
            }
            self.writer.write_safe_str(w, "\"")?;
            if let Some(title) = link.title() {
                self.writer.write_safe_str(w, " title=\"")?;
                self.writer.write(w, &title.str(source))?;
                self.writer.write_safe_str(w, "\"")?;
            }
            html::render_attributes(w, source, arena[node_ref].attributes(), None)?;
            self.writer.write_safe_str(w, ">")?;
        } else {
            self.writer.write_safe_str(w, "</a>")?;
        }
        Ok(WalkStatus::Continue)
    }
}

impl<'a, W: TextWrite> MultiLinkHtmlRenderer<'a, W> {
    fn rewrite_link(&self, source_node_ref: NodeRef, raw_target: &str) -> Result<String> {
        let target = raw_target.trim();
        let wrapped = target.starts_with('<') && target.ends_with('>');
        let target = target.trim_matches(['<', '>']);
        let source_doc_id = self.plan.pages[self.current_page].doc_id;
        let graph_target = self
            .graph
            .link_target(source_doc_id, source_node_ref)
            .unwrap_or(raw_target);

        // Pass through fragment-only, empty, URL, and mailto links.
        if target.is_empty()
            || target.starts_with('#')
            || target.contains("://")
            || target.starts_with("mailto:")
        {
            return Ok(graph_target.to_string());
        }

        let (path_part, fragment) = match target.split_once('#') {
            Some((p, f)) => (p, Some(f)),
            None => (target, None),
        };

        if Path::new(path_part).extension().and_then(|e| e.to_str()) != Some("md") {
            return Ok(graph_target.to_string());
        }

        if graph_target == raw_target {
            return Ok(raw_target.to_string());
        }

        // Check if this is a markdown file link.
        let source_doc = self
            .graph
            .document(self.plan.pages[self.current_page].doc_id);
        let source_dir = source_doc.path.parent().unwrap_or(self.base_dir);

        let resolved = match resolve_base_path(self.base_dir, source_dir, path_part) {
            Ok(path) => path,
            Err(_) => return Ok(graph_target.to_string()),
        };

        // Check if it resolves to a markdown file in the graph.
        let target_doc_id = match self.graph.resolve_markdown_target(source_doc_id, &resolved) {
            Ok(doc_id) => doc_id,
            Err(_) => return Ok(graph_target.to_string()),
        };
        let target_page = self.plan.page_for_doc(target_doc_id).ok_or_else(|| {
            anyhow::anyhow!(
                "target document {} has no page entry in multipage plan",
                resolved.display()
            )
        })?;
        let current_page = &self.plan.pages[self.current_page];
        let relative = compute_relative_href(&current_page.output_path, &target_page.output_path);
        let result = if let Some(frag) = fragment {
            let final_id = match self.graph.anchor_target(target_doc_id, frag) {
                Some(id) => id,
                None => return Ok(graph_target.to_string()),
            };
            format!("{relative}#{final_id}")
        } else {
            relative
        };

        Ok(if wrapped {
            format!("<{result}>")
        } else {
            result
        })
    }
}

impl<'a, 'cb, W> NodeRenderer<'cb, W> for MultiLinkHtmlRenderer<'a, W>
where
    W: TextWrite + 'cb,
    'a: 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<Link>(), BoxRenderNode::new(self));
    }
}

/// Image renderer for multi-page — uses graph-resolved targets.
struct MultiImageHtmlRenderer<'a, W: TextWrite> {
    graph: &'a DocumentGraph,
    plan: &'a MultiPagePlan,
    current_page: usize,
    /// Prefix to prepend to static asset paths (e.g. `../` for root-level
    /// pages, `../../` for pages one level deep).
    asset_prefix: &'a str,
    _phantom: std::marker::PhantomData<W>,
    writer: html::Writer,
    format_options: html::Options,
}

impl<'a, W: TextWrite> MultiImageHtmlRenderer<'a, W> {
    fn render_texts<'b>(
        &self,
        w: &mut W,
        source: &'b str,
        arena: &'b Arena,
        node_ref: NodeRef,
        _ctx: &mut renderer::Context,
    ) -> rushdown::Result<()> {
        for child in arena[node_ref].children(arena) {
            match arena[child].kind_data() {
                KindData::Text(text) => self.writer.write(w, text.str(source))?,
                KindData::CodeSpan(cs) => self.writer.write(w, &cs.value_str(source))?,
                _ => self.render_texts(w, source, arena, child, _ctx)?,
            }
        }
        Ok(())
    }
}

impl<'a, W: TextWrite> RenderNode<W> for MultiImageHtmlRenderer<'a, W> {
    fn render_node<'b>(
        &self,
        w: &mut W,
        source: &'b str,
        arena: &'b Arena,
        node_ref: NodeRef,
        entering: bool,
        ctx: &mut renderer::Context,
    ) -> rushdown::Result<WalkStatus> {
        if entering {
            let image = as_kind_data!(arena, node_ref, Image);
            let doc_id = self.plan.pages[self.current_page].doc_id;
            let raw_target = self.graph.image_target(doc_id, node_ref).ok_or_else(|| {
                rushdown::Error::io(
                    format!(
                        "missing resolved image target for `{}` in {}",
                        image.destination_str(source),
                        self.graph.document(doc_id).path.display()
                    ),
                    None,
                )
            })?;
            // Prepend the asset prefix to make the path relative to this page.
            let target = format!("{}{}", self.asset_prefix, raw_target);
            let mut dest = escape_url(
                target.as_bytes(),
                &EscapeUrlOptions {
                    resolves_refs: true,
                    ..EscapeUrlOptions::for_url()
                },
            );
            self.writer.write_safe_str(w, "<img src=\"")?;
            if self.format_options.allows_unsafe || !html::is_dangerous_url(&dest) {
                dest = escape_html(dest);
                w.write_str(&String::from_utf8_lossy(&dest))?;
            }
            self.writer.write_safe_str(w, "\" alt=\"")?;
            self.render_texts(w, source, arena, node_ref, ctx)?;
            self.writer.write_safe_str(w, "\"")?;
            if let Some(title) = image.title() {
                self.writer.write_safe_str(w, " title=\"")?;
                self.writer.write(w, &title.str(source))?;
                self.writer.write_safe_str(w, "\"")?;
            }
            render_image_attributes(w, source, arena, node_ref, &self.format_options)?;
        }
        Ok(WalkStatus::SkipChildren)
    }
}

impl<'a, 'cb, W> NodeRenderer<'cb, W> for MultiImageHtmlRenderer<'a, W>
where
    W: TextWrite + 'cb,
    'a: 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<Image>(), BoxRenderNode::new(self));
    }
}

/// Footnote reference renderer (per-page).
struct MultiFootnoteReferenceHtmlRenderer<W: TextWrite> {
    _phantom: std::marker::PhantomData<W>,
    keys: MultiRenderKeys,
    footnotes: FootnotePlan,
}

impl<W: TextWrite> RenderNode<W> for MultiFootnoteReferenceHtmlRenderer<W> {
    fn render_node<'a>(
        &self,
        w: &mut W,
        _source: &'a str,
        arena: &'a Arena,
        node_ref: NodeRef,
        entering: bool,
        ctx: &mut renderer::Context,
    ) -> rushdown::Result<WalkStatus> {
        let _ = as_extension_data!(arena, node_ref, FootnoteReference);
        if !entering {
            return Ok(WalkStatus::SkipChildren);
        }
        let doc_id = ctx.get(self.keys.current_doc_id).copied().unwrap_or(0);
        let (index, ref_index) = self
            .footnotes
            .refs
            .get(&NodeLocation::new(doc_id, node_ref))
            .copied()
            .ok_or_else(|| rushdown::Error::io("missing footnote render plan", None))?;
        w.write_str(&format!(
            "<sup id=\"fnref:{ref_index}\"><a href=\"#fn:{index}\" class=\"footnote-ref\" role=\"doc-noteref\">{index}</a></sup>"
        ))?;
        Ok(WalkStatus::SkipChildren)
    }
}

impl<'cb, W> NodeRenderer<'cb, W> for MultiFootnoteReferenceHtmlRenderer<W>
where
    W: TextWrite + 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<FootnoteReference>(), BoxRenderNode::new(self));
    }
}

/// PostRender hook: emit footnotes section at the end.
struct MultiFootnotePostRenderHook<'a> {
    graph: &'a DocumentGraph,
    doc_id: usize,
    keys: MultiRenderKeys,
    footnotes: FootnotePlan,
    writer: html::Writer,
}

impl<'a, W: TextWrite> PostRender<W> for MultiFootnotePostRenderHook<'a> {
    fn post_render(
        &self,
        w: &mut W,
        _source: &str,
        _arena: &Arena,
        _node_ref: NodeRef,
        render: &dyn Render<W>,
        ctx: &mut renderer::Context,
    ) -> rushdown::Result<()> {
        if self.footnotes.ordered.is_empty() {
            return Ok(());
        }

        let doc = self.graph.document(self.doc_id);
        self.writer.write_safe_str(
            w,
            "<div class=\"footnotes\" role=\"doc-endnotes\">\n<hr>\n<ol>\n",
        )?;
        for footnote in &self.footnotes.ordered {
            let definition = doc
                .footnote_definition(&footnote.label)
                .ok_or_else(|| rushdown::Error::io("missing footnote body", None))?;
            w.write_str(&format!("<li id=\"fn:{}\">", footnote.index))?;
            ctx.insert(self.keys.current_doc_id, self.doc_id);
            for child in doc.arena[definition.node_ref].children(&doc.arena) {
                render.render(w, &doc.source, &doc.arena, child, ctx)?;
            }
            for ref_index in &footnote.reference_indices {
                w.write_str(&format!(
                    "&#160;<a href=\"#fnref:{ref_index}\" class=\"footnote-backref\" role=\"doc-backlink\">\u{21a9}</a>"
                ))?;
            }
            self.writer.write_safe_str(w, "</li>\n")?;
        }
        self.writer.write_safe_str(w, "</ol>\n</div>\n")?;
        Ok(())
    }
}

/// Fenced div renderer that filters by format (same as single-page).
struct MultiFormatFencedDivHtmlRenderer<W: TextWrite> {
    _phantom: std::marker::PhantomData<W>,
    writer: html::Writer,
    skip_heading_key: ContextKey<NodeRefValue>,
}

impl<W: TextWrite> MultiFormatFencedDivHtmlRenderer<W> {
    fn with_options(options: html::Options, reg: Rc<RefCell<ContextKeyRegistry>>) -> Self {
        let skip_heading_key = reg
            .borrow_mut()
            .get_or_create::<NodeRefValue>(MULTI_SKIP_ADMONITION_HEADING);
        Self {
            _phantom: std::marker::PhantomData,
            writer: html::Writer::with_options(options),
            skip_heading_key,
        }
    }
}

impl<W: TextWrite> RenderNode<W> for MultiFormatFencedDivHtmlRenderer<W> {
    fn render_node<'a>(
        &self,
        w: &mut W,
        source: &'a str,
        arena: &'a Arena,
        node_ref: NodeRef,
        entering: bool,
        ctx: &mut renderer::Context,
    ) -> rushdown::Result<WalkStatus> {
        let _ = as_extension_data!(arena, node_ref, FencedDiv);
        let classes = arena[node_ref]
            .attributes()
            .get("class")
            .map(|value| value.str(source))
            .unwrap_or_default();

        // Skip include blocks entirely.
        if classes.split_whitespace().any(|c| c == "include") {
            return Ok(WalkStatus::SkipChildren);
        }

        let known_formats = crate::KNOWN_OUTPUT_FORMATS;
        let present_formats = classes
            .split_whitespace()
            .filter(|class_name| known_formats.contains(class_name))
            .collect::<Vec<_>>();
        if !present_formats.is_empty() && !present_formats.contains(&"html") {
            return Ok(WalkStatus::SkipChildren);
        }

        let is_admonition = crate::admonition_type(&classes).is_some();

        if entering {
            if is_admonition {
                w.write_str("<div class=\"admonition ")?;
                let esc = escape_html(classes.as_bytes());
                w.write_str(std::str::from_utf8(&esc).unwrap_or_default())?;
                w.write_str("\"")?;
                for (key, value) in arena[node_ref].attributes().iter() {
                    if key == "class" {
                        continue;
                    }
                    w.write_str(" ")?;
                    w.write_str(key)?;
                    w.write_str("=\"")?;
                    let esc = escape_html(value.bytes(source));
                    w.write_str(std::str::from_utf8(&esc).unwrap_or_default())?;
                    w.write_str("\"")?;
                }
                w.write_str(">")?;

                // If the first child is a heading, render it as a title
                // element instead of a real heading.
                if let Some(first_child) = arena[node_ref].children(arena).next() {
                    if matches!(arena[first_child].kind_data(), KindData::Heading(_)) {
                        let title_html = heading_text_html(arena, first_child, source);
                        w.write_str("<p class=\"admonition-title\">")?;
                        w.write_str(&title_html)?;
                        w.write_str("</p>\n")?;
                        ctx.insert(self.skip_heading_key, first_child);
                    }
                }
            } else {
                self.writer.write_safe_str(w, "<div")?;
                html::render_attributes(w, source, arena[node_ref].attributes(), None)?;
                self.writer.write_safe_str(w, ">")?;
            }
        } else {
            self.writer.write_safe_str(w, "</div>")?;
        }
        Ok(WalkStatus::Continue)
    }
}

impl<'cb, W> NodeRenderer<'cb, W> for MultiFormatFencedDivHtmlRenderer<W>
where
    W: TextWrite + 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<FencedDiv>(), BoxRenderNode::new(self));
    }
}

// ---------------------------------------------------------------------------
// Extension composition
// ---------------------------------------------------------------------------

fn multi_format_fenced_div_renderer<'cb, W>() -> impl RendererExtension<'cb, W>
where
    W: TextWrite + 'cb,
{
    RendererExtensionFn::new(move |renderer: &mut Renderer<'cb, W>| {
        renderer.add_node_renderer(
            |options: html::Options, reg: Rc<RefCell<ContextKeyRegistry>>| {
                MultiFormatFencedDivHtmlRenderer::with_options(options, reg)
            },
            rushdown::renderer::NoRendererOptions,
        );
    })
}

fn multi_page_renderer_extension<'cb, W>(
    graph: &'cb DocumentGraph,
    plan: &'cb MultiPagePlan,
    current_page: usize,
    base_dir: &'cb Path,
    asset_prefix: &'cb str,
    footnotes: FootnotePlan,
) -> impl RendererExtension<'cb, W>
where
    W: TextWrite + 'cb,
{
    let doc_id = plan.pages[current_page].doc_id;
    let ref_footnotes = footnotes.clone();
    let post_footnotes = footnotes.clone();

    RendererExtensionFn::new(move |renderer: &mut Renderer<'cb, W>| {
        renderer.add_pre_render_hook(
            move |reg: Rc<RefCell<ContextKeyRegistry>>| MultiPageRenderHook {
                graph,
                doc_id,
                keys: MultiRenderKeys::new(reg),
            },
            rushdown::renderer::NoRendererOptions,
            1000,
        );
        renderer.add_node_renderer(
            || SuppressDocumentRenderer,
            rushdown::renderer::NoRendererOptions,
        );
        renderer.add_node_renderer(
            move |options: html::Options, reg: Rc<RefCell<ContextKeyRegistry>>| {
                MultiHeadingHtmlRenderer::new(options, reg)
            },
            rushdown::renderer::NoRendererOptions,
        );
        renderer.add_node_renderer(
            move |options: html::Options| MultiLinkHtmlRenderer {
                graph,
                plan,
                current_page,
                base_dir,
                _phantom: std::marker::PhantomData,
                writer: html::Writer::with_options(options.clone()),
                format_options: options,
            },
            rushdown::renderer::NoRendererOptions,
        );
        renderer.add_node_renderer(
            move |options: html::Options| MultiImageHtmlRenderer {
                graph,
                plan,
                current_page,
                asset_prefix,
                _phantom: std::marker::PhantomData,
                writer: html::Writer::with_options(options.clone()),
                format_options: options,
            },
            rushdown::renderer::NoRendererOptions,
        );
        let ref_fn = ref_footnotes.clone();
        renderer.add_node_renderer(
            move |_options: html::Options, reg: Rc<RefCell<ContextKeyRegistry>>| {
                MultiFootnoteReferenceHtmlRenderer {
                    _phantom: std::marker::PhantomData,
                    keys: MultiRenderKeys::new(reg),
                    footnotes: ref_fn.clone(),
                }
            },
            rushdown::renderer::NoRendererOptions,
        );
        renderer.add_node_renderer(
            || SuppressFootnoteDefinitionRenderer,
            rushdown::renderer::NoRendererOptions,
        );
        let post_fn = post_footnotes.clone();
        renderer.add_post_render_hook(
            move |options: html::Options, reg: Rc<RefCell<ContextKeyRegistry>>| {
                MultiFootnotePostRenderHook {
                    graph,
                    doc_id,
                    keys: MultiRenderKeys::new(reg),
                    footnotes: post_fn.clone(),
                    writer: html::Writer::with_options(options),
                }
            },
            rushdown::renderer::NoRendererOptions,
            500,
        );
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Rendered page output.
pub(crate) struct RenderedPage {
    /// Output path relative to the document output root.
    pub path: PathBuf,
    /// Full HTML content.
    pub html: String,
}

/// Build all pages for multi-page HTML output.
pub(crate) fn build_multi_page_html(
    template: &str,
    context: &tera::Context,
    prepared: &PreparedDocument,
    base_dir: &Path,
    cache: &crate::cache::Cache,
    diagram_themes: &DiagramThemes,
) -> Result<Vec<RenderedPage>> {
    let graph = prepared.graph();
    let root_doc = graph.root_document();
    let root_source_dir = root_doc
        .path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("root document has no parent directory"))?;

    let plan = build_multi_page_plan(prepared, root_source_dir, base_dir)?;

    let mut pages = Vec::new();

    for page_idx in 0..plan.pages.len() {
        let page_info = &plan.pages[page_idx];
        let doc_id = page_info.doc_id;
        let doc = graph.document(doc_id);

        // Compute relative prefix from this page back to the output root.
        let asset_prefix = compute_asset_prefix(&page_info.output_path);

        // Build body HTML.
        let footnotes = build_page_footnote_plan(graph, doc_id)?;
        let mut body = String::new();
        let renderer = html::Renderer::with_extensions(
            html::Options::default(),
            rushdown_fenced_div::fenced_div_html_renderer_extension(FencedDivHtmlRendererOptions)
                .and(code_block_attribute_html_renderer_extension(
                    CodeBlockAttributeHtmlRendererOptions,
                ))
                .and(definition_list_html_renderer_extension())
                .and(crate::bracketed_span::bracketed_span_html_renderer_extension())
                .and(crate::caption::caption_html_renderer_extension())
                .and(multi_format_fenced_div_renderer())
                .and(diagram_html_renderer_extension(
                    DiagramHtmlRendererOptions {
                        themes: diagram_themes.clone(),
                        cache: cache.clone(),
                        ..Default::default()
                    },
                ))
                .and(multi_page_renderer_extension(
                    graph,
                    &plan,
                    page_idx,
                    base_dir,
                    &asset_prefix,
                    footnotes,
                )),
        );
        renderer
            .render(&mut body, &doc.source, &doc.arena, doc.doc_ref)
            .map_err(|error| anyhow::anyhow!("failed to render HTML body: {error:?}"))?;

        // Build TOC variants.
        let toc = build_page_toc(&plan, page_idx, &page_info.output_path);
        let toc_expanded = build_page_toc_expanded(&plan, page_idx, graph, &page_info.output_path);
        let toc_page = build_intra_page_toc(graph, page_info.doc_id);

        // Build nav links.
        let prev = page_info.prev_index.map(|i| {
            let p = &plan.pages[i];
            NavLink {
                href: compute_relative_href(&page_info.output_path, &p.output_path),
                title: p.title.clone(),
            }
        });
        let next = page_info.next_index.map(|i| {
            let p = &plan.pages[i];
            NavLink {
                href: compute_relative_href(&page_info.output_path, &p.output_path),
                title: p.title.clone(),
            }
        });
        let up = page_info.parent_index.map(|i| {
            let p = &plan.pages[i];
            NavLink {
                href: compute_relative_href(&page_info.output_path, &p.output_path),
                title: p.title.clone(),
            }
        });

        // Root href (relative path from current page to root index).
        let root_href = compute_relative_href(&page_info.output_path, &plan.pages[0].output_path);

        // Build breadcrumbs: walk parent chain from current page up to root, then reverse.
        let mut breadcrumbs: Vec<NavLink> = Vec::new();
        {
            // Current page (last crumb, no link needed — template renders it as plain text).
            breadcrumbs.push(NavLink {
                href: String::new(),
                title: page_info.title.clone(),
            });
            let mut idx = page_idx;
            while let Some(parent_idx) = plan.pages[idx].parent_index {
                let p = &plan.pages[parent_idx];
                breadcrumbs.push(NavLink {
                    href: compute_relative_href(&page_info.output_path, &p.output_path),
                    title: p.title.clone(),
                });
                idx = parent_idx;
            }
            breadcrumbs.reverse();
        }

        // Assemble Tera context.
        let mut page_ctx = context.clone();
        page_ctx.insert("asset_prefix", &asset_prefix);
        page_ctx.insert("body", &body);
        page_ctx.insert("toc", &toc);
        page_ctx.insert("toc_expanded", &toc_expanded);
        page_ctx.insert("toc_page", &toc_page);
        page_ctx.insert("page_title", &page_info.title);
        page_ctx.insert("root_href", &root_href);
        page_ctx.insert("breadcrumbs", &breadcrumbs);
        if let Some(ref prev) = prev {
            page_ctx.insert("prev", prev);
        }
        if let Some(ref next) = next {
            page_ctx.insert("next", next);
        }
        if let Some(ref up) = up {
            page_ctx.insert("up", up);
        }

        let html = tera::Tera::one_off(template, &page_ctx, false)
            .context("failed to render multi-page HTML template")?;

        pages.push(RenderedPage {
            path: page_info.output_path.clone(),
            html,
        });
    }

    Ok(pages)
}

// ---------------------------------------------------------------------------
// Path utilities
// ---------------------------------------------------------------------------

/// Compute the relative prefix from a page's output path back to the output
/// root directory.
/// For `index.html` → `""`, for `builder/config.html` → `"../"`.
fn compute_asset_prefix(page_output_path: &Path) -> String {
    let dir_depth = page_output_path
        .parent()
        .map(|p| p.components().count())
        .unwrap_or(0);
    "../".repeat(dir_depth)
}

/// Compute a relative href from `from_path` to `to_path`, where both are
/// relative paths from the same root (the document output directory).
fn compute_relative_href(from: &Path, to: &Path) -> String {
    let from_dir = from.parent().unwrap_or(Path::new(""));
    let to_str = to.to_string_lossy().replace('\\', "/");
    let from_dir_str = from_dir.to_string_lossy().replace('\\', "/");

    if from_dir_str.is_empty() {
        return to_str;
    }

    // Split into components (early return above guarantees non-empty).
    let from_parts: Vec<&str> = from_dir_str.split('/').collect();
    let to_parts: Vec<&str> = to_str.split('/').collect();

    // Find common prefix length.
    let common = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let ups = from_parts.len() - common;
    let mut result = String::new();
    for _ in 0..ups {
        result.push_str("../");
    }
    let remaining = &to_parts[common..];
    result.push_str(&remaining.join("/"));
    if result.is_empty() {
        // Same file.
        to.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    use crate::cache::Cache;
    use crate::cli::BrokenLinkMode;
    use crate::diagram::DiagramThemes;
    use crate::graph::prepare_document;
    use crate::DebugDumper;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    fn write_custom_base_fixture(root: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
        let base_dir = root.path().to_path_buf();
        let root_doc = base_dir.join("docs/index.md");
        let part_doc = base_dir.join("docs/nested/part.md");
        let guide_doc = base_dir.join("shared/guide.md");

        write(
            &root_doc,
            "# Root\n\n[Guide](/shared/guide.md#sec)\n\n:::include\n# [Part](nested/part.md)\n:::\n",
        );
        write(
            &part_doc,
            "# Part\n\n[Guide From Part](/shared/guide.md#sec)\n\n:::include\n# [Guide](/shared/guide.md)\n:::\n",
        );
        write(&guide_doc, "# Guide\n\n## Section {#sec}\n");

        let canonical_base_dir = base_dir.canonicalize().unwrap();
        let canonical_root_doc = root_doc.canonicalize().unwrap();
        let root_source_dir = canonical_root_doc.parent().unwrap().to_path_buf();
        (canonical_base_dir, canonical_root_doc, root_source_dir)
    }

    fn page_html<'a>(pages: &'a [RenderedPage], path: &str) -> &'a str {
        let path = Path::new(path);
        pages
            .iter()
            .find(|page| page.path == path)
            .map(|page| page.html.as_str())
            .unwrap()
    }

    #[test]
    fn relative_href_same_dir() {
        assert_eq!(
            compute_relative_href(Path::new("index.html"), Path::new("document.html")),
            "document.html"
        );
    }

    #[test]
    fn relative_href_into_subdir() {
        assert_eq!(
            compute_relative_href(Path::new("index.html"), Path::new("builder/index.html")),
            "builder/index.html"
        );
    }

    #[test]
    fn relative_href_up_from_subdir() {
        assert_eq!(
            compute_relative_href(Path::new("builder/index.html"), Path::new("index.html")),
            "../index.html"
        );
    }

    #[test]
    fn relative_href_sibling_in_subdir() {
        assert_eq!(
            compute_relative_href(
                Path::new("builder/config.html"),
                Path::new("builder/formats.html")
            ),
            "formats.html"
        );
    }

    #[test]
    fn relative_href_cross_dirs() {
        assert_eq!(
            compute_relative_href(Path::new("builder/config.html"), Path::new("document.html")),
            "../document.html"
        );
    }

    #[test]
    fn plan_handles_docs_outside_root_source_with_custom_base() {
        let temp = TempDir::new().unwrap();
        let (base_dir, root_doc, root_source_dir) = write_custom_base_fixture(&temp);
        let prepared = prepare_document(
            &DebugDumper::new(false),
            &base_dir,
            &root_doc,
            Some("test"),
            BrokenLinkMode::Error,
        )
        .unwrap();

        let plan = build_multi_page_plan(&prepared, &root_source_dir, &base_dir).unwrap();
        let paths: Vec<PathBuf> = plan.pages.iter().map(|p| p.output_path.clone()).collect();

        assert!(paths.contains(&PathBuf::from("index.html")));
        assert!(paths.contains(&PathBuf::from("nested/part.html")));
        assert!(paths.contains(&PathBuf::from("shared/guide.html")));
    }

    #[test]
    fn rewrites_absolute_markdown_links_with_custom_base() {
        let temp = TempDir::new().unwrap();
        let (base_dir, root_doc, _root_source_dir) = write_custom_base_fixture(&temp);
        let prepared = prepare_document(
            &DebugDumper::new(false),
            &base_dir,
            &root_doc,
            Some("test"),
            BrokenLinkMode::Error,
        )
        .unwrap();

        let context = tera::Context::new();
        let cache = Cache::new(None, true).unwrap();
        let pages = build_multi_page_html(
            "{{ body }}",
            &context,
            &prepared,
            &base_dir,
            &cache,
            &DiagramThemes::default(),
        )
        .unwrap();

        let root_html = page_html(&pages, "index.html");
        let part_html = page_html(&pages, "nested/part.html");

        assert!(root_html.contains("href=\"shared/guide.html#sec\""));
        assert!(part_html.contains("href=\"../shared/guide.html#sec\""));
    }
}
