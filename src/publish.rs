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
    cli_server: Option<&str>,
    cli_space: Option<&str>,
    cli_parents: &[String],
    cli_name: Option<&str>,
    broken_links: BrokenLinkMode,
) -> Result<()> {
    // ── authentication ──────────────────────────────────────────────────
    let token = std::env::var("CONFLUENCE_TOKEN").context(
        "CONFLUENCE_TOKEN environment variable is not set.\n\
         Set it to a Confluence Personal Access Token (PAT).",
    )?;

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
    let metadata = build.document_metadata();

    // ── resolve publish parameters ──────────────────────────────────────
    let (server, space, parents, name) =
        resolve_publish_params(cli_server, cli_space, cli_parents, cli_name, &metadata)?;

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

/// Merge CLI flags with front-matter defaults.
///
/// Front matter uses a nested `confluence:` object:
///
/// ```yaml
/// confluence:
///   server: https://confluence.example.com
///   space: PROJ
///   path:
///     - Parent Page
///     - Page Title
/// ```
///
/// All fields are optional; CLI flags take precedence.
///
/// Returns `(server, space, parents, name)`.
fn resolve_publish_params(
    cli_server: Option<&str>,
    cli_space: Option<&str>,
    cli_parents: &[String],
    cli_name: Option<&str>,
    metadata: &serde_json::Map<String, JsonValue>,
) -> Result<(String, String, Vec<String>, String)> {
    // Extract the `confluence` front-matter object (if present).
    let conf = metadata.get("confluence").and_then(|v| v.as_object());

    // Helper: read a string field from the `confluence` object.
    let conf_str =
        |key: &str| -> Option<&str> { conf.and_then(|c| c.get(key)).and_then(|v| v.as_str()) };

    // ── server ──────────────────────────────────────────────────────────
    let server = if let Some(s) = cli_server {
        s.to_string()
    } else if let Some(s) = conf_str("server") {
        s.to_string()
    } else {
        bail!(
            "Confluence server not specified.\n\
             Use --server or set `confluence.server` in front matter."
        );
    };

    // ── space ───────────────────────────────────────────────────────────
    let space = if let Some(s) = cli_space {
        s.to_string()
    } else if let Some(s) = conf_str("space") {
        s.to_string()
    } else {
        bail!(
            "Confluence space not specified.\n\
             Use --space or set `confluence.space` in front matter."
        );
    };

    // ── parents and name from CLI ───────────────────────────────────────
    if !cli_parents.is_empty() || cli_name.is_some() {
        // CLI path was (partially) specified — use it.
        let name = if let Some(n) = cli_name {
            n.to_string()
        } else if let Some(title) = metadata.get("title").and_then(|v| v.as_str()) {
            title.to_string()
        } else {
            bail!(
                "Page name not specified.\n\
                 Use --name, or set `title` in front matter."
            );
        };
        return Ok((server, space, cli_parents.to_vec(), name));
    }

    // ── fall back to confluence.path from front matter ───────────────────
    let fm_path = conf.and_then(|c| c.get("path")).and_then(|v| v.as_array());
    if let Some(path_arr) = fm_path {
        if path_arr.is_empty() {
            bail!("`confluence.path` in front matter is empty");
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
         Use --name, set `title` in front matter, or provide `confluence.path`."
    );
}

/// Walk the parent chain, verifying each ancestor page exists.
///
/// Returns `Some(page_id)` of the deepest parent, or `None` if
/// `parents` is empty (page goes at the space root).
fn resolve_parent_chain(
    client: &ConfluenceClient,
    space: &str,
    parents: &[String],
) -> Result<Option<String>> {
    if parents.is_empty() {
        return Ok(None);
    }
    // First parent is found by title at the space level.
    let mut current = client.find_page(space, &parents[0])?.ok_or_else(|| {
        anyhow::anyhow!("parent page '{}' not found in space {}", parents[0], space)
    })?;

    // Subsequent parents are found as children of the previous.
    for title in &parents[1..] {
        current = client
            .find_child_by_title(&current.id, title)?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "parent page '{}' not found under '{}'",
                    title,
                    current.title
                )
            })?;
    }

    Ok(Some(current.id))
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
