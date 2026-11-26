use std::any::TypeId;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use anyhow::Result;
use rushdown::as_extension_data;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, Image, Link, NodeRef, WalkStatus};
use rushdown::context::{ContextKeyRegistry, NodeRefValue};
use rushdown::renderer;
use rushdown::renderer::html::{self, Renderer, RendererExtension, RendererExtensionFn};
use rushdown::renderer::{
    BoxRenderNode, NodeRenderer, NodeRendererRegistry, RenderNode, RenderOptions, TextWrite,
};
use rushdown_footnote::FootnoteReference;

use crate::conditional::RenderProfile;
use crate::graph::resolve::resolve_base_path;
use crate::graph::{DocumentGraph, NodeLocation};
use crate::html::{
    render_footnote_reference, render_image_tag, render_link_open_tag, ImageRenderData,
    LinkRenderData, RenderState, SharedFormatFencedDivHtmlRenderer, SharedHeadingHtmlRenderer,
    SuppressFootnoteDefinitionRenderer,
};

use super::footnotes::FootnotePlan;
use super::path_utils::compute_relative_href;
use super::plan::MultiPagePlan;

const MULTI_SKIP_ADMONITION_HEADING: &str = "docbake-multi-skip-admonition-heading";

/// Renders the visible children of the page document `doc_id`, without the
/// `:::include` blocks. Included documents are pages of their own.
pub(super) fn render_page_document<W: TextWrite>(
    renderer: &Renderer<'_, W>,
    writer: &mut W,
    graph: &DocumentGraph,
    profile: &RenderProfile,
    state: &RenderState,
    doc_id: usize,
) -> rushdown::Result<()> {
    let doc = graph.document(doc_id);
    state.current_doc_id.set(doc_id);
    for child in doc.arena[doc.doc_ref].children(&doc.arena) {
        let visible = crate::conditional::node_is_visible(&doc.arena, child, &doc.source, profile)
            .map_err(|error| rushdown::Error::io(format!("{error:#}"), None))?;
        if !visible {
            continue;
        }
        if graph.include_entries(doc_id, child).is_some() {
            continue;
        }
        renderer.render(
            writer,
            &doc.source,
            &doc.arena,
            child,
            RenderOptions::default(),
        )?;
    }
    Ok(())
}

/// Link renderer for multi-page: rewrites .md targets to .html relative paths.
struct MultiLinkHtmlRenderer<'a, W: TextWrite> {
    graph: &'a DocumentGraph,
    plan: &'a MultiPagePlan,
    current_page: usize,
    base_dir: &'a Path,
    _phantom: std::marker::PhantomData<W>,
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
            let raw_dest = link.destination().str(source);
            let target = self
                .rewrite_link(node_ref, &raw_dest)
                .map_err(|e| rushdown::Error::io(format!("link rewrite error: {e}"), None))?;
            render_link_open_tag(
                w,
                &self.format_options,
                &LinkRenderData {
                    source,
                    arena,
                    node_ref,
                    link,
                    target: &target,
                },
            )?;
        } else {
            w.write_str("</a>")?;
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
    format_options: html::Options,
}

