use std::path::Path;

use resvg::tiny_skia;
use serde_json::Map as JsonMap;

use crate::cache::Cache;
use crate::conditional::{EffectiveFormat, RenderProfile};
use crate::diagram::DiagramThemes;
use crate::DebugDumper;

use super::DocxRenderer;

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
    let renderer = DocxRenderer::new(
        &cache,
        &themes,
        &logger,
        Path::new("."),
        logo_asset,
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
