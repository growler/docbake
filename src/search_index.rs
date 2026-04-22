// ── Search index builder ─────────────────────────────────────────────────
//
// Generates a JSON array of search entries for client-side fuzzy search
// with Fuse.js.  Each entry corresponds to a document section anchored
// by an ID-bearing element (heading, captioned figure/table, or named
// code block).
//
// The index is embedded directly into the HTML template via a Tera
// variable (`search_index`).
//
// Entry structure:
//
//   { "title": "...", "href": "#anchor", "body": "...", "snippet": "..." }
//
// - `title`   – plain text of the anchor element (heading, caption, etc.)
// - `href`    – fragment-only for single-page; page-relative for multi-page
// - `body`    – full cleaned plain text of the section (searched by Fuse.js)
// - `snippet` – first ~200 chars of `body` (displayed in search results)

use rushdown::ast::{self, Arena, CodeBlock, CodeBlockKind, KindData, NodeRef, WalkStatus};
use serde::Serialize;

use crate::graph::{DocumentGraph, PreparedDocument};
use crate::html_site::MultiPagePlan;

/// Maximum length (in chars) of the display snippet.
const SNIPPET_MAX_CHARS: usize = 200;

/// Code-block languages whose content should be excluded from the search
/// index because they contain visual markup rather than searchable text.
const DIAGRAM_LANGUAGES: &[&str] = &["plantuml", "mermaid", "d2", "typst", "drawio"];

// ---------------------------------------------------------------------------
// Public data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SearchEntry {
    pub title: String,
    pub href: String,
    pub body: String,
    pub snippet: String,
}

// ---------------------------------------------------------------------------
// Single-page index
// ---------------------------------------------------------------------------

/// Build a search index for the single-page HTML format.
///
/// Every ID-bearing node (heading, caption, named code block) starts a new
/// chunk.  The `href` values are fragment-only (`#anchor-id`).
pub(crate) fn build_search_index(prepared: &PreparedDocument) -> String {
    let graph = prepared.graph();
    let entries = collect_entries(graph, None);
    serde_json::to_string(&entries).expect("search index serialisation must not fail")
}

// ---------------------------------------------------------------------------
// Multi-page index
// ---------------------------------------------------------------------------

/// Build a search index for the multi-page HTML format.
///
/// All pages share the same index.  The `href` values are relative to the
/// output root (`page.html#anchor` or `dir/page.html#anchor`).  The JS
/// side prepends `asset_prefix` when navigating from nested pages.
pub(crate) fn build_search_index_multi(
    prepared: &PreparedDocument,
    plan: &MultiPagePlan,
) -> String {
    let graph = prepared.graph();
    let entries = collect_entries(graph, Some(plan));
    serde_json::to_string(&entries).expect("search index serialisation must not fail")
}

// ---------------------------------------------------------------------------
// Core collection logic
// ---------------------------------------------------------------------------

