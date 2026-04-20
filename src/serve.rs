use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use notify::RecursiveMode;
use notify_debouncer_mini::new_debouncer;
use tiny_http::{Header, Response, Server, StatusCode};

use crate::build::{build_template_context, copy_assets};
use crate::cache::Cache;
use crate::cli::{BrokenLinkMode, OutputFormat};
use crate::diagram::DiagramThemes;
use crate::graph::prepare_document;
use crate::render_html::{build_html_document, build_toc};
use crate::render_html_multi::build_multi_page_html;
use crate::theme::Theme;
use crate::DebugDumper;

/// JavaScript snippet injected into HTML pages for live-reload polling.
const LIVE_UPDATE_JS: &str = r#"<script>
(function() {
  var v = '';
  setInterval(function() {
    fetch('/__version')
      .then(function(r) { return r.text(); })
      .then(function(nv) { if (v && nv !== v) location.reload(); v = nv; })
      .catch(function() {});
  }, 300);
})();
</script>"#;

#[allow(clippy::too_many_arguments)]
pub(crate) fn serve_command(
    logger: &DebugDumper,
    cache: &Cache,
    theme: &Theme,
    format: OutputFormat,
    input: &Path,
    base: Option<&Path>,
    port: u16,
    broken_links: BrokenLinkMode,
    meta_overrides: &[(String, String)],
) -> Result<()> {
    match format {
        OutputFormat::Html | OutputFormat::HtmlMulti => {}
        _ => bail!("serve only supports html and html-multi formats"),
    }

    let (source, base_dir) = crate::resolve_input_and_base(input, base)?;

    let temp_dir = tempfile::Builder::new()
        .prefix("docbake-serve-")
        .tempdir()
        .context("failed to create temporary directory")?;
    let output_dir = temp_dir.path().to_path_buf();

    // Initial build.
    rebuild(
        logger,
        cache,
        theme,
        format,
        &source,
        &base_dir,
        &output_dir,
        broken_links,
        meta_overrides,
    )?;

    // Version counter polled by the live-reload script.
    let version = Arc::new(AtomicU64::new(1));

    let bind_addr = format!("127.0.0.1:{port}");
    let server = Server::http(&bind_addr)
        .map_err(|e| anyhow::anyhow!("failed to start HTTP server on {}: {}", bind_addr, e))?;
    let server = Arc::new(server);

    let addr = match server.server_addr() {
        tiny_http::ListenAddr::IP(addr) => addr,
        _ => unreachable!("TCP server always has an IP address"),
    };

    eprintln!("Serving on http://{addr}");

    if let Err(e) = open::that(format!("http://{addr}")) {
        eprintln!("Warning: failed to open browser: {e}");
    }

    // --- file watcher --------------------------------------------------
    let version_w = version.clone();
    let logger_w = logger.clone();
    let cache_w = cache.clone();
    let theme_w = theme.clone();
    let source_w = source.clone();
    let base_dir_w = base_dir.clone();
    let output_dir_w = output_dir.clone();
    let meta_w: Vec<(String, String)> = meta_overrides.to_vec();
    let rebuild_lock = Arc::new(Mutex::new(()));

    let _debouncer = {
        let lock = rebuild_lock.clone();
        let mut debouncer = new_debouncer(Duration::from_millis(200), move |result| match result {
            Ok(_events) => {
                let _guard = lock.lock().unwrap();
                eprintln!("File change detected, rebuilding\u{2026}");
                match rebuild(
                    &logger_w,
                    &cache_w,
                    &theme_w,
                    format,
                    &source_w,
                    &base_dir_w,
                    &output_dir_w,
                    broken_links,
                    &meta_w,
                ) {
                    Ok(()) => {
                        version_w.fetch_add(1, Ordering::Relaxed);
                        eprintln!("Rebuild complete.");
                    }
                    Err(e) => {
                        eprintln!("Rebuild failed: {e:#}");
                    }
                }
            }
            Err(e) => {
                eprintln!("File watcher error: {e}");
            }
        })
        .context("failed to create file watcher")?;

        debouncer
            .watcher()
            .watch(base_dir.as_ref(), RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch {}", base_dir.display()))?;

        if let Theme::Custom(ref theme_dir) = *theme {
            debouncer
                .watcher()
                .watch(theme_dir.as_ref(), RecursiveMode::Recursive)
                .with_context(|| {
                    format!("failed to watch theme directory {}", theme_dir.display())
                })?;
        }

        debouncer // keep alive; dropped on function exit
    };

    // --- HTTP server loop -----------------------------------------------
    for request in server.incoming_requests() {
        let url = request.url().to_string();

        // Live-reload version endpoint.
        if url == "/__version" {
            let body = version.load(Ordering::Relaxed).to_string();
            let resp = Response::from_string(body)
                .with_header(header("Content-Type", "text/plain"))
                .with_header(header("Cache-Control", "no-cache, no-store"));
            let _ = request.respond(resp);
            continue;
        }

        let file_path = resolve_request_path(&url, &output_dir);

        match file_path {
            Some(ref path) if path.exists() && path.starts_with(&output_dir) => {
                match fs::read(path) {
                    Ok(data) => {
                        let mime = mime_for_path(path);
                        let resp = Response::from_data(data)
                            .with_header(header("Content-Type", mime))
                            .with_header(header("Cache-Control", "no-cache"));
                        let _ = request.respond(resp);
                    }
                    Err(_) => {
                        let _ = request.respond(
                            Response::from_string("Internal Server Error")
                                .with_status_code(StatusCode(500)),
                        );
                    }
                }
            }
            _ => {
                let _ = request
                    .respond(Response::from_string("Not Found").with_status_code(StatusCode(404)));
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn rebuild(
    logger: &DebugDumper,
    cache: &Cache,
    theme: &Theme,
    format: OutputFormat,
    source: &Path,
    base_dir: &Path,
    output_dir: &Path,
    broken_links: BrokenLinkMode,
    meta_overrides: &[(String, String)],
) -> Result<()> {
    let doc_name = source
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    logger.log(format!(
        "building {} from {} as {:?}",
        doc_name,
        source.display(),
        format
    ));

    let build = prepare_document(logger, base_dir, source, Some(&doc_name), broken_links)?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create output dir {}", output_dir.display()))?;

    let document_assets = build.asset_targets().clone();
    copy_assets(base_dir, output_dir, &document_assets, logger)?;
    theme.copy_assets_to(output_dir)?;

    let mut metadata = build.document_metadata();
    crate::apply_meta_overrides(&mut metadata, meta_overrides)?;
    let style = theme.style_css()?;
    let mut template_context = build_template_context(&metadata, style.as_deref())?;
    template_context.insert("live_update", LIVE_UPDATE_JS);

    let diagram_themes = DiagramThemes {
        plantuml: theme.plantuml_theme()?,
        mermaid: theme.mermaid_theme()?,
        d2: theme.d2_theme()?,
    };

    match format {
        OutputFormat::Html => {
            let template = theme.template_for(format)?;
            let toc = build_toc(build.graph());
            let search_json = crate::search_index::build_search_index(&build);
            template_context.insert("search_index", &search_json);
            let html = build_html_document(
                &template,
                &template_context,
                &build,
                &toc,
                cache,
                &diagram_themes,
            )?;
            let output_file = output_dir.join("index.html");
            fs::write(&output_file, &html)
                .with_context(|| format!("failed to write {}", output_file.display()))?;
            logger.log(format!("wrote {}", output_file.display()));
        }
        OutputFormat::HtmlMulti => {
            let template = theme.template_for(format)?;
            let root_source_dir = build
                .graph()
                .root_document()
                .path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("root document has no parent directory"))?;
            let plan = crate::render_html_multi::build_multi_page_plan(&build, root_source_dir);
            let search_json = crate::search_index::build_search_index_multi(&build, &plan);
            template_context.insert("search_index", &search_json);
            let pages = build_multi_page_html(
                &template,
                &template_context,
                &build,
                cache,
                &diagram_themes,
            )?;
            for page in &pages {
                let target = output_dir.join(&page.path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("failed to create {}", parent.display()))?;
                }
                fs::write(&target, &page.html)
                    .with_context(|| format!("failed to write {}", target.display()))?;
                logger.log(format!("wrote {}", target.display()));
            }
        }
        _ => unreachable!("serve only handles html and html-multi"),
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

/// Map a request URL to a filesystem path inside `output_dir`.
/// Returns `None` for suspicious paths (traversal attempts).
fn resolve_request_path(url: &str, output_dir: &Path) -> Option<PathBuf> {
    // Strip query string if any.
    let path_part = url.split('?').next().unwrap_or(url);
    let clean = path_part.trim_start_matches('/');

    if clean.contains("..") {
        return None;
    }

    let path = if clean.is_empty() {
        output_dir.join("index.html")
    } else {
        let candidate = output_dir.join(clean);
        if candidate.is_dir() {
            candidate.join("index.html")
        } else {
            candidate
        }
    };

    Some(path)
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid HTTP header")
}

fn mime_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("pdf") => "application/pdf",
        Some("xml") => "application/xml",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
