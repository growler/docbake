pub(crate) mod bracketed_span;
pub(crate) mod build;
pub(crate) mod cache;
pub(crate) mod caption;
pub mod cli;
pub(crate) mod code_block_attribute;
pub(crate) mod confluence;
pub(crate) mod credentials;
pub(crate) mod diagram;
pub(crate) mod gdoc;
pub(crate) mod graph;
mod publish;
mod render_confluence;
mod render_docx;
mod render_html;
mod render_html_common;
mod render_html_multi;
mod render_markdown;
mod render_typst;
mod search_index;
mod serve;
pub(crate) mod theme;
mod watch;

use std::fs;
use std::path::{Path, PathBuf};

use crate::build::{build_template_context, compile_typst, copy_assets, render_pdf_template};
use crate::cache::Cache;
use crate::cli::{BrokenLinkMode, Cli, OutputFormat};
use crate::graph::prepare_document;
use crate::render_html::{build_html_document, build_toc};
use crate::render_html_multi::build_multi_page_html;
use crate::render_typst::TypstRenderer;
use crate::theme::Theme;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{Map as JsonMap, Value as JsonValue};

pub fn run(cli: Cli) -> Result<()> {
    let logger = DebugDumper::new(cli.debug_dump);
    let cache = Cache::new(cli.cache.clone(), cli.no_cache)?;
    let theme = match cli.theme {
        Some(ref dir) => {
            let dir = dir
                .canonicalize()
                .with_context(|| format!("failed to resolve theme directory {}", dir.display()))?;
            if !dir.is_dir() {
                bail!("theme path is not a directory: {}", dir.display());
            }
            Theme::Custom(dir)
        }
        None => Theme::Default,
    };

    match cli.command {
        cli::Command::Build {
            format,
            ref input,
            ref output,
            ref base,
            broken_links,
            ref meta,
            ref meta_drop,
        } => {
            let meta_overrides = parse_meta_pairs(meta)?;
            build_command(
                &logger,
                &cache,
                &theme,
                format,
                input,
                output,
                base.as_deref(),
                broken_links,
                meta_drop,
                &meta_overrides,
            )
        }
        cli::Command::Dump { ref path, ref base } => {
            let (source, base_dir) = resolve_input_and_base(path, base.as_deref())?;
            let build = prepare_document(&logger, &base_dir, &source, None, BrokenLinkMode::Error)?;
            print!("{}", build.dump_graph());
            Ok(())
        }
        cli::Command::Parse { ref path, ref base } => {
            let (source, base_dir) = resolve_input_and_base(path, base.as_deref())?;
            let build = prepare_document(&logger, &base_dir, &source, None, BrokenLinkMode::Error)?;
            print!("{}", build.parse_dump()?);
            Ok(())
        }
        cli::Command::Meta {
            ref input,
            ref key,
        } => meta_command(input, key),
        cli::Command::Serve {
            format,
            ref input,
            ref base,
            port,
            broken_links,
            ref meta,
            ref meta_drop,
        } => {
            let meta_overrides = parse_meta_pairs(meta)?;
            serve::serve_command(
                &logger,
                &cache,
                &theme,
                format,
                input,
                base.as_deref(),
                port.unwrap_or(0),
                broken_links,
                meta_drop,
                &meta_overrides,
            )
        }
        cli::Command::Watch {
            format,
            ref input,
            ref output,
            ref base,
            broken_links,
            ref meta,
            ref meta_drop,
        } => {
            let meta_overrides = parse_meta_pairs(meta)?;
            watch::watch_command(
                &logger,
                &cache,
                &theme,
                format,
                input,
                output,
                base.as_deref(),
                broken_links,
                meta_drop,
                &meta_overrides,
            )
        }
        cli::Command::Check {
            ref input,
            ref base,
        } => check_command(&logger, &cache, &theme, input, base.as_deref()),
        cli::Command::InitTheme { ref target } => {
            Theme::export_to(target)?;
            eprintln!("Default theme exported to {}", target.display());
            Ok(())
        }
        cli::Command::Publish { ref target } => match target {
            cli::PublishTarget::Confluence {
                ref input,
                ref base,
                broken_links,
                ref meta,
                ref meta_drop,
            } => {
                let meta_overrides = parse_meta_pairs(meta)?;
                publish::publish_confluence_command(
                    &logger,
                    &cache,
                    &theme,
                    input,
                    base.as_deref(),
                    *broken_links,
                    meta_drop,
                    &meta_overrides,
                )
            }
            cli::PublishTarget::Gdoc {
                ref input,
                ref base,
                broken_links,
                ref meta,
                ref meta_drop,
                write_id,
            } => {
                let meta_overrides = parse_meta_pairs(meta)?;
                publish::publish_gdoc_command(
                    &logger,
                    &cache,
                    &theme,
                    input,
                    base.as_deref(),
                    *broken_links,
                    meta_drop,
                    &meta_overrides,
                    *write_id,
                )
            }
        },
        cli::Command::Auth { ref target } => match target {
            cli::AuthTarget::Confluence {
                ref server,
                ref token,
            } => auth_confluence(server, token.as_deref()),
            cli::AuthTarget::Gdoc {
                ref service_account,
            } => auth_gdoc(service_account.as_deref()),
        },
    }
}