/// Walk the rendered node list and collect search entries.
///
/// When `plan` is `Some`, href values include the page path (multi-page
/// mode).  When `None`, href values are fragment-only (single-page mode).
fn collect_entries(graph: &DocumentGraph, plan: Option<&MultiPagePlan>) -> Vec<SearchEntry> {
    let mut entries: Vec<SearchEntry> = Vec::new();

    // Current chunk state — accumulates text between anchor points.
    let mut current_title: Option<String> = None;
    let mut current_href: Option<String> = None;
    let mut current_body = String::new();

    for (doc_id, node_ref, _shift) in graph.rendered_nodes_preorder() {
        let doc = graph.document(doc_id);
        let arena = &doc.arena;
        let source = &doc.source;

        // Check whether this node carries an ID (anchor point).
        let node_id = arena[node_ref]
            .attributes()
            .get("id")
            .map(|v| v.str(source).to_string());

        if let Some(id) = node_id {
            // Flush the previous chunk (if any).
            flush_entry(&mut entries, &current_title, &current_href, &current_body);
            current_body.clear();

            // Determine the title for this anchor.
            let title = extract_anchor_title(arena, node_ref, source);

            // Determine the href.
            let href = match plan {
                Some(plan) => {
                    // Multi-page: prefix with the page output path.
                    let page_path = plan
                        .page_for_doc(doc_id)
                        .map(|p| p.output_path.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default();
                    format!("{page_path}#{id}")
                }
                None => {
                    // Single-page: fragment-only.
                    format!("#{id}")
                }
            };

            current_title = Some(title);
            current_href = Some(href);

            // For headings, the title text is already captured — don't
            // duplicate it in the body.  For other anchor types (captions,
            // code blocks) the body will contain the surrounding content.
            if matches!(arena[node_ref].kind_data(), KindData::Heading(_)) {
                continue;
            }
        }

        // Accumulate plain text into the current chunk body.
        collect_node_text(arena, node_ref, source, &mut current_body);
    }

    // Flush the last chunk.
    flush_entry(&mut entries, &current_title, &current_href, &current_body);

    entries
}

/// Push a finished search entry (if it has a title and href).
fn flush_entry(
    entries: &mut Vec<SearchEntry>,
    title: &Option<String>,
    href: &Option<String>,
    body: &str,
) {
    if let (Some(title), Some(href)) = (title, href) {
        if title.is_empty() {
            return;
        }
        let body_clean = normalise_whitespace(body);
        let snippet = make_snippet(&body_clean, SNIPPET_MAX_CHARS);
        entries.push(SearchEntry {
            title: title.clone(),
            href: href.clone(),
            body: body_clean,
            snippet,
        });
    }
}

// ---------------------------------------------------------------------------
// Text extraction
// ---------------------------------------------------------------------------

/// Extract a display title for an anchor node.
///
/// - Headings → heading text (inline content).
/// - Captions → caption text content.
/// - Code blocks → the descriptive text from the info string, or the
///   explicit id as a fallback.
fn extract_anchor_title(arena: &Arena, node_ref: NodeRef, source: &str) -> String {
    match arena[node_ref].kind_data() {
        KindData::Heading(_) => plain_text(arena, node_ref, source),
        _ if rushdown::matches_extension_kind!(arena, node_ref, crate::caption::Caption) => {
            // Caption node: extract text from CaptionText child.
            for child in arena[node_ref].children(arena) {
                if rushdown::matches_extension_kind!(arena, child, crate::caption::CaptionText) {
                    return plain_text(arena, child, source);
                }
            }
            String::new()
        }
        KindData::CodeBlock(cb) => {
            // Named code block: use the descriptive text from the info
            // string if available, otherwise the explicit id.
            if cb.code_block_kind() == CodeBlockKind::Fenced {
                if let Some(desc) = code_block_description(cb, source) {
                    return desc;
                }
            }
            // Fallback: use the ID attribute.
            arena[node_ref]
                .attributes()
                .get("id")
                .map(|v| v.str(source).to_string())
                .unwrap_or_default()
        }
        _ => {
            // Generic anchor: try to extract child text.
            plain_text(arena, node_ref, source)
        }
    }
}

/// Collect plain text from a single AST node into `out`.
///
/// Skips diagram code blocks (their content is visual markup, not
/// searchable prose).  Also skips HTML blocks.
fn collect_node_text(arena: &Arena, node_ref: NodeRef, source: &str, out: &mut String) {
    match arena[node_ref].kind_data() {
        // Leaf text nodes — the most common case.
        KindData::Text(value) => {
            out.push_str(value.str(source));
            out.push(' ');
        }
        KindData::CodeSpan(cs) => {
            out.push_str(&cs.value_str(source));
            out.push(' ');
        }
        // Code blocks — include content unless it's a diagram language.
        KindData::CodeBlock(cb) => {
            if cb.code_block_kind() == CodeBlockKind::Fenced {
                if let Some(lang) = cb.language_str(source) {
                    if DIAGRAM_LANGUAGES.iter().any(|&d| d == lang) {
                        return; // skip diagram content
                    }
                }
            }
            for line in cb.value().iter(source) {
                out.push_str(&line);
                out.push(' ');
            }
        }
        // Skip HTML blocks — they contain raw markup, not prose.
        KindData::HtmlBlock(_) | KindData::RawHtml(_) => {}
        // All other node types (paragraphs, lists, line breaks, etc.)
        // are containers — their text comes from child nodes which we
        // see separately in the preorder walk.
        _ => {}
    }
}

/// Extract plain text from a node and all its children (recursive walk).
/// Used for heading titles, caption titles, etc.
fn plain_text(arena: &Arena, node_ref: NodeRef, source: &str) -> String {
    let mut text = String::new();
    let _ = ast::walk(arena, node_ref, &mut |arena: &Arena,
                                             child_ref: NodeRef,
                                             entering: bool|
     -> std::result::Result<
        WalkStatus,
        std::io::Error,
    > {
        if !entering || child_ref == node_ref {
            return Ok(WalkStatus::Continue);
        }
        match arena[child_ref].kind_data() {
            KindData::Text(value) => text.push_str(value.str(source)),
            KindData::CodeSpan(cs) => text.push_str(&cs.value_str(source)),
            _ => {}
        }
        Ok(WalkStatus::Continue)
    });
    text
}

/// Extracts the descriptive text from a fenced code block's info string.
///
/// The info string has the form `language Description text {#id .class}`.
/// Returns `Some("Description text")` or `None` if there is no description.
fn code_block_description(cb: &CodeBlock, source: &str) -> Option<String> {
    let info = cb.info_str(source)?;
    // Skip the language token (first word).
    let rest = info.split_once(char::is_whitespace).map(|(_, r)| r)?;
    // Strip any trailing attribute block {#id .class key=val}.
    let rest = rest.trim();
    let desc = if let Some(pos) = rest.rfind('{') {
        rest[..pos].trim()
    } else {
        rest
    };
    if desc.is_empty() {
        None
    } else {
        Some(desc.to_string())
    }
}

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

/// Collapse runs of whitespace into single spaces and trim.
fn normalise_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_space = true; // start true to skip leading whitespace
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                result.push(' ');
                prev_space = true;
            }
        } else {
            result.push(ch);
            prev_space = false;
        }
    }
    // Trim trailing space.
    if result.ends_with(' ') {
        result.pop();
    }
    result
}

/// Truncate to approximately `max_chars` (measured in Unicode chars, not
/// bytes), breaking at a word boundary.
fn make_snippet(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    // Find the byte offset of the `max_chars`-th character.
    let byte_limit = text
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    // Walk back to the last space before the limit.
    let break_pos = text[..byte_limit].rfind(' ').unwrap_or(byte_limit);
    let mut snippet = text[..break_pos].to_string();
    snippet.push('\u{2026}'); // ellipsis
    snippet
}
