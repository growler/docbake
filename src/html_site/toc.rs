use std::path::Path;

use anyhow::Result;
use rushdown::ast::{KindData, NodeRef};

use crate::conditional::RenderProfile;
use crate::graph::DocumentGraph;
use crate::html::{heading_text_html, html_escape, is_admonition_title_heading};

use super::path_utils::compute_relative_href;
use super::plan::MultiPagePlan;

/// Build a page-level TOC tree (collapsed, no internal headings).
/// Each page is one `<li>` entry; the tree structure follows the include
/// hierarchy. The current page's entry gets class="current".
/// All hrefs are relative to `current_output_path`.
pub(super) fn build_page_toc(
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
    let page_idx = plan
        .page_index_for_doc(doc_id)
        .expect("document must be present in multi-page plan");
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
pub(super) fn build_page_toc_expanded(
    plan: &MultiPagePlan,
    current_page: usize,
    graph: &DocumentGraph,
    profile: &RenderProfile,
    current_output_path: &Path,
) -> Result<String> {
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
                profile,
                current_output_path,
            )?;
        }
    }
    html.push_str("</ul>\n");
    Ok(html)
}

fn build_expanded_toc_node(
    html: &mut String,
    plan: &MultiPagePlan,
    doc_id: usize,
    current_page: usize,
    graph: &DocumentGraph,
    profile: &RenderProfile,
    current_output_path: &Path,
) -> Result<()> {
    let page_idx = plan
        .page_index_for_doc(doc_id)
        .expect("document must be present in multi-page plan");
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
    let all_headings = collect_page_headings(graph, doc_id, profile)?;
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
            profile,
            current_output_path,
        )?;
    }

    if has_headings || has_children {
        html.push_str("</ul>\n");
    }

    html.push_str("</li>\n");
    Ok(())
}

/// Build the intra-page TOC: headings within the current page only,
/// excluding the page's own title heading (the first one).
/// Returns a nested `<ul>/<li>/<a>` HTML string suitable for a right-side
/// "on this page" sidebar.
pub(super) fn build_intra_page_toc(
    graph: &DocumentGraph,
    doc_id: usize,
    profile: &RenderProfile,
) -> Result<String> {
    let all_headings = collect_page_headings(graph, doc_id, profile)?;
    // Skip the first heading — it is the page title.
    let headings: Vec<_> = all_headings.into_iter().skip(1).collect();
    if headings.is_empty() {
        return Ok(String::new());
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
    Ok(html)
}

/// Collect headings for a single page (not recursing into includes).
/// Returns `(level, id, text_html)` tuples.
fn collect_page_headings(
    graph: &DocumentGraph,
    doc_id: usize,
    profile: &RenderProfile,
) -> Result<Vec<(usize, String, String)>> {
    let doc = graph.document(doc_id);
    let mut headings = Vec::new();
    for child in doc.arena[doc.doc_ref].children(&doc.arena) {
        collect_headings_recursive(graph, doc_id, child, profile, &mut headings)?;
    }
    Ok(headings)
}

fn collect_headings_recursive(
    graph: &DocumentGraph,
    doc_id: usize,
    node_ref: NodeRef,
    profile: &RenderProfile,
    headings: &mut Vec<(usize, String, String)>,
) -> Result<()> {
    let doc = graph.document(doc_id);

    if !crate::conditional::node_is_visible(&doc.arena, node_ref, &doc.source, profile)? {
        return Ok(());
    }

    // Skip include fenced divs entirely.
    if graph.include_entries(doc_id, node_ref).is_some() {
        return Ok(());
    }

    if let KindData::Heading(heading) = doc.arena[node_ref].kind_data() {
        // Skip headings used as admonition titles.
        if is_admonition_title_heading(&doc.arena, node_ref, &doc.source) {
            return Ok(());
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
        collect_headings_recursive(graph, doc_id, child, profile, headings)?;
    }
    Ok(())
}
