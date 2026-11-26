use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rushdown::as_kind_data;
use rushdown::ast::{self, Arena, CodeBlock, CodeBlockKind, KindData, NodeRef, WalkStatus};
use rushdown::matches_kind;
use rushdown::text;

use super::types::NodeLocation;
use super::DocumentGraph;

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// A single broken-link diagnostic produced during graph resolution.
#[derive(Debug)]
pub(crate) struct LinkDiagnostic {
    /// Source markdown file that contains the broken link.
    pub file: PathBuf,
    /// 1-based line number (0 when the position cannot be determined).
    pub line: usize,
    /// Display text of the link (collected from child text nodes).
    pub link_text: String,
    /// The raw destination string as written in the source.
    pub target: String,
    /// Human-readable explanation of why the link is broken.
    pub message: String,
}

impl LinkDiagnostic {
    /// Format the diagnostic with paths shown relative to `base_dir`.
    pub(crate) fn display_relative(&self, base_dir: &Path) -> String {
        let rel = self.file.strip_prefix(base_dir).unwrap_or(&self.file);
        let location = if self.line > 0 {
            format!("{}:{}", rel.display(), self.line)
        } else {
            format!("{}", rel.display())
        };
        // Also strip base_dir prefix from paths embedded in the message.
        let base_prefix = format!("{}/", base_dir.display());
        let message = self.message.replace(&base_prefix, "");
        format!(
            "{location}: broken link [{}]({}): {}",
            self.link_text, self.target, message,
        )
    }
}

impl fmt::Display for LinkDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let location = if self.line > 0 {
            format!("{}:{}", self.file.display(), self.line)
        } else {
            format!("{}", self.file.display())
        };
        write!(
            f,
            "{location}: broken link [{}]({}): {}",
            self.link_text, self.target, self.message,
        )
    }
}

/// Compute a 1-based line number from a byte offset into `source`.
/// Returns 0 if the offset is out of range.
fn byte_offset_to_line(source: &str, offset: usize) -> usize {
    if offset > source.len() {
        return 0;
    }
    source[..offset].bytes().filter(|&b| b == b'\n').count() + 1
}

/// Try to extract a byte offset from a `text::Value` for line-number
/// computation.  Returns `Some(start)` for index-backed values, `None`
/// for heap-allocated strings (synthesised by transforms).
fn destination_byte_offset(dest: &text::Value) -> Option<usize> {
    match dest {
        text::Value::Index(idx, _) => Some(idx.start()),
        _ => None,
    }
}

