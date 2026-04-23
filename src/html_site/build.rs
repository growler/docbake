use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rushdown::renderer::html::{self, RendererExtension};
use rushdown_definition_list::definition_list_html_renderer_extension;
use rushdown_fenced_div::FencedDivHtmlRendererOptions;
use serde::Serialize;

use crate::code_block_attribute::{
    code_block_attribute_html_renderer_extension, CodeBlockAttributeHtmlRendererOptions,
};
use crate::conditional::RenderProfile;
use crate::diagram::{diagram_html_renderer_extension, DiagramHtmlRendererOptions, DiagramThemes};
use crate::graph::PreparedDocument;

use super::footnotes::build_page_footnote_plan;
use super::path_utils::{compute_asset_prefix, compute_relative_href};
use super::plan::build_multi_page_plan;
use super::renderer_hooks::{multi_format_fenced_div_renderer, multi_page_renderer_extension};
use super::toc::{build_intra_page_toc, build_page_toc, build_page_toc_expanded};

#[derive(Debug, Clone, Serialize)]
struct NavLink {
    href: String,
    title: String,
}

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
    profile: &RenderProfile,
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

    let plan = build_multi_page_plan(prepared, profile, root_source_dir, base_dir)?;

    let mut pages = Vec::new();

    for page_idx in 0..plan.pages.len() {
        let page_info = &plan.pages[page_idx];
        let doc_id = page_info.doc_id;
        let doc = graph.document(doc_id);

        // Compute relative prefix from this page back to the output root.
        let asset_prefix = compute_asset_prefix(&page_info.output_path);

        // Build body HTML.
        let footnotes = build_page_footnote_plan(graph, doc_id, profile)?;
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
                .and(multi_format_fenced_div_renderer(profile.clone()))
                .and(diagram_html_renderer_extension(
                    DiagramHtmlRendererOptions {
                        themes: diagram_themes.clone(),
                        cache: cache.clone(),
                        ..Default::default()
                    },
                ))
                .and(multi_page_renderer_extension(
                    graph,
                    profile.clone(),
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
        let toc_expanded =
            build_page_toc_expanded(&plan, page_idx, graph, profile, &page_info.output_path)?;
        let toc_page = build_intra_page_toc(graph, page_info.doc_id, profile)?;

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
