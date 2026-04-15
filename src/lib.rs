pub(crate) mod bracketed_span;
pub(crate) mod build;
pub(crate) mod cache;
pub(crate) mod caption;
pub mod cli;
pub(crate) mod code_block_attribute;
pub(crate) mod diagram;
pub(crate) mod graph;
mod render_html;
mod render_html_common;
mod render_html_multi;
mod render_markdown;
mod render_typst;
pub(crate) mod theme;

use std::fs;
use std::path::{Path, PathBuf};

use crate::build::{build_template_context, compile_typst, copy_assets, render_pdf_template};
use crate::cache::Cache;
use crate::cli::{Cli, OutputFormat};
use crate::graph::prepare_document;
use crate::render_html::{build_html_document, build_toc};
use crate::render_html_multi::build_multi_page_html;
use crate::render_typst::TypstRenderer;
use crate::theme::Theme;
use anyhow::{anyhow, bail, Context, Result};

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
        } => build_command(
            &logger,
            &cache,
            &theme,
            format,
            input,
            output,
            base.as_deref(),
        ),
        cli::Command::Dump { ref path, ref base } => {
            let (source, base_dir) = resolve_input_and_base(path, base.as_deref())?;
            let build = prepare_document(&logger, &base_dir, &source, None)?;
            print!("{}", build.dump_graph());
            Ok(())
        }
        cli::Command::Parse { ref path, ref base } => {
            let (source, base_dir) = resolve_input_and_base(path, base.as_deref())?;
            let build = prepare_document(&logger, &base_dir, &source, None)?;
            print!("{}", build.parse_dump()?);
            Ok(())
        }
        cli::Command::InitTheme { ref target } => {
            Theme::export_to(target)?;
            eprintln!("Default theme exported to {}", target.display());
            Ok(())
        }
    }
}

/// Canonicalize an input path and derive the base directory.
fn resolve_input_and_base(input: &Path, base: Option<&Path>) -> Result<(PathBuf, PathBuf)> {
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
        _ => {
            let ext = match format {
                OutputFormat::Html => "html",
                OutputFormat::Pdf => "pdf",
                OutputFormat::Markdown => "md",
                OutputFormat::HtmlMulti => unreachable!(),
            };
            // If -o has a recognised extension, treat as a file path.
            if let Some(given_ext) = output.extension() {
                let given = given_ext.to_string_lossy();
                if given == "html" || given == "pdf" || given == "md" {
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

fn build_command(
    logger: &DebugDumper,
    cache: &Cache,
    theme: &Theme,
    format: OutputFormat,
    input: &Path,
    output: &Path,
    base: Option<&Path>,
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

    let build = prepare_document(logger, &base_dir, &source, Some(&doc_name))?;
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

    // ── Collect and copy assets ─────────────────────────────────────────
    let document_assets = build.asset_targets().clone();
    copy_assets(&base_dir, &output_dir, &document_assets, logger)?;
    theme.copy_assets_to(&output_dir)?;

    // ── Build template context from front matter ────────────────────────
    let metadata = build.document_metadata();
    let style = theme.style_css()?;
    let template_context = build_template_context(&metadata, style.as_deref())?;

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
                output_dir.clone(),
                cache.clone(),
                diagram_themes.plantuml.clone(),
                diagram_themes.mermaid.clone(),
                diagram_themes.d2.clone(),
            );
            let body = renderer.render_document(&build, logger)?;
            let typst = render_pdf_template(&template, &template_context, &body)?;
            let typst_path = output_file.with_extension("typ");
            let pdf_path = output_file.with_extension("pdf");
            fs::write(&typst_path, typst)
                .with_context(|| format!("failed to write {}", typst_path.display()))?;
            logger.log(format!("wrote {}", typst_path.display()));
            compile_typst(&typst_path, &pdf_path)?;
            logger.log(format!("wrote {}", pdf_path.display()));
        }
        OutputFormat::Markdown => unreachable!(),
    }

    Ok(())
}

/// Output format names recognised by the format-gating fenced-div logic.
/// Both `html` and `html-multi` output share the `"html"` gate.
pub(crate) const KNOWN_OUTPUT_FORMATS: &[&str] = &["html", "pdf", "markdown"];

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
