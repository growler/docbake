use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::code_block_attribute::code_block_attribute_parser_extension;
use crate::diagram::{diagram_parser_extension, DiagramParserOptions};
use anyhow::{bail, Context, Result};
use rushdown::parser::{self, ParserExtension};
use rushdown_definition_list::definition_list_parser_extension;
use rushdown_footnote::footnote_parser_extension;
use rushdown_meta::meta_parser_extension;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::graph::resolve::static_asset_path;
use crate::DebugDumper;

pub(crate) fn markdown_parser() -> parser::Parser {
    parser::Parser::with_extensions(
        parser::Options {
            attributes: true,
            auto_heading_ids: false,
            ..Default::default()
        },
        parser::gfm(parser::GfmOptions::default())
            .and(meta_parser_extension())
            .and(definition_list_parser_extension())
            .and(footnote_parser_extension())
            .and(rushdown_link_attribute::link_attribute_parser_extension())
            .and(crate::bracketed_span::bracketed_span_parser_extension())
            .and(rushdown_fenced_div::fenced_div_parser_extension())
            .and(code_block_attribute_parser_extension())
            .and(crate::grid_table::grid_table_parser_extension())
            .and(crate::multiline_table::multiline_table_parser_extension())
            .and(crate::table_cell_breaks::table_cell_break_transformer_extension())
            .and(diagram_parser_extension(DiagramParserOptions::default())),
        // NOTE: caption_parser_extension is intentionally omitted here.
        // The caption AST transformer is run as a post-parse step in
        // graph/build.rs after the image-to-diagram transform, so that
        // Figure: captions can match both code-block diagrams and
        // file-based diagram images (.puml, .mmd).
    )
}

pub(crate) fn compile_typst(input: &Path, output: &Path) -> Result<()> {
    let status = Command::new("typst")
        .arg("compile")
        .arg(input)
        .arg(output)
        .status()
        .with_context(|| "failed to start `typst compile`")?;
    if !status.success() {
        bail!("`typst compile` failed with status {status}");
    }
    Ok(())
}

pub(crate) fn copy_assets(
    base_dir: &Path,
    output_root: &Path,
    assets: &HashSet<PathBuf>,
    logger: &DebugDumper,
) -> Result<()> {
    for asset in assets {
        let rel = static_asset_path(base_dir, asset)?;
        let target = output_root.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::copy(asset, &target).with_context(|| {
            format!("failed to copy {} to {}", asset.display(), target.display())
        })?;
        logger.log(format!(
            "copied {} -> {}",
            asset.display(),
            target.display()
        ));
    }
    Ok(())
}

/// Build a Tera template context from document front-matter metadata and
/// optional CSS content.
pub(crate) fn build_template_context(
    metadata: &JsonMap<String, JsonValue>,
    style: Option<&str>,
) -> Result<tera::Context> {
    let mut data = metadata.clone();

    // Normalize author/authors: if `author` is a string, also provide
    // `authors` as a single-element array, and vice versa.
    if let Some(author) = data.get("author").cloned() {
        match &author {
            JsonValue::String(name) => {
                data.insert(
                    "authors".to_string(),
                    JsonValue::Array(vec![JsonValue::String(name.clone())]),
                );
            }
            JsonValue::Array(values) => {
                let joined = values
                    .iter()
                    .filter_map(|value| value.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                data.insert("author".to_string(), JsonValue::String(joined));
            }
            _ => {}
        }
    }
    data.insert(
        "front_matter".to_string(),
        JsonValue::Object(metadata.clone()),
    );

    let mut context = tera::Context::from_serialize(&JsonValue::Object(data))
        .context("failed to build template context")?;
    if context.get("lang").is_none() {
        context.insert("lang", "en");
    }
    // Inject CSS content.
    if let Some(css) = style {
        context.insert("style", css);
    }
    // asset_prefix defaults to empty; multi-page renderer overrides per page.
    context.insert("asset_prefix", "");
    // live_update defaults to empty; the serve command injects a JS snippet.
    context.insert("live_update", "");
    Ok(context)
}

/// Render a Typst/PDF template with Tera.
///
/// `template` is the Tera template source string (not a file path).
pub(crate) fn render_pdf_template(
    template: &str,
    context: &tera::Context,
    body: &str,
) -> Result<String> {
    let mut context = context.clone();
    context.insert("body", body);
    if let Some(lang) = context.get("lang").and_then(|value| value.as_str()) {
        let (lang_code, region) = split_lang_tag(lang);
        context.insert("lang", &lang_code);
        if let Some(region) = region {
            context.insert("region", &region);
        }
    }
    tera::Tera::one_off(template, &context, false).context("failed to render PDF template")
}

fn split_lang_tag(value: &str) -> (String, Option<String>) {
    match value.split_once('-') {
        Some((lang, region)) => (lang.to_string(), Some(region.to_string())),
        None => (value.to_string(), None),
    }
}