/// Handle `auth confluence <server> [<token>]`.
///
/// If `token` is `None`, reads the token from standard input.  When stdin
/// is a terminal, a prompt is printed to stderr first.
fn auth_confluence(server: &str, token: Option<&str>) -> Result<()> {
    let token = match token {
        Some(t) => t.to_string(),
        None => {
            use std::io::{self, BufRead, IsTerminal};
            let stdin = io::stdin();
            if stdin.is_terminal() {
                eprint!("Enter Confluence token for {}: ", server);
            }
            let mut line = String::new();
            stdin.lock().read_line(&mut line)
                .context("failed to read token from stdin")?;
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() {
                bail!("empty token");
            }
            trimmed
        }
    };

    credentials::store_token(server, &token)?;
    let normalized = credentials::normalize_server(server);
    eprintln!("Token stored for {normalized}");
    Ok(())
}

/// Handle `auth gdoc [--service-account <path>]`.
///
/// Without `--service-account`: runs the OAuth2 browser consent flow and
/// stores the refresh token.
///
/// With `--service-account <path>`: reads the JSON key file and stores it
/// in the credential store.
fn auth_gdoc(service_account: Option<&Path>) -> Result<()> {
    if let Some(path) = service_account {
        let key_json = fs::read_to_string(path)
            .with_context(|| format!("failed to read service account key: {}", path.display()))?;

        // Validate that it parses as a service account key.
        let _key: gdoc::auth::ServiceAccountKey = serde_json::from_str(&key_json)
            .with_context(|| format!("invalid service account JSON in {}", path.display()))?;

        credentials::store_gdoc_service_account(&key_json)?;
        eprintln!(
            "Service account key stored from {}",
            path.display()
        );
    } else {
        let cred = gdoc::auth::oauth2_authorization_flow()?;
        let cred_json =
            serde_json::to_string(&cred).context("failed to serialize OAuth credential")?;
        credentials::store_gdoc_oauth(&cred_json)?;
        eprintln!("Google OAuth2 credentials stored successfully");
    }
    Ok(())
}

/// Canonicalize an input path and derive the base directory.
pub(crate) fn resolve_input_and_base(
    input: &Path,
    base: Option<&Path>,
) -> Result<(PathBuf, PathBuf)> {
    let source = input
        .canonicalize()
        .with_context(|| format!("failed to resolve input path {}", input.display()))?;
    let base_dir = match base {
        Some(b) => b
            .canonicalize()
            .with_context(|| format!("failed to resolve base directory {}", b.display()))?,
        None => source
            .parent()
            .ok_or_else(|| anyhow!("input path has no parent directory: {}", source.display()))?
            .to_path_buf(),
    };
    if !base_dir.is_dir() {
        bail!("base path is not a directory: {}", base_dir.display());
    }
    if !source.starts_with(&base_dir) {
        bail!(
            "input file {} is not under the base directory {}",
            source.display(),
            base_dir.display()
        );
    }
    Ok((source, base_dir))
}

