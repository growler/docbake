use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::Result;
use resvg::tiny_skia;
use serde_json::Map as JsonMap;
use tempfile::TempDir;
use zip::ZipArchive;

use crate::cache::Cache;
use crate::cli::BrokenLinkMode;
use crate::conditional::{EffectiveFormat, RenderProfile};
use crate::diagram::DiagramThemes;
use crate::graph::prepare_document;
use crate::theme::Theme;
use crate::DebugDumper;

use super::package::Package;
use super::DocxRenderer;

mod body_lint;

/// PNG signature followed by an IHDR chunk for a 2x3 image, then bytes
/// that are not valid PNG chunk data.
fn png_header_with_corrupt_data() -> Vec<u8> {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.extend_from_slice(&[0, 0, 0, 13]);
    data.extend_from_slice(b"IHDR");
    data.extend_from_slice(&2u32.to_be_bytes());
    data.extend_from_slice(&3u32.to_be_bytes());
    data.extend_from_slice(&[8, 6, 0, 0, 0]);
    data.extend_from_slice(b"corrupt image data");
    data
}

fn with_renderer<T>(
    logo_asset: Option<(Vec<u8>, String)>,
    f: impl FnOnce(&DocxRenderer<'_>) -> T,
) -> T {
    let cache = Cache::new(None, true).unwrap();
    let themes = DiagramThemes::default();
    let logger = DebugDumper::new(false);
    let profile = RenderProfile::from_metadata(EffectiveFormat::Docx, &JsonMap::new()).unwrap();
    let params = Theme::Embedded("blank").docx_params().unwrap();
    let renderer = DocxRenderer::new(
        &cache,
        &themes,
        &logger,
        Path::new("."),
        logo_asset,
        &params,
        profile,
    );
    f(&renderer)
}

#[test]
fn pic_from_png_skips_data_that_is_not_an_image() {
    let pic = with_renderer(None, |r| r.pic_from_png(b"this is not an image".to_vec()));
    assert!(pic.is_none());
}

#[test]
fn pic_from_png_keeps_bytes_when_header_is_valid() {
    let data = png_header_with_corrupt_data();
    let pic = with_renderer(None, |r| r.pic_from_png(data.clone())).unwrap();
    assert_eq!(pic.image, data);
    assert_eq!(pic.size, (2 * 9525, 3 * 9525));
}

#[test]
fn pic_from_png_keeps_encoded_png_and_caps_width() {
    let pixmap = tiny_skia::Pixmap::new(1200, 300).unwrap();
    let data = pixmap.encode_png().unwrap();
    let pic = with_renderer(None, |r| r.pic_from_png(data.clone())).unwrap();
    assert_eq!(pic.image, data);
    assert_eq!(pic.size, (5_760_000, 1_440_000));
}

#[test]
fn front_logo_skips_png_file_that_is_not_an_image() {
    let logo = (b"this is not an image".to_vec(), "png".to_string());
    let pic = with_renderer(Some(logo), |r| r.resolve_front_logo());
    assert!(pic.is_none());
}

/// Renders `markdown` with the blank theme.
fn render_package(markdown: &str) -> Result<Package> {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("index.md");
    fs::write(&source, markdown).unwrap();
    let prepared = prepare_document(
        &DebugDumper::new(false),
        temp.path(),
        &source,
        Some("index"),
        BrokenLinkMode::Warn,
    )
    .unwrap();
    let metadata = prepared.document_metadata();
    with_renderer(None, |r| r.render(&prepared, &metadata))
}

/// Renders `markdown` with the blank theme and gives the parts of the
/// DOCX file by name.
fn render_parts(markdown: &str) -> Result<impl Fn(&str) -> String> {
    let bytes = render_package(markdown)?.into_bytes()?;
    Ok(move |name: &str| {
        let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut text = String::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    })
}

const SAMPLE: &str = "\
---
title: Sample
lang: ru-RU
---
# One

## Two

### Three

#### Four
";

#[test]
fn page_size_and_margins_come_from_theme() {
    let part = render_parts(SAMPLE).unwrap();
    let document = part("word/document.xml");
    assert!(
        document.contains(r#"<w:pgSz w:w="11906" w:h="16838" />"#),
        "{document}"
    );
    assert!(
        document.contains(
            r#"<w:pgMar w:top="1134" w:right="567" w:bottom="1134" w:left="1134" w:header="851" w:footer="992" w:gutter="0" />"#
        ),
        "{document}"
    );
}

#[test]
fn tab_stops_use_text_width() {
    let part = render_parts(SAMPLE).unwrap();
    let document = part("word/document.xml");
    assert_eq!(document.matches(r#"w:pos="10205""#).count(), 3);
    assert!(!document.contains("80000"));
    for name in ["word/header1.xml", "word/footer1.xml"] {
        let xml = part(name);
        assert!(
            xml.contains(r#"<w:tab w:val="center" w:pos="5102" />"#),
            "{xml}"
        );
        assert!(
            xml.contains(r#"<w:tab w:val="right" w:pos="10205" />"#),
            "{xml}"
        );
    }
}

#[test]
fn document_language_comes_from_front_matter() {
    let part = render_parts(SAMPLE).unwrap();
    assert!(part("word/styles.xml").contains(r#"w:val="ru-RU""#));

    let part = render_parts("# One\n").unwrap();
    let styles = part("word/styles.xml");
    assert!(styles.contains(r#"<w:lang w:val="en""#), "{styles}");
}

#[test]
fn invalid_language_is_an_error() {
    let err = render_parts("---\nlang: ru_RU\n---\n# One\n")
        .err()
        .unwrap()
        .to_string();
    assert_eq!(err, "lang: invalid language tag 'ru_RU'");
}
