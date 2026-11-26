use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::code_block_attribute::{
    code_block_attribute_html_renderer_extension, CodeBlockAttributeHtmlRendererOptions,
};
use crate::diagram::{
    diagram_html_renderer_extension, write_mermaid_script, DiagramHtmlRendererOptions,
    DiagramThemes,
};
use anyhow::{bail, Context, Result};
use rushdown::as_extension_data;
use rushdown::as_kind_data;
use rushdown::ast::{Arena, Image, KindData, Link, NodeRef, WalkStatus};
use rushdown::context::{ContextKeyRegistry, NodeRefValue};
use rushdown::renderer;
use rushdown::renderer::html::{self, Renderer, RendererExtension, RendererExtensionFn};
use rushdown::renderer::{
    BoxRenderNode, NodeRenderer, NodeRendererRegistry, RenderNode, RenderOptions, TextWrite,
};
use rushdown_definition_list::definition_list_html_renderer_extension;
use rushdown_fenced_div::FencedDivHtmlRendererOptions;
use rushdown_footnote::FootnoteReference;

use crate::conditional::RenderProfile;
use crate::graph::{DocumentGraph, NodeLocation, PreparedDocument};
use crate::html::{
    heading_text_html, html_escape, is_admonition_title_heading, render_footnote_reference,
    render_image_tag, render_link_open_tag, ImageRenderData, LinkRenderData, RenderState,
    SharedFormatFencedDivHtmlRenderer, SharedHeadingHtmlRenderer,
    SuppressFootnoteDefinitionRenderer,
};

const SKIP_ADMONITION_HEADING: &str = "docbake-skip-admonition-heading";

#[derive(Clone)]
struct FootnoteEntry {
    doc_id: usize,
    label: String,
    index: usize,
    reference_indices: Vec<usize>,
}

#[derive(Clone, Default)]
struct FootnotePlan {
    refs: HashMap<NodeLocation, (usize, usize)>,
    ordered: Vec<FootnoteEntry>,
}

fn build_footnote_plan(
    prepared: &PreparedDocument,
    profile: &RenderProfile,
) -> Result<FootnotePlan> {
    let mut refs = HashMap::new();
    let mut ordered = Vec::<FootnoteEntry>::new();
    let mut by_key = HashMap::<(usize, String), usize>::new();
    let mut next_ref_index = 1usize;

    for (doc_id, node_ref, _) in prepared.graph().rendered_nodes_preorder_visible(profile)? {
        let doc = prepared.graph().document(doc_id);
        if !rushdown::matches_extension_kind!(&doc.arena, node_ref, FootnoteReference) {
            continue;
        }
        let footnote = as_extension_data!(&doc.arena, node_ref, FootnoteReference);
        let label = footnote.label().str(&doc.source).to_string();
        if doc.footnote_definition(&label).is_none() {
            bail!(
                "missing footnote definition for `{label}` in {}",
                doc.path.display()
            );
        }
        let index = if let Some(index) = by_key.get(&(doc_id, label.clone())) {
            *index
        } else {
            let index = ordered.len() + 1;
            ordered.push(FootnoteEntry {
                doc_id,
                label: label.clone(),
                index,
                reference_indices: Vec::new(),
            });
            by_key.insert((doc_id, label.clone()), index);
            index
        };
        let ref_index = next_ref_index;
        next_ref_index += 1;
        ordered[index - 1].reference_indices.push(ref_index);
        refs.insert(NodeLocation::new(doc_id, node_ref), (index, ref_index));
    }

    Ok(FootnotePlan { refs, ordered })
}