impl<'a, W: TextWrite> RenderNode<W> for MultiImageHtmlRenderer<'a, W> {
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
            let image = as_kind_data!(arena, node_ref, Image);
            let doc_id = self.plan.pages[self.current_page].doc_id;
            let raw_target = self.graph.image_target(doc_id, node_ref).ok_or_else(|| {
                rushdown::Error::io(
                    format!(
                        "missing resolved image target for `{}` in {}",
                        image.destination().str(source),
                        self.graph.document(doc_id).path.display()
                    ),
                    None,
                )
            })?;
            // Prepend the asset prefix to make the path relative to this page.
            let target = format!("{}{}", self.asset_prefix, raw_target);
            render_image_tag(
                w,
                &self.format_options,
                &ImageRenderData {
                    source,
                    arena,
                    node_ref,
                    image,
                    target: &target,
                },
            )?;
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
    state: Rc<RenderState>,
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
        _ctx: &mut renderer::Context,
    ) -> rushdown::Result<WalkStatus> {
        let _ = as_extension_data!(arena, node_ref, FootnoteReference);
        if !entering {
            return Ok(WalkStatus::SkipChildren);
        }
        let doc_id = self.state.current_doc_id.get();
        let (index, ref_index) = self
            .footnotes
            .refs
            .get(&NodeLocation::new(doc_id, node_ref))
            .copied()
            .ok_or_else(|| rushdown::Error::io("missing footnote render plan", None))?;
        render_footnote_reference(w, index, ref_index)?;
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

/// Renders the footnote section of the page document `doc_id`.
pub(super) fn render_page_footnotes<W: TextWrite>(
    renderer: &Renderer<'_, W>,
    w: &mut W,
    graph: &DocumentGraph,
    state: &RenderState,
    doc_id: usize,
    footnotes: &FootnotePlan,
) -> rushdown::Result<()> {
    if footnotes.ordered.is_empty() {
        return Ok(());
    }

    let doc = graph.document(doc_id);
    w.write_str("<div class=\"footnotes\" role=\"doc-endnotes\">\n<hr>\n<ol>\n")?;
    for footnote in &footnotes.ordered {
        let definition = doc
            .footnote_definition(&footnote.label)
            .ok_or_else(|| rushdown::Error::io("missing footnote body", None))?;
        w.write_str(&format!("<li id=\"fn:{}\">", footnote.index))?;
        state.current_doc_id.set(doc_id);
        for child in doc.arena[definition.node_ref].children(&doc.arena) {
            renderer.render(w, &doc.source, &doc.arena, child, RenderOptions::default())?;
        }
        for ref_index in &footnote.reference_indices {
            w.write_str(&format!(
                "&#160;<a href=\"#fnref:{ref_index}\" class=\"footnote-backref\" role=\"doc-backlink\">↩</a>"
            ))?;
        }
        w.write_str("</li>\n")?;
    }
    w.write_str("</ol>\n</div>\n")?;
    Ok(())
}

pub(super) fn multi_format_fenced_div_renderer<'cb, W>(
    profile: RenderProfile,
) -> impl RendererExtension<'cb, W>
where
    W: TextWrite + 'cb,
{
    RendererExtensionFn::new(move |renderer: &mut Renderer<'cb, W>| {
        renderer.add_node_renderer(
            move |reg: Rc<RefCell<ContextKeyRegistry>>| {
                let skip_heading_key = reg
                    .borrow_mut()
                    .get_or_create::<NodeRefValue>(MULTI_SKIP_ADMONITION_HEADING);
                SharedFormatFencedDivHtmlRenderer::with_options(
                    skip_heading_key,
                    true,
                    profile.clone(),
                )
            },
            rushdown::renderer::NoRendererOptions,
        );
    })
}

pub(super) fn multi_page_renderer_extension<'cb, W>(
    graph: &'cb DocumentGraph,
    state: Rc<RenderState>,
    plan: &'cb MultiPagePlan,
    current_page: usize,
    base_dir: &'cb Path,
    asset_prefix: &'cb str,
    footnotes: FootnotePlan,
) -> impl RendererExtension<'cb, W>
where
    W: TextWrite + 'cb,
{
    RendererExtensionFn::new(move |renderer: &mut Renderer<'cb, W>| {
        let heading_state = state.clone();
        renderer.add_node_renderer(
            move |reg: Rc<RefCell<ContextKeyRegistry>>| {
                let skip_heading_key = reg
                    .borrow_mut()
                    .get_or_create::<NodeRefValue>(MULTI_SKIP_ADMONITION_HEADING);
                SharedHeadingHtmlRenderer::new(skip_heading_key, heading_state.clone())
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
                format_options: options,
            },
            rushdown::renderer::NoRendererOptions,
        );
        let ref_state = state.clone();
        let ref_footnotes = footnotes.clone();
        renderer.add_node_renderer(
            move || MultiFootnoteReferenceHtmlRenderer {
                _phantom: std::marker::PhantomData,
                state: ref_state.clone(),
                footnotes: ref_footnotes.clone(),
            },
            rushdown::renderer::NoRendererOptions,
        );
        renderer.add_node_renderer(
            || SuppressFootnoteDefinitionRenderer,
            rushdown::renderer::NoRendererOptions,
        );
    })
}