/// Collect plain text from the children of a link/image node (for use in
/// diagnostic messages).
fn link_text(arena: &Arena, node_ref: NodeRef, source: &str) -> String {
    heading_text(arena, node_ref, source)
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub(super) fn resolve_document_graph(
    base_dir: &Path,
    graph: &mut DocumentGraph,
) -> Result<Vec<LinkDiagnostic>> {
    collect_and_assign_ids(graph)?;
    rewrite_graph_targets(base_dir, graph)
}

fn collect_and_assign_ids(graph: &mut DocumentGraph) -> Result<()> {
    let mut final_seen = HashSet::new();
    let mut local_seen: std::collections::HashMap<usize, HashSet<String>> =
        std::collections::HashMap::new();
    for (doc_id, node_ref, _) in graph.rendered_nodes_preorder().collect::<Vec<_>>() {
        let doc = graph.document(doc_id);

        // Handle headings.
        if matches!(doc.arena[node_ref].kind_data(), KindData::Heading(_)) {
            let (source_anchor, final_id) = {
                let source_anchor = if let Some(explicit) =
                    doc.arena[node_ref].attributes().get("id")
                {
                    explicit.str(&doc.source).to_string()
                } else {
                    let seen = local_seen.entry(doc_id).or_default();
                    let anchor =
                        generate_heading_id(&heading_text(&doc.arena, node_ref, &doc.source), seen);
                    seen.insert(anchor.clone());
                    anchor
                };
                let final_id = if doc.arena[node_ref].attributes().contains_key("id") {
                    if final_seen.contains(&source_anchor) {
                        bail!(
                            "duplicate explicit heading id `{}` in {}",
                            source_anchor,
                            doc.path.display()
                        );
                    }
                    source_anchor.clone()
                } else {
                    generate_heading_id(&source_anchor, &final_seen)
                };
                (source_anchor, final_id)
            };
            final_seen.insert(final_id.clone());
            graph
                .anchor_targets
                .insert((doc_id, source_anchor), final_id.clone());
            graph.documents[doc_id].arena[node_ref]
                .attributes_mut()
                .insert("id".to_string(), final_id.clone().into());
            graph.first_heading_by_doc.entry(doc_id).or_insert(final_id);
            continue;
        }

        // Handle captions.
        if rushdown::matches_extension_kind!(&doc.arena, node_ref, crate::caption::Caption) {
            let final_id = if let Some(explicit) = doc.arena[node_ref].attributes().get("id") {
                let id = explicit.str(&doc.source).to_string();
                if final_seen.contains(&id) {
                    bail!(
                        "duplicate explicit caption id `{}` in {}",
                        id,
                        doc.path.display()
                    );
                }
                id
            } else {
                // Extract text from the CaptionText child and generate an ID
                // that is unique across both headings and captions.
                let caption_text = caption_text_content(&doc.arena, node_ref, &doc.source);
                let base = generate_heading_id(&caption_text, &HashSet::new());
                generate_heading_id(&base, &final_seen)
            };
            final_seen.insert(final_id.clone());
            // Use final_id as source_anchor — this is the fragment the user
            // would write in a link (`[text](#final-id)`).
            graph
                .anchor_targets
                .insert((doc_id, final_id.clone()), final_id.clone());
            graph.documents[doc_id].arena[node_ref]
                .attributes_mut()
                .insert("id".to_string(), final_id.into());
            continue;
        }

        // Handle fenced code blocks.
        //
        // Code blocks with descriptive text in their info string (after the
        // language token) receive an auto-generated ID, making them
        // referenceable via fragment links just like headings.
        //
        //     ```rust Basic function {.hl}
        //     fn basic() {}
        //     ```
        //
        // The description `Basic function` produces id `basic-function`.
        // Code blocks with only a language token and no description are
        // skipped (no auto-id).  An explicit `{#my-id}` always wins.
        if matches_kind!(doc.arena[node_ref], CodeBlock) {
            let code_block = as_kind_data!(doc.arena[node_ref], CodeBlock);
            if code_block.code_block_kind() != CodeBlockKind::Fenced {
                continue;
            }

            if let Some(explicit) = doc.arena[node_ref].attributes().get("id") {
                // Explicit ID from {#my-id} — deduplicate against the
                // global pool (same rules as explicit heading IDs).
                let id = explicit.str(&doc.source).to_string();
                if final_seen.contains(&id) {
                    bail!(
                        "duplicate explicit code block id `{}` in {}",
                        id,
                        doc.path.display()
                    );
                }
                final_seen.insert(id.clone());
                graph.anchor_targets.insert((doc_id, id.clone()), id);
            } else if let Some(description) = code_block_description(code_block, &doc.source) {
                // Auto-generate ID from the descriptive text.
                let seen = local_seen.entry(doc_id).or_default();
                let source_anchor = generate_heading_id(&description, seen);
                seen.insert(source_anchor.clone());
                let final_id = generate_heading_id(&source_anchor, &final_seen);
                final_seen.insert(final_id.clone());
                graph
                    .anchor_targets
                    .insert((doc_id, source_anchor), final_id.clone());
                graph.documents[doc_id].arena[node_ref]
                    .attributes_mut()
                    .insert("id".to_string(), final_id.into());
            }
            continue;
        }

        // --------------------------------------------------------------------
        // Track explicit IDs on remaining element types so that fragment
        // links (`#anchor`) pointing at them are validated correctly.
        // No auto-generation — only user-supplied `{#id}` attributes.
        // --------------------------------------------------------------------

        let explicit_id = doc.arena[node_ref]
            .attributes()
            .get("id")
            .map(|v| v.str(&doc.source).to_string());

        if let Some(id) = explicit_id {
            let kind_label = if matches!(doc.arena[node_ref].kind_data(), KindData::Image(_)) {
                Some("image")
            } else if rushdown::matches_extension_kind!(
                &doc.arena,
                node_ref,
                crate::diagram::Diagram
            ) {
                Some("diagram")
            } else if rushdown::matches_extension_kind!(
                &doc.arena,
                node_ref,
                crate::bracketed_span::BracketedSpan
            ) {
                Some("span")
            } else if rushdown::matches_extension_kind!(
                &doc.arena,
                node_ref,
                rushdown_fenced_div::FencedDiv
            ) {
                Some("div")
            } else {
                None
            };

            if let Some(label) = kind_label {
                if final_seen.contains(&id) {
                    bail!(
                        "duplicate explicit {} id `{}` in {}",
                        label,
                        id,
                        doc.path.display()
                    );
                }
                final_seen.insert(id.clone());
                graph.anchor_targets.insert((doc_id, id.clone()), id);
            }
        }
    }
    Ok(())
}

/// Extract plain text from a [`Caption`] node's [`CaptionText`] child.
fn caption_text_content(arena: &Arena, caption_ref: NodeRef, source: &str) -> String {
    // The CaptionText is the first child of the Caption node.
    for child in arena[caption_ref].children(arena) {
        if rushdown::matches_extension_kind!(arena, child, crate::caption::CaptionText) {
            return heading_text(arena, child, source);
        }
    }
    String::new()
}

/// Extracts the descriptive text from a fenced code block's info string.
///
/// The info string has the form `language Description text {#id .class}`.
/// This function strips the language token (first whitespace-delimited word)
/// and any trailing `{…}` attribute block, returning the remaining descriptive
/// text.  Returns `None` if there is no description (only a language token or
/// empty info string).
///
/// Examples:
///   - `"rust Basic function"` → `Some("Basic function")`
///   - `"rust Basic function {.hl}"` → `Some("Basic function")`
///   - `"rust"` → `None`
///   - `""` / no info string → `None`
fn code_block_description(code_block: &CodeBlock, source: &str) -> Option<String> {
    let info = code_block.info().as_option()?.str(source);
    let trimmed = info.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Strip the trailing {…} attribute block if present.
    let without_attrs =
        if let Some(attr_part) = crate::code_block_attribute::extract_attribute_part(trimmed) {
            trimmed[..trimmed.len() - attr_part.len()].trim_end()
        } else {
            trimmed
        };

    // Strip the leading language token (first whitespace-delimited word).
    let after_lang = match without_attrs.find(char::is_whitespace) {
        Some(pos) => without_attrs[pos..].trim_start(),
        None => return None, // Only a language token, no description.
    };

    if after_lang.is_empty() {
        None
    } else {
        Some(after_lang.to_string())
    }
}

// ---------------------------------------------------------------------------
// Link/image target rewriting
// ---------------------------------------------------------------------------

/// Information collected in the first (read-only) pass over the AST for each
/// link or image node.
struct TargetInfo {
    doc_id: usize,
    node_ref: NodeRef,
    is_image: bool,
    destination: String,
    link_title: Option<String>,
    /// Byte offset of the destination in the source string (for line numbers).
    dest_byte_offset: Option<usize>,
}

fn rewrite_graph_targets(
    base_dir: &Path,
    graph: &mut DocumentGraph,
) -> Result<Vec<LinkDiagnostic>> {
    let mut diagnostics: Vec<LinkDiagnostic> = Vec::new();

    // First pass: collect all link/image targets without mutating the graph.
    let targets: Vec<TargetInfo> = graph
        .rendered_nodes_preorder()
        .filter_map(|(doc_id, node_ref, _)| {
            let doc = graph.document(doc_id);
            match doc.arena[node_ref].kind_data() {
                KindData::Link(link) => Some(TargetInfo {
                    doc_id,
                    node_ref,
                    is_image: false,
                    destination: link.destination().str(&doc.source).to_string(),
                    link_title: link
                        .title()
                        .as_option()
                        .map(|title| title.str(&doc.source).to_string()),
                    dest_byte_offset: destination_byte_offset(link.destination()),
                }),
                KindData::Image(image) => Some(TargetInfo {
                    doc_id,
                    node_ref,
                    is_image: true,
                    destination: image.destination().str(&doc.source).to_string(),
                    link_title: None,
                    dest_byte_offset: destination_byte_offset(image.destination()),
                }),
                _ => None,
            }
        })
        .collect();

    // Second pass: resolve and insert targets, collecting diagnostics for
    // broken links instead of aborting on the first error.
    for info in &targets {
        let doc = graph.document(info.doc_id);
        let source_path = doc.path.clone();
        let source_text = doc.source.clone();
        let text = link_text(&doc.arena, info.node_ref, &doc.source);

        match rewrite_target(
            base_dir,
            graph,
            info.doc_id,
            &info.destination,
            info.link_title.as_deref(),
            info.is_image,
        ) {
            Ok(rewritten) => {
                let location = NodeLocation::new(info.doc_id, info.node_ref);
                if info.is_image {
                    graph.image_targets.insert(location, rewritten);
                } else {
                    graph.link_targets.insert(location, rewritten);
                }
            }
            Err(message) => {
                let line = info
                    .dest_byte_offset
                    .map(|off| byte_offset_to_line(&source_text, off))
                    .unwrap_or(0);
                diagnostics.push(LinkDiagnostic {
                    file: source_path,
                    line,
                    link_text: text,
                    target: info.destination.clone(),
                    message,
                });
                // Insert the original target as a fallback so rendering can
                // still proceed (useful in warn mode).
                let location = NodeLocation::new(info.doc_id, info.node_ref);
                if info.is_image {
                    graph
                        .image_targets
                        .insert(location, info.destination.clone());
                } else {
                    graph
                        .link_targets
                        .insert(location, info.destination.clone());
                }
            }
        }
    }

    Ok(diagnostics)
}

pub(crate) fn heading_text(arena: &Arena, node_ref: NodeRef, source: &str) -> String {
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
            KindData::Text(value) => text.push_str(&value.value().str(source)),
            KindData::CodeSpan(cs) => {
                text.push_str(&cs.value().value(source));
            }
            _ => {}
        }
        Ok(WalkStatus::Continue)
    });
    text
}

