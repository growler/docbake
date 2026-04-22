use std::any::TypeId;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use anyhow::Result;
use rushdown::as_extension_data;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, Image, Link, NodeRef, WalkStatus};
use rushdown::context::{ContextKey, ContextKeyRegistry, NodeRefValue, UsizeValue};
use rushdown::renderer;
use rushdown::renderer::html::{self, Renderer, RendererExtension, RendererExtensionFn};
use rushdown::renderer::{
    BoxRenderNode, NodeRenderer, NodeRendererRegistry, PostRender, PreRender, Render, RenderNode,
    TextWrite,
};
use rushdown_footnote::FootnoteReference;

use crate::graph::resolve::resolve_base_path;
use crate::graph::{DocumentGraph, NodeLocation};
use crate::html::{
    render_footnote_reference, render_image_tag, render_link_open_tag, ImageRenderData,
    LinkRenderData, SharedFormatFencedDivHtmlRenderer, SharedHeadingHtmlRenderer,
    SuppressDocumentRenderer, SuppressFootnoteDefinitionRenderer,
};

use super::footnotes::FootnotePlan;
use super::path_utils::compute_relative_href;
use super::plan::MultiPagePlan;

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
            render_link_open_tag(
                w,
                &self.writer,
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
                        image.destination_str(source),
                        self.graph.document(doc_id).path.display()
                    ),
                    None,
                )
            })?;
            // Prepend the asset prefix to make the path relative to this page.
            let target = format!("{}{}", self.asset_prefix, raw_target);
            render_image_tag(
                w,
                &self.writer,
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
                    "&#160;<a href=\"#fnref:{ref_index}\" class=\"footnote-backref\" role=\"doc-backlink\">↩</a>"
                ))?;
            }
            self.writer.write_safe_str(w, "</li>\n")?;
        }
        self.writer.write_safe_str(w, "</ol>\n</div>\n")?;
        Ok(())
    }
}

pub(super) fn multi_format_fenced_div_renderer<'cb, W>() -> impl RendererExtension<'cb, W>
where
    W: TextWrite + 'cb,
{
    RendererExtensionFn::new(move |renderer: &mut Renderer<'cb, W>| {
        renderer.add_node_renderer(
            |options: html::Options, reg: Rc<RefCell<ContextKeyRegistry>>| {
                let skip_heading_key = reg
                    .borrow_mut()
                    .get_or_create::<NodeRefValue>(MULTI_SKIP_ADMONITION_HEADING);
                SharedFormatFencedDivHtmlRenderer::with_options(options, skip_heading_key, true)
            },
            rushdown::renderer::NoRendererOptions,
        );
    })
}

pub(super) fn multi_page_renderer_extension<'cb, W>(
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
                let skip_heading_key = reg
                    .borrow_mut()
                    .get_or_create::<NodeRefValue>(MULTI_SKIP_ADMONITION_HEADING);
                SharedHeadingHtmlRenderer::new(options, skip_heading_key, None)
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