/// Parse the flat `--meta KEY VALUE …` vector into `(key, value)` pairs.
///
/// Clap's `num_args = 2` guarantees the count is even, so this is an
/// internal consistency check rather than user-facing validation.
fn parse_meta_pairs(raw: &[String]) -> Result<Vec<(String, String)>> {
    debug_assert!(
        raw.len().is_multiple_of(2),
        "clap guarantees --meta always takes two values"
    );
    Ok(raw
        .chunks(2)
        .map(|c| (c[0].clone(), c[1].clone()))
        .collect())
}

/// Remove keys listed in `--meta-drop` from a metadata map.
///
/// Dot-separated keys navigate into nested objects.  For example,
/// `--meta-drop confluence.path` removes the `path` key from the
/// `confluence` object.  If any intermediate object does not exist,
/// the drop is silently ignored.
pub(crate) fn apply_meta_drops(metadata: &mut JsonMap<String, JsonValue>, drops: &[String]) {
    'outer: for key in drops {
        let parts: Vec<&str> = key.split('.').collect();
        if parts.len() == 1 {
            metadata.remove(parts[0]);
            continue;
        }
        // Navigate to the parent object, then remove the leaf.
        let leaf = parts[parts.len() - 1];
        let parent_parts = &parts[..parts.len() - 1];
        let mut current: &mut JsonValue = match metadata.get_mut(parent_parts[0]) {
            Some(v) => v,
            None => continue,
        };
        for &part in &parent_parts[1..] {
            match current.as_object_mut().and_then(|m| m.get_mut(part)) {
                Some(v) => current = v,
                None => continue 'outer,
            };
        }
        if let Some(obj) = current.as_object_mut() {
            obj.remove(leaf);
        }
    }
}

/// Apply `--meta` overrides to a metadata map.
///
/// Dot-separated keys create or traverse nested objects.  For example,
/// `--meta confluence.server https://x.com` sets
/// `metadata["confluence"]["server"]` to `"https://x.com"`, creating the
/// intermediate `confluence` object if it does not exist.
pub(crate) fn apply_meta_overrides(
    metadata: &mut JsonMap<String, JsonValue>,
    overrides: &[(String, String)],
) -> Result<()> {
    for (key, value) in overrides {
        let parts: Vec<&str> = key.split('.').collect();
        let mut target = &mut *metadata;
        for &part in &parts[..parts.len() - 1] {
            let entry = target
                .entry(part)
                .or_insert_with(|| JsonValue::Object(JsonMap::new()));
            target = entry.as_object_mut().ok_or_else(|| {
                anyhow!(
                    "--meta: '{}' is not an object when setting '{}'",
                    part,
                    key
                )
            })?;
        }
        if let Some(leaf) = parts.last() {
            target.insert((*leaf).to_string(), JsonValue::String(value.clone()));
        }
    }
    Ok(())
}

