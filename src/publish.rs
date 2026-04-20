//! Orchestrator for `publish confluence` — resolves parameters, renders the
//! document to Confluence Storage Format, diffs attachments, and uploads
//! to Confluence Server via REST API.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value as JsonValue;

use crate::cache::Cache;
use crate::cli::BrokenLinkMode;
use crate::confluence::ConfluenceClient;
use crate::graph::prepare_document;
use crate::render_confluence::ConfluenceRenderer;
use crate::theme::Theme;
use crate::DebugDumper;

/// Entry point called from `lib.rs` for the `publish confluence` subcommand.
#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_confluence_command(
    logger: &DebugDumper,
    cache: &Cache,
    theme: &Theme,
    input: &Path,
    base: Option<&Path>,
    broken_links: BrokenLinkMode,
    meta_overrides: &[(String, String)],
) -> Result<()> {
    // ── resolve input / base ────────────────────────────────────────────
    let (source, base_dir) = crate::resolve_input_and_base(input, base)?;
    let doc_name = source
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    logger.log(format!(
        "preparing {} from {} for Confluence",
        doc_name,
        source.display()
    ));

    // ── prepare document graph ──────────────────────────────────────────
    let build = prepare_document(logger, &base_dir, &source, Some(&doc_name), broken_links)?;
    let mut metadata = build.document_metadata();
    crate::apply_meta_overrides(&mut metadata, meta_overrides)?;

    // ── resolve publish parameters ──────────────────────────────────────
    let (server, space, parents, name) = resolve_publish_params(&metadata)?;

    // ── authentication ──────────────────────────────────────────────────
    // CONFLUENCE_TOKEN env var takes precedence; otherwise look up a
    // stored credential for this server (see `docbake auth confluence`).
    let token = crate::credentials::resolve_token(&server)?;

    logger.log(format!(
        "target: server={}, space={}, parents={:?}, name={}",
        server, space, parents, name
    ));

    // ── diagram themes ──────────────────────────────────────────────────
    let diagram_themes = crate::diagram::DiagramThemes {
        plantuml: theme.plantuml_theme()?,
        mermaid: theme.mermaid_theme()?,
        d2: theme.d2_theme()?,
    };

    // ── render to Confluence Storage Format ─────────────────────────────
    let renderer = ConfluenceRenderer::new(cache, &diagram_themes, logger, &base_dir);
    let doc = renderer.render(&build)?;

    logger.log(format!(
        "rendered: {} bytes body, {} attachments",
        doc.body.len(),
        doc.attachments.len()
    ));

    // ── connect to Confluence ───────────────────────────────────────────
    let client = ConfluenceClient::new(&server, &token);

    // ── resolve parent chain ────────────────────────────────────────────
    let parent_id = resolve_parent_chain(&client, &space, &parents)?;

    // ── check if page exists ────────────────────────────────────────────
    let existing = if let Some(ref pid) = parent_id {
        client.find_child_by_title(pid, &name)?
    } else {
        client.find_page(&space, &name)?
    };

    // Hash the rendered body for change detection via page property.
    let body_hash = format!("blake3:{}", blake3::hash(doc.body.as_bytes()).to_hex());
    const BODY_HASH_KEY: &str = "docbake-body-hash";

    if let Some(page) = existing {
        // ── update existing page ────────────────────────────────────────
        let remote_hash = client.get_page_property(&page.id, BODY_HASH_KEY)?;
        let body_changed = remote_hash.as_deref() != Some(&body_hash);

        let remote_attachments = client.list_attachments(&page.id)?;
        let (to_upload, unchanged_count) = diff_attachments(&doc.attachments, &remote_attachments);

        if !body_changed && to_upload.is_empty() {
            eprintln!(
                "No changes detected for '{}' in space {} — skipping publish",
                name, space
            );
            return Ok(());
        }

        if body_changed {
            let (_, current_version) = client.get_page_body(&page.id)?;
            client.update_page(&page.id, &name, &doc.body, current_version)?;
            client.set_page_property(&page.id, BODY_HASH_KEY, &body_hash)?;
        }

        for pending in &to_upload {
            let att = pending.attachment;
            if let Some(ref remote_id) = pending.remote_id {
                logger.log(format!("updating attachment: {}", att.filename));
                client.update_attachment(
                    &page.id,
                    remote_id,
                    &att.filename,
                    &att.data,
                    &att.content_hash,
                )?;
            } else {
                logger.log(format!("creating attachment: {}", att.filename));
                client.create_attachment(&page.id, &att.filename, &att.data, &att.content_hash)?;
            }
        }

        eprintln!(
            "Updated '{}' in space {}: body {}, {} attachment{} uploaded, {} unchanged",
            name,
            space,
            if body_changed { "updated" } else { "unchanged" },
            to_upload.len(),
            if to_upload.len() == 1 { "" } else { "s" },
            unchanged_count,
        );
    } else {
        // ── create new page ─────────────────────────────────────────────
        let page = client.create_page(&space, &name, &doc.body, parent_id.as_deref())?;
        client.set_page_property(&page.id, BODY_HASH_KEY, &body_hash)?;

        for att in &doc.attachments {
            logger.log(format!("creating attachment: {}", att.filename));
            client.create_attachment(&page.id, &att.filename, &att.data, &att.content_hash)?;
        }

        eprintln!(
            "Created '{}' in space {}: {} attachment{} uploaded",
            name,
            space,
            doc.attachments.len(),
            if doc.attachments.len() == 1 { "" } else { "s" },
        );
    }

    Ok(())
}

// ── parameter resolution ────────────────────────────────────────────────

