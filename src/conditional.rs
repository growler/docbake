use std::collections::BTreeSet;

use anyhow::{bail, Result};
use rushdown::ast::{Arena, NodeRef};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::cli::{CheckFormat, OutputFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum EffectiveFormat {
    Html,
    Pdf,
    Docx,
    Confluence,
    Markdown,
}

impl EffectiveFormat {
    pub(crate) fn from_output_format(format: OutputFormat) -> Self {
        match format {
            OutputFormat::Html | OutputFormat::HtmlMulti => Self::Html,
            OutputFormat::Pdf | OutputFormat::Typst => Self::Pdf,
            OutputFormat::Docx => Self::Docx,
            OutputFormat::Markdown => Self::Markdown,
        }
    }

    pub(crate) fn from_check_format(format: CheckFormat) -> Self {
        match format {
            CheckFormat::Html => Self::Html,
            CheckFormat::Pdf => Self::Pdf,
            CheckFormat::Docx => Self::Docx,
            CheckFormat::Confluence => Self::Confluence,
            CheckFormat::Markdown => Self::Markdown,
        }
    }

    pub(crate) fn parse_token(token: &str) -> Option<Self> {
        match token {
            "html" => Some(Self::Html),
            "pdf" => Some(Self::Pdf),
            "docx" => Some(Self::Docx),
            "confluence" => Some(Self::Confluence),
            "markdown" => Some(Self::Markdown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RenderProfile {
    pub(crate) format: EffectiveFormat,
    pub(crate) variants: BTreeSet<String>,
}

impl RenderProfile {
    pub(crate) fn from_metadata(
        format: EffectiveFormat,
        metadata: &JsonMap<String, JsonValue>,
    ) -> Result<Self> {
        let variants = normalize_variants(metadata)?;
        Ok(Self { format, variants })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FenceConditions {
    pub(crate) formats: BTreeSet<EffectiveFormat>,
    pub(crate) variants: BTreeSet<String>,
    has_formats: bool,
    has_variants: bool,
}

impl FenceConditions {
    fn matches(&self, profile: &RenderProfile) -> bool {
        if self.has_formats && !self.formats.contains(&profile.format) {
            return false;
        }
        if self.has_variants
            && self
                .variants
                .iter()
                .all(|variant| !profile.variants.contains(variant))
        {
            return false;
        }
        true
    }

    fn is_unconditional(&self) -> bool {
        !self.has_formats && !self.has_variants
    }
}

pub(crate) fn normalize_variants(
    metadata: &JsonMap<String, JsonValue>,
) -> Result<BTreeSet<String>> {
    let Some(value) = metadata.get("variants") else {
        return Ok(BTreeSet::new());
    };

    let JsonValue::Array(items) = value else {
        bail!("metadata key `variants` must be an array of strings");
    };

    let mut variants = BTreeSet::new();
    for item in items {
        let JsonValue::String(value) = item else {
            bail!("metadata key `variants` must contain only string values");
        };
        variants.insert(value.clone());
    }
    Ok(variants)
}

pub(crate) fn parse_fence_conditions(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
) -> Result<Option<FenceConditions>> {
    if !rushdown::matches_extension_kind!(arena, node_ref, rushdown_fenced_div::FencedDiv) {
        return Ok(None);
    }

    let mut formats = BTreeSet::new();
    let mut variants = BTreeSet::new();
    let mut has_formats = false;
    let mut has_variants = false;

    if let Some(value) = arena[node_ref].attributes().get("if-format") {
        has_formats = true;
        for token in value.str(source).split_whitespace() {
            let format = EffectiveFormat::parse_token(token).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown format `{token}` in if-format; expected one of: html pdf docx confluence markdown"
                )
            })?;
            formats.insert(format);
        }
        if formats.is_empty() {
            bail!("empty if-format attribute");
        }
    }

    if let Some(value) = arena[node_ref].attributes().get("if-variant") {
        has_variants = true;
        for token in value.str(source).split_whitespace() {
            variants.insert(token.to_string());
        }
        if variants.is_empty() {
            bail!("empty if-variant attribute");
        }
    }

    let conditions = FenceConditions {
        formats,
        variants,
        has_formats,
        has_variants,
    };

    if conditions.is_unconditional() {
        Ok(None)
    } else {
        Ok(Some(conditions))
    }
}

pub(crate) fn fenced_div_is_visible(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
    profile: &RenderProfile,
) -> Result<bool> {
    let Some(conditions) = parse_fence_conditions(arena, node_ref, source)? else {
        return Ok(true);
    };
    Ok(conditions.matches(profile))
}

pub(crate) fn node_is_visible(
    arena: &Arena,
    node_ref: NodeRef,
    source: &str,
    profile: &RenderProfile,
) -> Result<bool> {
    if rushdown::matches_extension_kind!(arena, node_ref, rushdown_fenced_div::FencedDiv) {
        return fenced_div_is_visible(arena, node_ref, source, profile);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use rushdown::ast::{Arena, NodeRef, WalkStatus};
    use serde_json::{Map as JsonMap, Value as JsonValue};

    use super::{normalize_variants, parse_fence_conditions, EffectiveFormat, RenderProfile};

    fn parse(source: &str) -> (Arena, NodeRef) {
        let parser = crate::build::markdown_parser();
        parser.parse(source, rushdown::parser::ParseOptions::default())
    }

    fn first_fenced_div(arena: &Arena, root: NodeRef) -> NodeRef {
        let mut found = None;
        let _ = rushdown::ast::walk(arena, root, &mut |arena: &Arena,
                                                       node_ref: NodeRef,
                                                       entering: bool|
         -> std::result::Result<
            WalkStatus,
            std::io::Error,
        > {
            if entering
                && rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    rushdown_fenced_div::FencedDiv
                )
            {
                found = Some(node_ref);
                return Ok(WalkStatus::Stop);
            }
            Ok(WalkStatus::Continue)
        });
        found.expect("expected fenced div")
    }

    fn profile(format: EffectiveFormat, variants: &[&str]) -> RenderProfile {
        RenderProfile {
            format,
            variants: variants.iter().map(|v| (*v).to_string()).collect(),
        }
    }

    #[test]
    fn if_format_uses_or_semantics() {
        let source = ":::{if-format=\"html confluence\"}\nvisible\n:::";
        let (arena, root) = parse(source);
        let div = first_fenced_div(&arena, root);
        let conditions = parse_fence_conditions(&arena, div, source)
            .unwrap()
            .expect("conditions must be parsed");

        assert!(conditions.matches(&profile(EffectiveFormat::Html, &[])));
        assert!(conditions.matches(&profile(EffectiveFormat::Confluence, &[])));
        assert!(!conditions.matches(&profile(EffectiveFormat::Pdf, &[])));
    }

    #[test]
    fn if_format_and_if_variant_use_and_semantics() {
        let source = ":::{if-format=\"html\" if-variant=\"mts\"}\nvisible\n:::";
        let (arena, root) = parse(source);
        let div = first_fenced_div(&arena, root);
        let conditions = parse_fence_conditions(&arena, div, source)
            .unwrap()
            .expect("conditions must be parsed");

        assert!(conditions.matches(&profile(EffectiveFormat::Html, &["mts"])));
        assert!(!conditions.matches(&profile(EffectiveFormat::Html, &["other"])));
        assert!(!conditions.matches(&profile(EffectiveFormat::Pdf, &["mts"])));
    }

    #[test]
    fn parse_rejects_unknown_and_empty_if_format() {
        let source = ":::{if-format=\"html unknown\"}\ntext\n:::";
        let (arena, root) = parse(source);
        let div = first_fenced_div(&arena, root);
        let error = parse_fence_conditions(&arena, div, source).unwrap_err();
        assert!(error.to_string().contains("unknown format"));

        let source = ":::{if-format=\"   \"}\ntext\n:::";
        let (arena, root) = parse(source);
        let div = first_fenced_div(&arena, root);
        let error = parse_fence_conditions(&arena, div, source).unwrap_err();
        assert!(error.to_string().contains("empty if-format"));
    }

    #[test]
    fn normalize_variants_requires_string_array() {
        let mut metadata = JsonMap::new();
        metadata.insert(
            "variants".to_string(),
            JsonValue::Array(vec![
                JsonValue::String("mts".to_string()),
                JsonValue::Bool(true),
            ]),
        );

        let error = normalize_variants(&metadata).unwrap_err();
        assert!(error.to_string().contains("only string values"));
    }

    #[test]
    fn normalize_variants_deduplicates_values() {
        let mut metadata = JsonMap::new();
        metadata.insert(
            "variants".to_string(),
            JsonValue::Array(vec![
                JsonValue::String("mts".to_string()),
                JsonValue::String("mts".to_string()),
                JsonValue::String("foo".to_string()),
            ]),
        );

        let variants = normalize_variants(&metadata).unwrap();
        assert_eq!(variants.len(), 2);
        assert!(variants.contains("mts"));
        assert!(variants.contains("foo"));
    }

    #[test]
    fn docx_output_format_maps_to_docx_effective_format() {
        assert_eq!(
            EffectiveFormat::from_output_format(crate::cli::OutputFormat::Docx),
            EffectiveFormat::Docx
        );
    }
}
