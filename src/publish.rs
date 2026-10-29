//! Orchestrator for `publish confluence` and `publish gdoc` — resolves
//! parameters, renders the document, and uploads to the target platform.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value as JsonValue;

use crate::cache::Cache;
use crate::cli::BrokenLinkMode;
use crate::conditional::{EffectiveFormat, RenderProfile};
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
    meta_drops: &[String],
    meta_overrides: &[(String, String)],
    meta_file: Option<&Path>,
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
    let build = prepare_document(
        logger,
        &base_dir,
        &source,
        Some(&doc_name),
        BrokenLinkMode::Warn,
    )?;
    let mut metadata = build.document_metadata();
    crate::apply_meta_file_from_path(&mut metadata, meta_file)?;
    crate::apply_meta_drops(&mut metadata, meta_drops);
    crate::apply_meta_overrides(&mut metadata, meta_overrides)?;
    let render_profile = RenderProfile::from_metadata(EffectiveFormat::Confluence, &metadata)?;
    crate::enforce_visible_target_policy(build.graph(), &render_profile, broken_links)?;

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
    let renderer =
        ConfluenceRenderer::new(cache, &diagram_themes, logger, &base_dir, render_profile);
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
/// `path` may also be a plain string, in which case the page is created
/// directly in the space root with that string as the title:
///
/// ```yaml
/// confluence:
///   server: https://confluence.example.com
///   space: PROJ
///   path: My Page Title
/// ```
///
/// When `path` is an array, each element is either a page title or a
/// literal Confluence content ID prefixed with `id:`.
///
/// All fields can be overridden via `--meta` (e.g.
/// `--meta confluence.path "Page Title"`).
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
    if let Some(path_val) = conf.get("path") {
        if let Some(path_arr) = path_val.as_array() {
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
        } else if let Some(s) = path_val.as_str() {
            // String value = page name directly in the space root, no parents.
            if s.is_empty() {
                bail!("`confluence.path` is empty");
            }
            return Ok((server, space, Vec::new(), s.to_string()));
        } else {
            bail!("`confluence.path` must be an array or a string");
        }
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
        client.find_page(space, &parents[0])?.ok_or_else(|| {
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

// ════════════════════════════════════════════════════════════════════════
// Google Docs publishing
// ════════════════════════════════════════════════════════════════════════

/// Entry point called from `lib.rs` for the `publish gdoc` subcommand.
#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_gdoc_command(
    logger: &DebugDumper,
    cache: &Cache,
    theme: &Theme,
    input: &Path,
    base: Option<&Path>,
    broken_links: BrokenLinkMode,
    meta_drops: &[String],
    meta_overrides: &[(String, String)],
    meta_file: Option<&Path>,
    write_id: bool,
) -> Result<()> {
    // ── resolve input / base ────────────────────────────────────────────
    let (source, base_dir) = crate::resolve_input_and_base(input, base)?;
    let doc_name = source
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    logger.log(format!(
        "preparing {} from {} for Google Docs",
        doc_name,
        source.display()
    ));

    // ── prepare document graph ──────────────────────────────────────────
    let build = prepare_document(
        logger,
        &base_dir,
        &source,
        Some(&doc_name),
        BrokenLinkMode::Warn,
    )?;
    let mut metadata = build.document_metadata();
    crate::apply_meta_file_from_path(&mut metadata, meta_file)?;
    crate::apply_meta_drops(&mut metadata, meta_drops);
    crate::apply_meta_overrides(&mut metadata, meta_overrides)?;
    let render_profile = RenderProfile::from_metadata(EffectiveFormat::Docx, &metadata)?;
    crate::enforce_visible_target_policy(build.graph(), &render_profile, broken_links)?;

    // ── resolve publish parameters ──────────────────────────────────────
    let (doc_id, folder_id, name) = resolve_gdoc_params(&metadata, &doc_name)?;

    logger.log(format!(
        "target: doc_id={}, folder_id={}, name={}",
        doc_id.as_deref().unwrap_or("<new>"),
        folder_id.as_deref().unwrap_or("<none>"),
        name
    ));

    // ── diagram themes ──────────────────────────────────────────────────
    let diagram_themes = crate::diagram::DiagramThemes {
        plantuml: theme.plantuml_theme()?,
        mermaid: theme.mermaid_theme()?,
        d2: theme.d2_theme()?,
    };
    let logo_asset = theme.logo_asset()?;
    let docx_params = theme.docx_params()?;

    // ── render to DOCX ──────────────────────────────────────────────────
    let renderer = crate::docx::DocxRenderer::new(
        cache,
        &diagram_themes,
        logger,
        &base_dir,
        logo_asset,
        &docx_params,
        render_profile,
    );
    let docx_bytes = renderer.render(&build, &metadata)?.into_bytes()?;

    logger.log(format!("rendered: {} bytes DOCX", docx_bytes.len()));

    // ── authenticate ────────────────────────────────────────────────────
    let access_token = crate::credentials::resolve_gdoc_access_token()?;
    let client = crate::gdoc::api::GoogleDriveClient::new(&access_token);

    // ── upload ──────────────────────────────────────────────────────────
    let result = if let Some(ref id) = doc_id {
        // Update existing document.
        let file = client
            .update_doc(id, &docx_bytes)
            .context("failed to update Google Doc")?;
        eprintln!("Updated '{name}' in Google Docs");
        file
    } else {
        // Create new document.
        let file = client
            .create_doc(&name, &docx_bytes, folder_id.as_deref())
            .context("failed to create Google Doc")?;
        eprintln!("Created '{name}' in Google Docs");
        file
    };

    // ── print URL ───────────────────────────────────────────────────────
    let url = result
        .web_view_link
        .unwrap_or_else(|| crate::gdoc::api::google_docs_url(&result.id));
    eprintln!("{url}");

    // ── write ID back to front matter ───────────────────────────────────
    if write_id && doc_id.is_none() {
        write_gdoc_id_to_source(input, &url)?;
        eprintln!("Wrote google-docs.id to {}", input.display());
    }

    Ok(())
}

/// Resolve Google Docs publish parameters from document metadata.
///
/// The `google-docs` front-matter key may contain:
///
/// ```yaml
/// google-docs:
///   id: https://docs.google.com/document/d/1AbC.../edit
///   folder: https://drive.google.com/drive/folders/1XyZ...
/// ```
///
/// Returns `(doc_id, folder_id, name)`.
fn resolve_gdoc_params(
    metadata: &serde_json::Map<String, JsonValue>,
    fallback_name: &str,
) -> Result<(Option<String>, Option<String>, String)> {
    let gdoc = metadata.get("google-docs").and_then(|v| v.as_object());

    let doc_id = gdoc
        .and_then(|g| g.get("id"))
        .and_then(|v| v.as_str())
        .map(|s| crate::gdoc::api::parse_doc_id(s).to_string());

    let folder_id = gdoc
        .and_then(|g| g.get("folder"))
        .and_then(|v| v.as_str())
        .map(|s| crate::gdoc::api::parse_folder_id(s).to_string());

    // Document name: title from front matter, or input file stem.
    let name = metadata
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or(fallback_name)
        .to_string();

    Ok((doc_id, folder_id, name))
}

/// Write a `google-docs.id` value back into the source file's YAML front
/// matter.
///
/// If the file already has a `google-docs:` block, inserts `id:` into it.
/// If not, appends a `google-docs:` block before the closing `---`.
fn write_gdoc_id_to_source(path: &Path, url: &str) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let new_content = if let Some((fm, rest)) = split_front_matter_for_edit(&content) {
        let updated_front_matter = upsert_google_docs_id_in_front_matter(fm, url);
        format!("{updated_front_matter}{rest}")
    } else {
        let newline = preferred_newline(&content);
        format!("---{newline}google-docs:{newline}  id: {url}{newline}---{newline}{content}")
    };

    std::fs::write(path, new_content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Split a document into front matter and remainder.
///
/// Returns `Some((front_matter_including_delimiters, rest))` or `None`
/// if no front matter is present.
fn split_front_matter_for_edit(content: &str) -> Option<(&str, &str)> {
    let (front_matter, rest) = crate::graph::build::split_front_matter(content);
    let front_matter = front_matter?;
    Some((&content[..front_matter.len()], rest))
}

fn upsert_google_docs_id_in_front_matter(front_matter: &str, url: &str) -> String {
    let newline = preferred_newline(front_matter);
    let had_trailing_newline = front_matter.ends_with('\n');
    let mut lines: Vec<String> = front_matter.lines().map(str::to_string).collect();

    if lines.len() < 2 {
        return front_matter.to_string();
    }

    let closing_idx = lines.len() - 1;
    let google_docs_idx = (1..closing_idx).find(|&idx| is_google_docs_key_line(&lines[idx]));

    if let Some(gdoc_idx) = google_docs_idx {
        let gdoc_indent = indentation_width(&lines[gdoc_idx]);
        let mut block_end = closing_idx;
        let mut id_replaced = false;

        for (idx, line) in lines
            .iter_mut()
            .enumerate()
            .take(closing_idx)
            .skip(gdoc_idx + 1)
        {
            if line.trim().is_empty() {
                continue;
            }
            if indentation_width(line) <= gdoc_indent {
                block_end = idx;
                break;
            }

            if is_id_key_line(line) {
                let prefix_len = line
                    .chars()
                    .take_while(|ch| *ch == ' ' || *ch == '\t')
                    .map(char::len_utf8)
                    .sum();
                let indent = &line[..prefix_len];
                *line = format!("{indent}id: {url}");
                id_replaced = true;
                break;
            }
        }

        if !id_replaced {
            let block_indent = child_block_indent(&lines, gdoc_idx, block_end)
                .unwrap_or_else(|| format!("{}  ", leading_indent(&lines[gdoc_idx])));
            lines.insert(block_end, format!("{block_indent}id: {url}"));
        }
    } else {
        lines.insert(closing_idx, "google-docs:".to_string());
        lines.insert(closing_idx + 1, format!("  id: {url}"));
    }

    let mut out = lines.join(newline);
    if had_trailing_newline {
        out.push_str(newline);
    }
    out
}

fn preferred_newline(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn indentation_width(line: &str) -> usize {
    line.chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .count()
}

fn leading_indent(line: &str) -> &str {
    let end = line
        .char_indices()
        .find_map(|(idx, ch)| (ch != ' ' && ch != '\t').then_some(idx))
        .unwrap_or(line.len());
    &line[..end]
}

fn child_block_indent(lines: &[String], start: usize, end: usize) -> Option<String> {
    for line in lines.iter().take(end).skip(start + 1) {
        if line.trim().is_empty() {
            continue;
        }
        if indentation_width(line) > indentation_width(&lines[start]) {
            return Some(leading_indent(line).to_string());
        }
    }
    None
}

fn is_google_docs_key_line(line: &str) -> bool {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let Some(after_key) = trimmed.strip_prefix("google-docs:") else {
        return false;
    };
    after_key.trim().is_empty() || after_key.trim_start().starts_with('#')
}

fn is_id_key_line(line: &str) -> bool {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let Some((key, _value)) = trimmed.split_once(':') else {
        return false;
    };
    key.trim() == "id"
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::Result;

    use super::write_gdoc_id_to_source;

    fn write_and_update(content: &str, url: &str) -> Result<String> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("doc.md");
        std::fs::write(&path, content)?;
        write_gdoc_id_to_source(Path::new(&path), url)?;
        Ok(std::fs::read_to_string(path)?)
    }

    #[test]
    fn replaces_existing_google_docs_id() -> Result<()> {
        let input = "---\ntitle: Test\ngoogle-docs:\n  id: old\n  folder: abc\n---\nBody\n";
        let updated = write_and_update(input, "https://docs.google.com/document/d/new/edit")?;
        assert!(updated.contains("  id: https://docs.google.com/document/d/new/edit\n"));
        assert!(!updated.contains("  id: old\n"));
        assert!(updated.contains("  folder: abc\n"));
        Ok(())
    }

    #[test]
    fn inserts_id_into_existing_google_docs_block() -> Result<()> {
        let input = "---\ntitle: Test\ngoogle-docs:\n  folder: abc\nmeta: 1\n---\nBody\n";
        let updated = write_and_update(input, "https://docs.google.com/document/d/new/edit")?;
        let expected = "---\ntitle: Test\ngoogle-docs:\n  folder: abc\n  id: https://docs.google.com/document/d/new/edit\nmeta: 1\n---\nBody\n";
        assert_eq!(updated, expected);
        Ok(())
    }

    #[test]
    fn prepends_front_matter_when_absent() -> Result<()> {
        let input = "# Title\n\nBody\n";
        let updated = write_and_update(input, "https://docs.google.com/document/d/new/edit")?;
        assert!(updated.starts_with(
            "---\ngoogle-docs:\n  id: https://docs.google.com/document/d/new/edit\n---\n"
        ));
        assert!(updated.ends_with("# Title\n\nBody\n"));
        Ok(())
    }

    #[test]
    fn preserves_crlf_line_endings() -> Result<()> {
        let input = "---\r\ntitle: Test\r\ngoogle-docs:\r\n  folder: abc\r\n---\r\nBody\r\n";
        let updated = write_and_update(input, "https://docs.google.com/document/d/new/edit")?;
        let expected = "---\r\ntitle: Test\r\ngoogle-docs:\r\n  folder: abc\r\n  id: https://docs.google.com/document/d/new/edit\r\n---\r\nBody\r\n";
        assert_eq!(updated, expected);
        assert!(updated
            .as_bytes()
            .windows(2)
            .any(|window| window == b"\r\n"));
        assert!(!updated
            .as_bytes()
            .windows(1)
            .enumerate()
            .any(|(idx, window)| window == b"\n"
                && (idx == 0 || updated.as_bytes()[idx - 1] != b'\r')));
        Ok(())
    }
}