/// Read a single metadata value from a document's YAML front matter.
///
/// Returns an error if the file has no front matter (or empty front matter).
/// If the key is not found, prints nothing and returns `Ok(())`.
/// Dot-separated keys traverse nested objects (e.g. `confluence.server`).
fn meta_command(input: &Path, key: &str) -> Result<()> {
    use rushdown::ast::KindData;
    use rushdown::text::BasicReader;
    use rushdown_meta::{meta_parser_extension, MetaParserOptions};
    use serde_json::{Map as JsonMap, Value as JsonValue};

    use crate::graph::types::meta_to_json;

    let content = fs::read_to_string(input)
        .with_context(|| format!("failed to read {}", input.display()))?;

    // Check for front matter presence before parsing.
    if graph::build::split_front_matter(&content).0.is_none() {
        bail!("{}: no YAML front matter", input.display());
    }

    // Parse with a minimal rushdown parser (meta extension only).
    let parser = rushdown::parser::Parser::with_extensions(
        rushdown::parser::Options::default(),
        meta_parser_extension(MetaParserOptions::default()),
    );
    let mut reader = BasicReader::new(&content);
    let (arena, doc_ref) = parser.parse(&mut reader);

    let KindData::Document(doc) = arena[doc_ref].kind_data() else {
        bail!("{}: failed to parse document", input.display());
    };

    let metadata: JsonMap<String, JsonValue> = doc
        .metadata()
        .iter()
        .map(|(k, v)| (k.clone(), meta_to_json(v)))
        .collect();

    if metadata.is_empty() {
        bail!("{}: no YAML front matter", input.display());
    }

    // Traverse the dot-separated key path.
    let mut current: &JsonValue = &JsonValue::Object(metadata);
    for part in key.split('.') {
        match current {
            JsonValue::Object(map) => match map.get(part) {
                Some(v) => current = v,
                None => return Ok(()),
            },
            _ => return Ok(()),
        }
    }

    // Print the value.
    match current {
        JsonValue::String(s) => println!("{s}"),
        JsonValue::Null => {}
        JsonValue::Bool(b) => println!("{b}"),
        JsonValue::Number(n) => println!("{n}"),
        _ => println!("{}", serde_json::to_string(current).unwrap()),
    }

    Ok(())
}