pub fn build_html_document(
    template: &str,
    context: &tera::Context,
    prepared: &PreparedDocument,
    profile: &RenderProfile,
    toc: &str,
    cache: &crate::cache::Cache,
    diagram_themes: &DiagramThemes,
) -> Result<String> {
    let state = Rc::new(RenderState::default());
    let diagram_opts = DiagramHtmlRendererOptions {
        themes: diagram_themes.clone(),
        cache: cache.clone(),
        state: state.clone(),
        ..Default::default()
    };

    let mut body = String::new();
    let footnotes = build_footnote_plan(prepared, profile)?;
    let renderer = html::Renderer::with_extensions(
        html::Options::default(),
        rushdown_fenced_div::fenced_div_html_renderer_extension(FencedDivHtmlRendererOptions)
            .and(code_block_attribute_html_renderer_extension(
                CodeBlockAttributeHtmlRendererOptions,
            ))
            .and(definition_list_html_renderer_extension())
            .and(crate::bracketed_span::bracketed_span_html_renderer_extension())
            .and(crate::caption::caption_html_renderer_extension())
            .and(format_specific_fenced_div_renderer(profile.clone()))
            .and(diagram_html_renderer_extension(diagram_opts.clone()))
            .and(graph_html_renderer_extension(
                prepared,
                state.clone(),
                footnotes.clone(),
            )),
    );
    let root_id = prepared.graph().root_id;
    render_graph_document(&renderer, &mut body, prepared, profile, &state, root_id, 0)
        .and_then(|()| write_mermaid_script(&mut body, &diagram_opts))
        .and_then(|()| render_graph_footnotes(&renderer, &mut body, prepared, &state, &footnotes))
        .map_err(|error| anyhow::anyhow!("failed to render HTML body: {error:?}"))?;

    let mut page = context.clone();
    page.insert("body", &body);
    page.insert("toc", toc);
    tera::Tera::one_off(template, &page, false).context("failed to render HTML template")
}

pub fn build_toc(graph: &DocumentGraph, profile: &RenderProfile) -> Result<String> {
    let mut headings = Vec::new();
    for (doc_id, node_ref, shift) in graph.rendered_nodes_preorder_visible(profile)? {
        let doc = graph.document(doc_id);
        let KindData::Heading(heading) = doc.arena[node_ref].kind_data() else {
            continue;
        };
        // Skip headings used as admonition titles.
        if is_admonition_title_heading(&doc.arena, node_ref, &doc.source) {
            continue;
        }
        let classes = doc.arena[node_ref]
            .attributes()
            .get("class")
            .map(|value| value.str(&doc.source))
            .unwrap_or_default();
        let skip = classes
            .split_whitespace()
            .any(|class_name| class_name == "unlisted" || class_name == "unnumbered");
        if skip {
            continue;
        }
        let id = doc.arena[node_ref]
            .attributes()
            .get("id")
            .map(|value| value.str(&doc.source).to_string())
            .unwrap_or_default();
        headings.push((
            (heading.level() as usize + shift).min(6),
            id,
            heading_text_html(&doc.arena, node_ref, &doc.source),
        ));
    }

    let mut html = String::new();
    let mut current_level = 0usize;
    // Track whether the last <li> is still open (waiting for a potential sub-<ul>).
    let mut li_open = false;

    for (level, id, text) in headings {
        if level > current_level {
            // Going deeper: open <ul> for each new level. The first level
            // transition (0→1) opens the root <ul> with no <li> wrapper.
            // Subsequent deeper transitions nest <ul> inside the currently
            // open <li> (which we deliberately left unclosed).
            while current_level < level {
                html.push_str("<ul>\n");
                current_level += 1;
            }
        } else if level < current_level {
            // Going up: close the open <li>, then close </ul></li> pairs.
            if li_open {
                html.push_str("</li>\n");
            }
            while current_level > level {
                html.push_str("</ul>\n</li>\n");
                current_level -= 1;
            }
        } else {
            // Same level: close the previous <li>.
            if li_open {
                html.push_str("</li>\n");
            }
        }
        html.push_str(&format!(
            "<li><a href=\"#{}\">{}</a>\n",
            html_escape(&id),
            text
        ));
        li_open = true;
    }

    // Close all remaining open elements.
    if li_open {
        html.push_str("</li>\n");
    }
    while current_level > 0 {
        html.push_str("</ul>\n");
        current_level -= 1;
        if current_level > 0 {
            html.push_str("</li>\n");
        }
    }
    Ok(html)
}