fn generate_heading_id(value: &str, seen: &HashSet<String>) -> String {
    let trimmed = value
        .trim()
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '-' || c == '_');
    let trimmed = trimmed.trim_end_matches(['-', '_']);
    let mut current_dash = false;
    let mut output = String::new();
    for character in trimmed.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_lowercase() || character.is_ascii_digit() || !character.is_ascii() {
            output.push(character);
            current_dash = false;
        } else if (character.is_whitespace() || character == '-' || character == '_')
            && !current_dash
            && !output.is_empty()
        {
            output.push('-');
            current_dash = true;
        }
    }
    if output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        output.push_str("heading");
    }
    if !seen.contains(&output) {
        return output;
    }
    let mut suffix = 1usize;
    loop {
        let candidate = format!("{output}-{suffix}");
        if !seen.contains(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

/// Rewrite a single link/image destination.
///
/// Returns `Ok(rewritten)` on success, or `Err(message)` with a
/// human-readable explanation when the target cannot be resolved (broken
/// link).  The caller is responsible for wrapping this into a
/// [`LinkDiagnostic`].
fn rewrite_target(
    base_dir: &Path,
    graph: &mut DocumentGraph,
    source_doc_id: usize,
    raw_target: &str,
    raw_title: Option<&str>,
    is_image: bool,
) -> std::result::Result<String, String> {
    let target = raw_target.trim();
    let wrapped = target.starts_with('<') && target.ends_with('>');
    let target = target.trim_matches(['<', '>']);
    let title_fragment = raw_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(|title| generate_heading_id(title, &HashSet::new()));

    // Empty, external URLs, and mailto links pass through unchanged.
    if target.is_empty() || target.contains("://") || target.starts_with("mailto:") {
        return Ok(raw_target.to_string());
    }

    // ── Fragment-only links (#anchor) — file-scoped ────────────────────
    if let Some(fragment) = target.strip_prefix('#') {
        if fragment.is_empty() {
            if let Some(source_anchor) = title_fragment.as_deref() {
                let final_id = graph
                    .anchor_target(source_doc_id, source_anchor)
                    .ok_or_else(|| {
                        format!(
                            "title '{}' resolved to '#{}', but this anchor was not found in {}",
                            raw_title.unwrap_or_default(),
                            source_anchor,
                            graph.document(source_doc_id).path.display()
                        )
                    })?;
                let rewritten = format!("#{final_id}");
                return Ok(if wrapped {
                    format!("<{rewritten}>")
                } else {
                    rewritten
                });
            }
            return Ok(raw_target.to_string());
        }
        return match graph.anchor_target(source_doc_id, fragment) {
            Some(final_id) => {
                let rewritten = format!("#{final_id}");
                Ok(if wrapped {
                    format!("<{rewritten}>")
                } else {
                    rewritten
                })
            }
            None => Err(format!(
                "anchor '#{fragment}' not found in {}",
                graph.document(source_doc_id).path.display()
            )),
        };
    }

    let (path_part, fragment) = split_fragment(target);
    let source = &graph.document(source_doc_id).path;
    let resolved = resolve_base_path(base_dir, source.parent().unwrap_or(base_dir), path_part)
        .map_err(|e| format!("{e:#}"))?;

    if resolved.extension().and_then(|ext| ext.to_str()) == Some("md") && !is_image {
        let target_doc_id = graph
            .resolve_markdown_target(source_doc_id, &resolved)
            .map_err(|e| format!("{e:#}"))?;
        let rewritten = if let Some(fragment) = fragment.filter(|fragment| !fragment.is_empty()) {
            let target_id = graph
                .anchor_target(target_doc_id, fragment)
                .ok_or_else(|| {
                    format!("anchor '#{fragment}' not found in {}", resolved.display())
                })?;
            format!("#{target_id}")
        } else if let Some(source_anchor) = title_fragment.as_deref() {
            let target_id = graph
                .anchor_target(target_doc_id, source_anchor)
                .ok_or_else(|| {
                    format!(
                        "title '{}' resolved to '#{}', but this anchor was not found in {}",
                        raw_title.unwrap_or_default(),
                        source_anchor,
                        resolved.display()
                    )
                })?;
            format!("#{target_id}")
        } else {
            let target_id = graph
                .first_heading_id(target_doc_id)
                .ok_or_else(|| format!("target {} has no headings", resolved.display()))?;
            format!("#{target_id}")
        };
        return Ok(if wrapped {
            format!("<{rewritten}>")
        } else {
            rewritten
        });
    }

    graph.asset_targets.insert(resolved.clone());
    let static_path = static_asset_path(base_dir, &resolved).map_err(|e| format!("{e:#}"))?;
    let rewritten = static_path.to_string_lossy().replace('\\', "/");
    Ok(if wrapped {
        format!("<{rewritten}>")
    } else {
        rewritten
    })
}

fn split_fragment(target: &str) -> (&str, Option<&str>) {
    match target.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (target, None),
    }
}

pub(crate) fn resolve_base_path(base_dir: &Path, base: &Path, path: &str) -> Result<PathBuf> {
    let candidate = if path.starts_with('/') {
        base_dir.join(path.trim_start_matches('/'))
    } else {
        base.join(path)
    };
    let canonical = candidate
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", candidate.display()))?;
    if !canonical.starts_with(base_dir) {
        bail!(
            "path {} escapes the project root {}",
            canonical.display(),
            base_dir.display()
        );
    }
    Ok(canonical)
}

pub(crate) fn static_asset_path(base_dir: &Path, asset: &Path) -> Result<PathBuf> {
    let relative = asset.strip_prefix(base_dir).with_context(|| {
        format!(
            "asset {} is outside the project root {}",
            asset.display(),
            base_dir.display()
        )
    })?;
    Ok(PathBuf::from("static").join(relative))
}