/// Resolve Confluence publish parameters from document metadata.
///
/// The `confluence` front-matter key must be a dictionary:
///
/// ```yaml
/// confluence:
///   server: https://confluence.example.com
///   space: PROJ
///   path:
///     - Parent Page        # looked up by title
///     - id:12345678        # or by content ID
///     - Page Title         # last element = page name
/// ```
///
/// Each element in `path` is either a page title or a literal Confluence
/// content ID prefixed with `id:`.
///
/// All fields can be overridden via `--meta` (e.g.
/// `--meta confluence.server https://…`).
///
/// Returns `(server, space, parents, name)`.
fn resolve_publish_params(
    metadata: &serde_json::Map<String, JsonValue>,
) -> Result<(String, String, Vec<String>, String)> {
    let conf = metadata
        .get("confluence")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Confluence settings not found.\n\
                 Set the `confluence` object in front matter or use --meta."
            )
        })?;

    // Helper: read a string field from the `confluence` object.
    let conf_str = |key: &str| -> Option<&str> { conf.get(key).and_then(|v| v.as_str()) };

    // ── server ──────────────────────────────────────────────────────────
    let server = conf_str("server")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Confluence server not specified.\n\
                 Set `confluence.server` in front matter or use --meta confluence.server <url>."
            )
        })?
        .to_string();

    // ── space ───────────────────────────────────────────────────────────
    let space = conf_str("space")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Confluence space not specified.\n\
                 Set `confluence.space` in front matter or use --meta confluence.space <key>."
            )
        })?
        .to_string();

    // ── path → parents + name ───────────────────────────────────────────
    if let Some(path_arr) = conf.get("path").and_then(|v| v.as_array()) {
        if path_arr.is_empty() {
            bail!("`confluence.path` is empty");
        }
        let path_strings: Vec<String> = path_arr
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .context("each element of `confluence.path` must be a string")
            })
            .collect::<Result<_>>()?;
        // Last element is the page name, rest are parents.
        let (parents, name_slice) = path_strings.split_at(path_strings.len() - 1);
        return Ok((server, space, parents.to_vec(), name_slice[0].clone()));
    }

    // ── fall back to title ──────────────────────────────────────────────
    if let Some(title) = metadata.get("title").and_then(|v| v.as_str()) {
        return Ok((server, space, Vec::new(), title.to_string()));
    }

    bail!(
        "Page name not specified.\n\
         Set `title` in front matter or provide `confluence.path`."
    );
}

/// Walk the parent chain, verifying each ancestor page exists.
///
/// Returns `Some(page_id)` of the deepest parent, or `None` if
/// `parents` is empty (page goes at the space root).
///
/// Each element in `parents` is either a page title (looked up via the
/// Confluence search API) or a literal content ID prefixed with `id:`
/// (e.g. `"id:12345678"`).  When an `id:` reference is encountered the
/// page is fetched directly by ID, skipping the title-based search.
fn resolve_parent_chain(
    client: &ConfluenceClient,
    space: &str,
    parents: &[String],
) -> Result<Option<String>> {
    if parents.is_empty() {
        return Ok(None);
    }

    // Resolve the first parent — either by ID or by title at the space level.
    let mut current = if let Some(id) = strip_id_prefix(&parents[0]) {
        client
            .get_page_by_id(id)
            .with_context(|| format!("parent page id:{id} not found"))?
    } else {
        client
            .find_page(space, &parents[0])?
            .ok_or_else(|| {
                anyhow::anyhow!("parent page '{}' not found in space {}", parents[0], space)
            })?
    };

    // Subsequent parents are resolved as children of the previous (by title)
    // or fetched directly (by ID).
    for entry in &parents[1..] {
        current = if let Some(id) = strip_id_prefix(entry) {
            client
                .get_page_by_id(id)
                .with_context(|| format!("parent page id:{id} not found"))?
        } else {
            client
                .find_child_by_title(&current.id, entry)?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "parent page '{}' not found under '{}'",
                        entry,
                        current.title
                    )
                })?
        };
    }

    Ok(Some(current.id))
}

/// If `value` starts with `id:` return the remainder (the numeric ID),
/// otherwise return `None`.
fn strip_id_prefix(value: &str) -> Option<&str> {
    value
        .strip_prefix("id:")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}

// ── attachment diffing ──────────────────────────────────────────────────

/// An attachment that needs to be uploaded.
struct PendingUpload<'a> {
    attachment: &'a crate::render_confluence::ConfluenceAttachment,
    /// `Some(id)` if updating an existing attachment, `None` if creating.
    remote_id: Option<String>,
}

/// Compare local attachments against remote ones.
///
/// Returns `(to_upload, unchanged_count)` where `to_upload` contains
/// entries that are new or have changed (by content hash).
fn diff_attachments<'a>(
    local: &'a [crate::render_confluence::ConfluenceAttachment],
    remote: &[crate::confluence::api::AttachmentInfo],
) -> (Vec<PendingUpload<'a>>, usize) {
    let mut to_upload = Vec::new();
    let mut unchanged = 0usize;
    for att in local {
        let remote_match = remote.iter().find(|r| r.title == att.filename);
        match remote_match {
            Some(r) if r.comment == att.content_hash => {
                unchanged += 1;
            }
            Some(r) => {
                to_upload.push(PendingUpload {
                    attachment: att,
                    remote_id: Some(r.id.clone()),
                });
            }
            None => {
                to_upload.push(PendingUpload {
                    attachment: att,
                    remote_id: None,
                });
            }
        }
    }
    (to_upload, unchanged)
}