/// Resolve the output path from the `-o` flag, the format, and the input
/// file name.  Returns `(output_file, output_dir)` where `output_dir` is
/// the directory that should contain assets (static/ and assets/).
fn resolve_output(output: &Path, input: &Path, format: OutputFormat) -> Result<(PathBuf, PathBuf)> {
    let stem = input
        .file_stem()
        .ok_or_else(|| anyhow!("input path has no file name: {}", input.display()))?
        .to_string_lossy();

    match format {
        OutputFormat::HtmlMulti => {
            // html-multi always produces a directory of pages.
            let dir = output.to_path_buf();
            let file = dir.join("index.html"); // sentinel; actual pages are written individually
            Ok((file, dir))
        }
        OutputFormat::Typst => {
            // typst always produces a self-contained directory with .typ + assets.
            let dir = output.to_path_buf();
            let file = dir.join(format!("{stem}.typ"));
            Ok((file, dir))
        }
        _ => {
            let ext = match format {
                OutputFormat::Html => "html",
                OutputFormat::Pdf => "pdf",
                OutputFormat::Markdown => "md",
                OutputFormat::DocxGdoc => "docx",
                OutputFormat::HtmlMulti | OutputFormat::Typst => unreachable!(),
            };
            // If -o has a recognised extension, treat as a file path.
            if let Some(given_ext) = output.extension() {
                let given = given_ext.to_string_lossy();
                if given == "html" || given == "pdf" || given == "md" || given == "docx" {
                    let dir = output
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .to_path_buf();
                    return Ok((output.to_path_buf(), dir));
                }
            }
            // Otherwise treat -o as a directory; derive filename from input.
            let file = output.join(format!("{stem}.{ext}"));
            Ok((file, output.to_path_buf()))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_command(
    logger: &DebugDumper,
    cache: &Cache,
    theme: &Theme,
    format: OutputFormat,
    input: &Path,
    output: &Path,
    base: Option<&Path>,
    broken_links: BrokenLinkMode,
    meta_drops: &[String],
    meta_overrides: &[(String, String)],
) -> Result<()> {
    let (source, base_dir) = resolve_input_and_base(input, base)?;
    let (output_file, output_dir) = resolve_output(output, &source, format)?;
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

    let build = prepare_document(logger, &base_dir, &source, Some(&doc_name), broken_links)?;
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create output dir {}", output_dir.display()))?;

    // ── Markdown format (no theme needed) ───────────────────────────────
    if matches!(format, OutputFormat::Markdown) {
        let renderer = render_markdown::MarkdownRenderer::new(&base_dir, &output_dir);
        let markdown = renderer.render_document(&build)?;
        fs::write(&output_file, markdown)
            .with_context(|| format!("failed to write {}", output_file.display()))?;
        logger.log(format!("wrote {}", output_file.display()));
        return Ok(());
    }

    // ── DOCX-for-Google-Docs format (no theme needed) ───────────────────
    // Uses the same DocxRenderer as `publish gdoc` — diagrams are rendered
    // to PNG and embedded, images are read from disk and embedded.
    if matches!(format, OutputFormat::DocxGdoc) {
        let diagram_themes = crate::diagram::DiagramThemes {
            plantuml: theme.plantuml_theme()?,
            mermaid: theme.mermaid_theme()?,
            d2: theme.d2_theme()?,
        };
        let renderer =
            render_docx::DocxRenderer::new(cache, &diagram_themes, logger, &base_dir);
        let docx_bytes = renderer.render(&build)?;
        fs::write(&output_file, docx_bytes)
            .with_context(|| format!("failed to write {}", output_file.display()))?;
        logger.log(format!("wrote {}", output_file.display()));
        return Ok(());
    }

    // ── Working directory for asset preparation ────────────────────────
    // For PDF output, use a temporary directory so the final output
    // location contains only the .pdf file.  The temp dir is cleaned up
    // automatically when `_temp_dir` is dropped at the end of this
    // function.  For all other formats the working directory *is* the
    // output directory.
    let (_temp_dir, work_dir) = match format {
        OutputFormat::Pdf => {
            let tmp = tempfile::tempdir()
                .context("failed to create temporary directory for PDF build")?;
            let path = tmp.path().to_path_buf();
            (Some(tmp), path)
        }
        _ => (None, output_dir.clone()),
    };

    // ── Collect and copy assets ─────────────────────────────────────────
    let document_assets = build.asset_targets().clone();
    copy_assets(&base_dir, &work_dir, &document_assets, logger)?;
    theme.copy_assets_to(&work_dir)?;

    // ── Build template context from front matter ────────────────────────
    let mut metadata = build.document_metadata();
    apply_meta_drops(&mut metadata, meta_drops);
    apply_meta_overrides(&mut metadata, meta_overrides)?;
    let style = theme.style_css()?;
    let mut template_context = build_template_context(&metadata, style.as_deref())?;

    // ── Diagram themes ──────────────────────────────────────────────────
    let diagram_themes = crate::diagram::DiagramThemes {
        plantuml: theme.plantuml_theme()?,
        mermaid: theme.mermaid_theme()?,
        d2: theme.d2_theme()?,
    };

    match format {
        OutputFormat::Html => {
            let template = theme.template_for(format)?;
            let toc = build_toc(build.graph());
            let search_json = search_index::build_search_index(&build);
            template_context.insert("search_index", &search_json);
            let html = build_html_document(
                &template,
                &template_context,
                &build,
                &toc,
                cache,
                &diagram_themes,
            )?;
            fs::write(&output_file, html)
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
                .ok_or_else(|| anyhow!("root document has no parent directory"))?;
            let plan = crate::render_html_multi::build_multi_page_plan(&build, root_source_dir);
            let search_json = search_index::build_search_index_multi(&build, &plan);
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
        OutputFormat::Pdf => {
            let template = theme.template_for(format)?;
            let renderer = TypstRenderer::new(
                work_dir.clone(),
                cache.clone(),
                diagram_themes.plantuml.clone(),
                diagram_themes.mermaid.clone(),
                diagram_themes.d2.clone(),
            );
            let body = renderer.render_document(&build, logger)?;
            let typst = render_pdf_template(&template, &template_context, &body)?;
            let typst_name = output_file
                .file_stem()
                .ok_or_else(|| anyhow!("output path has no file stem"))?;
            let typst_path = work_dir.join(typst_name).with_extension("typ");
            let pdf_path = output_file.with_extension("pdf");
            fs::write(&typst_path, &typst)
                .with_context(|| format!("failed to write {}", typst_path.display()))?;
            compile_typst(&typst_path, &pdf_path)?;
            logger.log(format!("wrote {}", pdf_path.display()));
        }
        OutputFormat::Typst => {
            let template = theme.template_for(format)?;
            let renderer = TypstRenderer::new(
                work_dir.clone(),
                cache.clone(),
                diagram_themes.plantuml.clone(),
                diagram_themes.mermaid.clone(),
                diagram_themes.d2.clone(),
            );
            let body = renderer.render_document(&build, logger)?;
            let typst = render_pdf_template(&template, &template_context, &body)?;
            fs::write(&output_file, &typst)
                .with_context(|| format!("failed to write {}", output_file.display()))?;
            logger.log(format!("wrote {}", output_file.display()));
        }
        OutputFormat::Markdown | OutputFormat::DocxGdoc => unreachable!(),
    }

    Ok(())
}

fn check_command(
    logger: &DebugDumper,
    cache: &Cache,
    theme: &Theme,
    input: &Path,
    base: Option<&Path>,
) -> Result<()> {
    let (source, base_dir) = resolve_input_and_base(input, base)?;
    let build = prepare_document(logger, &base_dir, &source, None, BrokenLinkMode::Error)?;
    let graph = build.graph();

    let anchor_count = graph.anchor_count();
    let link_count = graph.link_count();

    // ── Diagram themes ──────────────────────────────────────────────────
    let diagram_themes = crate::diagram::DiagramThemes {
        plantuml: theme.plantuml_theme()?,
        mermaid: theme.mermaid_theme()?,
        d2: theme.d2_theme()?,
    };

    // ── Render every diagram, collecting errors ─────────────────────────
    let mut diagram_count: usize = 0;
    let mut diagram_errors: Vec<String> = Vec::new();

    for (doc_id, node_ref, _) in graph.rendered_nodes_preorder() {
        let doc = graph.document(doc_id);
        if !rushdown::matches_extension_kind!(&doc.arena, node_ref, crate::diagram::Diagram) {
            continue;
        }
        let diagram = rushdown::as_extension_data!(&doc.arena, node_ref, crate::diagram::Diagram);
        let src = crate::diagram::collect_diagram_source(diagram, &doc.source);
        diagram_count += 1;

        let result: Result<()> = match diagram.diagram_type() {
            crate::diagram::DiagramType::Typst => {
                crate::diagram::cached_render_svg(cache, src.as_bytes(), "typst.svg", || {
                    crate::diagram::render_typst_svg(None, &src).map_err(|e| anyhow!("{e}"))
                })
                .map(|_| ())
            }
            crate::diagram::DiagramType::DrawIo => {
                if let Some(path) = diagram.drawio_path() {
                    let cache_key = crate::diagram::drawio_cache_key(path, diagram.drawio_page());
                    crate::diagram::cached_render_bytes(cache, &cache_key, "drawio.pdf", || {
                        crate::diagram::render_drawio(
                            None,
                            path,
                            diagram.drawio_page(),
                            crate::diagram::DrawioFormat::Pdf,
                        )
                        .map_err(|e| anyhow!("{e}"))
                    })
                    .map(|_| ())
                } else {
                    Ok(())
                }
            }
            crate::diagram::DiagramType::D2 => {
                let themed = crate::diagram::apply_d2_theme(&src, diagram_themes.d2.as_deref());
                crate::diagram::cached_render_svg(cache, themed.as_bytes(), "d2.svg", || {
                    crate::diagram::render_d2_svg(None, &themed).map_err(|e| anyhow!("{e}"))
                })
                .map(|_| ())
            }
            crate::diagram::DiagramType::PlantUml => {
                let themed =
                    crate::diagram::apply_plantuml_theme(&src, diagram_themes.plantuml.as_deref());
                crate::diagram::cached_render_bytes(
                    cache,
                    themed.as_bytes(),
                    "plantuml.svg",
                    || {
                        crate::diagram::render_plantuml_svg(None, themed.as_bytes())
                            .map_err(|e| anyhow!("{e}"))
                    },
                )
                .map(|_| ())
            }
            crate::diagram::DiagramType::Mermaid => {
                let themed =
                    crate::diagram::apply_mermaid_theme(&src, diagram_themes.mermaid.as_deref());
                crate::diagram::cached_render_bytes(cache, themed.as_bytes(), "mermaid.svg", || {
                    crate::diagram::render_mermaid_svg_auto(None, &themed).map(|s| s.into_bytes())
                })
                .map(|_| ())
            }
        };

        if let Err(e) = result {
            diagram_errors.push(format!("{}: {e}", doc.path.display()));
        }
    }

    if !diagram_errors.is_empty() {
        for err in &diagram_errors {
            eprintln!("{err}");
        }
        bail!(
            "aborting due to {} diagram error{}",
            diagram_errors.len(),
            if diagram_errors.len() == 1 { "" } else { "s" }
        );
    }

    eprintln!(
        "check passed: {} anchor{}, {} link{}, {} diagram{}",
        anchor_count,
        if anchor_count == 1 { "" } else { "s" },
        link_count,
        if link_count == 1 { "" } else { "s" },
        diagram_count,
        if diagram_count == 1 { "" } else { "s" },
    );
    Ok(())
}

/// Output format names recognised by the format-gating fenced-div logic.
/// Both `html` and `html-multi` output share the `"html"` gate.
pub(crate) const KNOWN_OUTPUT_FORMATS: &[&str] = &["html", "pdf", "markdown", "confluence"];

/// Fenced-div class names recognised as admonitions (callout blocks).
pub(crate) const ADMONITION_TYPES: &[&str] =
    &["note", "warning", "important", "caution", "tip", "related"];

/// Check whether a space-separated class string contains an admonition type.
/// Returns the first matching admonition type, if any.
pub(crate) fn admonition_type(classes: &str) -> Option<&'static str> {
    classes
        .split_whitespace()
        .find_map(|c| ADMONITION_TYPES.iter().find(|&&t| t == c).copied())
}

#[derive(Clone, Debug)]
pub(crate) struct DebugDumper {
    enabled: bool,
}

impl DebugDumper {
    pub(crate) fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub(crate) fn log(&self, message: impl AsRef<str>) {
        if self.enabled {
            eprintln!("{}", message.as_ref());
        }
    }
}

#[cfg(test)]
mod tests {
    use rushdown::ast::{Arena, KindData, NodeRef};
    use rushdown::parser::{self, ParserExtension};
    use rushdown::text::BasicReader;

    use crate::render_html_common::is_inline_image;

    fn parse(source: &str) -> (Arena, NodeRef) {
        let parser = parser::Parser::with_extensions(
            parser::Options {
                attributes: true,
                ..Default::default()
            },
            parser::gfm(parser::GfmOptions::default())
                .and(rushdown_link_attribute::link_attribute_parser_extension()),
        );
        let mut reader = BasicReader::new(source);
        parser.parse(&mut reader)
    }

    /// Find the first `Image` node anywhere in the tree.
    fn find_image(arena: &Arena, root: NodeRef) -> Option<NodeRef> {
        let mut stack = vec![root];
        while let Some(nr) = stack.pop() {
            if matches!(arena[nr].kind_data(), KindData::Image(_)) {
                return Some(nr);
            }
            for child in arena[nr].children(arena) {
                stack.push(child);
            }
        }
        None
    }

    #[test]
    fn sole_image_in_paragraph_is_not_inline() {
        let source = "# Heading\n\n![Cat](cat.svg)\n";
        let (arena, doc_ref) = parse(source);
        let img = find_image(&arena, doc_ref).expect("image node should exist");
        assert!(
            !is_inline_image(&arena, img),
            "sole image in paragraph must not be detected as inline"
        );
    }

    #[test]
    fn image_with_text_is_inline() {
        let source = "# Heading\n\nSee ![Cat](cat.svg) here.\n";
        let (arena, doc_ref) = parse(source);
        let img = find_image(&arena, doc_ref).expect("image node should exist");
        assert!(
            is_inline_image(&arena, img),
            "image mixed with text should be detected as inline"
        );
    }

    #[test]
    fn image_with_existing_class_and_text_is_inline() {
        let source = "# Heading\n\nSee ![Cat](cat.svg){.my-class} here.\n";
        let (arena, doc_ref) = parse(source);
        let img = find_image(&arena, doc_ref).expect("image node should exist");
        assert!(
            is_inline_image(&arena, img),
            "image with user class mixed with text should be detected as inline"
        );
    }
}
