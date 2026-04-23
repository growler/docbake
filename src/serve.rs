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
use crate::conditional::{EffectiveFormat, RenderProfile};
use crate::diagram::DiagramThemes;
use crate::graph::prepare_document;
use crate::html_page::{build_html_document, build_toc};
use crate::html_site::build_multi_page_html;
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
    meta_drops: &[String],
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
        meta_drops,
        meta_overrides,
    )?;

    // Version counter polled by the live-reload script.
    let version = Arc::new(AtomicU64::new(1));

    let bind_addr = format!("127.0.0.1:{port}");
    let server = Server::http(&bind_addr)
        .map_err(|e| anyhow::anyhow!("failed to start HTTP server on {}: {}", bind_addr, e))?;
    let server = Arc::new(server);

    let addr = server
        .server_addr()
        .to_ip()
        .context("failed to get local server address")?;

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
    let meta_drops_w: Vec<String> = meta_drops.to_vec();
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
                    &meta_drops_w,
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

    let output_root = output_dir
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", output_dir.display()))?;

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

        let file_path = resolve_request_path(&url, &output_dir)
            .and_then(|path| canonical_request_path(&path, &output_root));

        match file_path {
            Some(ref path) => match fs::read(path) {
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
            },
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
    meta_drops: &[String],
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

    let build = prepare_document(
        logger,
        base_dir,
        source,
        Some(&doc_name),
        BrokenLinkMode::Warn,
    )?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create output dir {}", output_dir.display()))?;

    let mut metadata = build.document_metadata();
    crate::apply_meta_drops(&mut metadata, meta_drops);
    crate::apply_meta_overrides(&mut metadata, meta_overrides)?;
    let render_profile =
        RenderProfile::from_metadata(EffectiveFormat::from_output_format(format), &metadata)?;
    crate::enforce_visible_target_policy(build.graph(), &render_profile, broken_links)?;

    let document_assets =
        crate::visible_asset_targets_for_profile(build.graph(), base_dir, &render_profile)?;
    copy_assets(base_dir, output_dir, &document_assets, logger)?;
    theme.copy_assets_to(output_dir)?;

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
            let toc = build_toc(build.graph(), &render_profile)?;
            let search_json = crate::search_index::build_search_index(&build, &render_profile)?;
            template_context.insert("search_index", &search_json);
            let html = build_html_document(
                &template,
                &template_context,
                &build,
                &render_profile,
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
            let plan = crate::html_site::build_multi_page_plan(
                &build,
                &render_profile,
                root_source_dir,
                base_dir,
            )?;
            let search_json =
                crate::search_index::build_search_index_multi(&build, &render_profile, &plan)?;
            template_context.insert("search_index", &search_json);
            let pages = build_multi_page_html(
                &template,
                &template_context,
                &build,
                &render_profile,
                base_dir,
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
    // Strip query and fragment if any.
    let path_part = url.split(['?', '#']).next().unwrap_or(url);
    let has_trailing_slash = path_part.ends_with('/');
    let clean = path_part.trim_start_matches('/');

    let mut relative = PathBuf::new();
    for segment in clean.split('/') {
        if segment.is_empty() {
            continue;
        }

        let decoded = decode_percent_segment(segment)?;
        if decoded.contains('/') || decoded.contains('\\') {
            return None;
        }

        let mut components = Path::new(&decoded).components();
        match (components.next(), components.next()) {
            (Some(std::path::Component::Normal(part)), None) => relative.push(part),
            _ => return None,
        }
    }

    let candidate = if relative.as_os_str().is_empty() {
        output_dir.join("index.html")
    } else {
        let base = output_dir.join(relative);
        if has_trailing_slash || base.is_dir() {
            base.join("index.html")
        } else {
            base
        }
    };

    Some(candidate)
}

fn canonical_request_path(path: &Path, output_root: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    if canonical.starts_with(output_root) {
        Some(canonical)
    } else {
        None
    }
}

fn decode_percent_segment(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_value(bytes[i + 1])?;
            let lo = hex_value(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(10 + (byte - b'a')),
        b'A'..=b'F' => Some(10 + (byte - b'A')),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{canonical_request_path, resolve_request_path};

    #[test]
    fn resolves_root_and_query_to_index() {
        let out = tempdir().unwrap();
        let out_path = out.path();

        assert_eq!(
            resolve_request_path("/", out_path).unwrap(),
            out_path.join("index.html")
        );
        assert_eq!(
            resolve_request_path("/?v=1", out_path).unwrap(),
            out_path.join("index.html")
        );
    }

    #[test]
    fn preserves_valid_nested_routes() {
        let out = tempdir().unwrap();
        let out_path = out.path();

        fs::create_dir_all(out_path.join("guide")).unwrap();
        fs::write(out_path.join("guide/index.html"), "ok").unwrap();

        assert_eq!(
            resolve_request_path("/guide", out_path).unwrap(),
            out_path.join("guide/index.html")
        );
        assert_eq!(
            resolve_request_path("/guide/", out_path).unwrap(),
            out_path.join("guide/index.html")
        );
        assert_eq!(
            resolve_request_path("/guide/page.html", out_path).unwrap(),
            out_path.join("guide/page.html")
        );
    }

    #[test]
    fn rejects_traversal_and_absolute_payloads() {
        let out = tempdir().unwrap();
        let out_path = out.path();

        let malicious = [
            "/../secret.txt",
            "/%2e%2e/secret.txt",
            "/%2E%2E/secret.txt",
            "/..%2fsecret.txt",
            "/%2e%2e%2fsecret.txt",
            "/%2Fetc%2Fpasswd",
            "/%5Cwindows%5Csystem32",
            "/guide/%2e%2e/secret.txt",
            "/%2",
        ];

        for path in malicious {
            assert!(
                resolve_request_path(path, out_path).is_none(),
                "expected {path} to be rejected"
            );
        }
    }

    #[test]
    fn canonical_path_must_stay_under_output_root() {
        let out = tempdir().unwrap();
        let out_path = out.path();
        fs::write(out_path.join("index.html"), "ok").unwrap();

        let output_root = out_path.canonicalize().unwrap();
        let inside = out_path.join("index.html");
        assert_eq!(
            canonical_request_path(&inside, &output_root),
            Some(output_root.join("index.html"))
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let outside = tempdir().unwrap();
            let outside_file = outside.path().join("secret.txt");
            fs::write(&outside_file, "secret").unwrap();

            let link = out_path.join("link.txt");
            symlink(&outside_file, &link).unwrap();

            assert!(canonical_request_path(&link, &output_root).is_none());
        }
    }
}