/// Renders the visible children of document `doc_id` and, in place of each
/// `:::include` block, the documents that it includes.
fn render_graph_document<W: TextWrite>(
    renderer: &Renderer<'_, W>,
    writer: &mut W,
    prepared: &PreparedDocument,
    profile: &RenderProfile,
    state: &RenderState,
    doc_id: usize,
    heading_shift: usize,
) -> rushdown::Result<()> {
    let doc = prepared.graph().document(doc_id);
    for child in doc.arena[doc.doc_ref].children(&doc.arena) {
        let visible = crate::conditional::node_is_visible(&doc.arena, child, &doc.source, profile)
            .map_err(|error| rushdown::Error::io(format!("{error:#}"), None))?;
        if !visible {
            continue;
        }
        if let Some(entries) = prepared.graph().include_entries(doc_id, child) {
            for entry in entries {
                render_graph_document(
                    renderer,
                    writer,
                    prepared,
                    profile,
                    state,
                    entry.doc_id,
                    heading_shift + entry.heading_shift,
                )?;
            }
            continue;
        }
        state.current_doc_id.set(doc_id);
        state.heading_shift.set(heading_shift);
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

struct GraphFootnoteReferenceHtmlRenderer<W: TextWrite> {
    _phantom: std::marker::PhantomData<W>,
    state: Rc<RenderState>,
    footnotes: FootnotePlan,
}

impl<W: TextWrite> GraphFootnoteReferenceHtmlRenderer<W> {
    fn new(state: Rc<RenderState>, footnotes: FootnotePlan) -> Self {
        Self {
            _phantom: std::marker::PhantomData,
            state,
            footnotes,
        }
    }
}

impl<W: TextWrite> RenderNode<W> for GraphFootnoteReferenceHtmlRenderer<W> {
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

impl<'cb, W> NodeRenderer<'cb, W> for GraphFootnoteReferenceHtmlRenderer<W>
where
    W: TextWrite + 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<FootnoteReference>(), BoxRenderNode::new(self));
    }
}

/// Renders the footnote section of the page: one list item for each entry of
/// `footnotes`, in plan order.
fn render_graph_footnotes<W: TextWrite>(
    renderer: &Renderer<'_, W>,
    w: &mut W,
    prepared: &PreparedDocument,
    state: &RenderState,
    footnotes: &FootnotePlan,
) -> rushdown::Result<()> {
    if footnotes.ordered.is_empty() {
        return Ok(());
    }

    w.write_str("<div class=\"footnotes\" role=\"doc-endnotes\">\n<hr>\n<ol>\n")?;
    for footnote in &footnotes.ordered {
        let doc = prepared.graph().document(footnote.doc_id);
        let definition = doc
            .footnote_definition(&footnote.label)
            .ok_or_else(|| rushdown::Error::io("missing footnote body", None))?;
        w.write_str(&format!("<li id=\"fn:{}\">", footnote.index))?;
        state.current_doc_id.set(footnote.doc_id);
        state.heading_shift.set(0);
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

struct GraphLinkHtmlRenderer<'a, W: TextWrite> {
    prepared: &'a PreparedDocument,
    _phantom: std::marker::PhantomData<W>,
    state: Rc<RenderState>,
    format_options: html::Options,
}

impl<'a, W: TextWrite> GraphLinkHtmlRenderer<'a, W> {
    fn new(prepared: &'a PreparedDocument, options: html::Options, state: Rc<RenderState>) -> Self {
        Self {
            prepared,
            _phantom: std::marker::PhantomData,
            state,
            format_options: options,
        }
    }
}

impl<'a, W: TextWrite> RenderNode<W> for GraphLinkHtmlRenderer<'a, W> {
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
            let doc_id = self.state.current_doc_id.get();
            let target = self
                .prepared
                .graph()
                .link_target(doc_id, node_ref)
                .ok_or_else(|| {
                    rushdown::Error::io(
                        format!(
                            "missing resolved link target for `{}` in {}",
                            link.destination().str(source),
                            self.prepared.graph().document(doc_id).path.display()
                        ),
                        None,
                    )
                })?
                .to_string();
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

impl<'a, 'cb, W> NodeRenderer<'cb, W> for GraphLinkHtmlRenderer<'a, W>
where
    W: TextWrite + 'cb,
    'a: 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<Link>(), BoxRenderNode::new(self));
    }
}

struct GraphImageHtmlRenderer<'a, W: TextWrite> {
    prepared: &'a PreparedDocument,
    _phantom: std::marker::PhantomData<W>,
    state: Rc<RenderState>,
    format_options: html::Options,
}

impl<'a, W: TextWrite> GraphImageHtmlRenderer<'a, W> {
    fn new(prepared: &'a PreparedDocument, options: html::Options, state: Rc<RenderState>) -> Self {
        Self {
            prepared,
            _phantom: std::marker::PhantomData,
            state,
            format_options: options,
        }
    }
}

impl<'a, W: TextWrite> RenderNode<W> for GraphImageHtmlRenderer<'a, W> {
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
            let doc_id = self.state.current_doc_id.get();
            let target = self
                .prepared
                .graph()
                .image_target(doc_id, node_ref)
                .ok_or_else(|| {
                    rushdown::Error::io(
                        format!(
                            "missing resolved image target for `{}` in {}",
                            image.destination().str(source),
                            self.prepared.graph().document(doc_id).path.display()
                        ),
                        None,
                    )
                })?
                .to_string();
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

impl<'a, 'cb, W> NodeRenderer<'cb, W> for GraphImageHtmlRenderer<'a, W>
where
    W: TextWrite + 'cb,
    'a: 'cb,
{
    fn register_node_renderer_fn(self, nrr: &mut impl NodeRendererRegistry<'cb, W>) {
        nrr.register_node_renderer_fn(TypeId::of::<Image>(), BoxRenderNode::new(self));
    }
}

fn format_specific_fenced_div_renderer<'cb, W>(
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
                    .get_or_create::<NodeRefValue>(SKIP_ADMONITION_HEADING);
                SharedFormatFencedDivHtmlRenderer::with_options(
                    skip_heading_key,
                    false,
                    profile.clone(),
                )
            },
            rushdown::renderer::NoRendererOptions,
        );
    })
}

fn graph_html_renderer_extension<'cb, W>(
    prepared: &'cb PreparedDocument,
    state: Rc<RenderState>,
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
                    .get_or_create::<NodeRefValue>(SKIP_ADMONITION_HEADING);
                SharedHeadingHtmlRenderer::new(skip_heading_key, heading_state.clone())
            },
            rushdown::renderer::NoRendererOptions,
        );
        let link_state = state.clone();
        renderer.add_node_renderer(
            move |options: html::Options| {
                GraphLinkHtmlRenderer::new(prepared, options, link_state.clone())
            },
            rushdown::renderer::NoRendererOptions,
        );
        let image_state = state.clone();
        renderer.add_node_renderer(
            move |options: html::Options| {
                GraphImageHtmlRenderer::new(prepared, options, image_state.clone())
            },
            rushdown::renderer::NoRendererOptions,
        );
        let ref_state = state.clone();
        let ref_footnotes = footnotes.clone();
        renderer.add_node_renderer(
            move || {
                GraphFootnoteReferenceHtmlRenderer::new(ref_state.clone(), ref_footnotes.clone())
            },
            rushdown::renderer::NoRendererOptions,
        );
        renderer.add_node_renderer(
            || SuppressFootnoteDefinitionRenderer,
            rushdown::renderer::NoRendererOptions,
        );
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use anyhow::Result;
    use tempfile::TempDir;

    use super::{build_html_document, build_toc};
    use crate::cache::Cache;
    use crate::cli::BrokenLinkMode;
    use crate::conditional::{EffectiveFormat, RenderProfile};
    use crate::diagram::DiagramThemes;
    use crate::graph::prepare_document;
    use crate::DebugDumper;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    fn html_profile() -> RenderProfile {
        RenderProfile::from_metadata(EffectiveFormat::Html, &serde_json::Map::new()).unwrap()
    }

    #[test]
    fn toc_skips_admonition_titles_and_unlisted_headings() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            concat!(
                "# Root\n\n",
                ":::note\n",
                "## Hidden Admonition Heading\n",
                "Some note.\n",
                ":::\n\n",
                "## Visible\n\n",
                "### Hidden In Toc {.unlisted}\n",
            ),
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            Some("test"),
            BrokenLinkMode::Error,
        )
        .unwrap();

        let profile = html_profile();
        let toc = build_toc(prepared.graph(), &profile).unwrap();
        assert!(toc.contains("Root"));
        assert!(toc.contains("Visible"));
        assert!(!toc.contains("Hidden Admonition Heading"));
        assert!(!toc.contains("Hidden In Toc"));
    }

    #[test]
    fn renders_endnotes_for_single_page_html() -> Result<()> {
        let temp = TempDir::new()?;
        let root = temp.path();
        write(
            &root.join("doc/index.md"),
            "# Root\n\nText with note[^a].\n\n[^a]: Footnote body\n",
        );

        let prepared = prepare_document(
            &DebugDumper::new(false),
            root,
            &root.join("doc/index.md"),
            Some("test"),
            BrokenLinkMode::Error,
        )?;
        let profile = html_profile();
        let html = build_html_document(
            "{{ body }}",
            &tera::Context::new(),
            &prepared,
            &profile,
            "",
            &Cache::new(None, true)?,
            &DiagramThemes::default(),
        )?;

        assert!(html.contains("class=\"footnote-ref\""));
        assert!(html.contains("<div class=\"footnotes\" role=\"doc-endnotes\">"));
        assert!(html.contains("class=\"footnote-backref\""));
        Ok(())
    }
}
