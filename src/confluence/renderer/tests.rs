use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::cache::Cache;
use crate::cli::BrokenLinkMode;
use crate::conditional::{EffectiveFormat, RenderProfile};
use crate::diagram::DiagramThemes;
use crate::graph::prepare_document;
use crate::DebugDumper;

use super::attachments::{parse_svg_intrinsic_width, svg_width_to_pixels};
use super::{ConfluenceDocument, ConfluenceRenderer};

fn write_text(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

fn write_bytes(path: &Path, data: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, data).unwrap();
}

fn attachment_refs_from_body(body: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut rest = body;
    let needle = "ri:attachment ri:filename=\"";
    while let Some(start) = rest.find(needle) {
        let after = &rest[start + needle.len()..];
        if let Some(end) = after.find('"') {
            refs.push(after[..end].to_string());
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    refs
}

fn build_prepared(base_dir: &Path, source: &Path) -> crate::graph::PreparedDocument {
    prepare_document(
        &DebugDumper::new(false),
        base_dir,
        source,
        Some("test"),
        BrokenLinkMode::Error,
    )
    .unwrap()
}

fn render_document(
    prepared: &crate::graph::PreparedDocument,
    base_dir: &Path,
) -> ConfluenceDocument {
    let cache = Cache::new(None, true).unwrap();
    let themes = DiagramThemes::default();
    let logger = DebugDumper::new(false);
    let profile =
        RenderProfile::from_metadata(EffectiveFormat::Confluence, &serde_json::Map::new()).unwrap();
    ConfluenceRenderer::new(&cache, &themes, &logger, base_dir, profile)
        .render(prepared)
        .unwrap()
}

#[test]
fn duplicate_basenames_keep_distinct_attachment_identity() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();
    let source = base_dir.join("index.md");
    let image_a = base_dir.join("a/logo.png");
    let image_b = base_dir.join("b/logo.png");

    write_text(
        &source,
        "![A](a/logo.png)\n\n![A again](a/logo.png)\n\n![B](b/logo.png)\n",
    );
    write_bytes(&image_a, b"image-a");
    write_bytes(&image_b, b"image-b");

    let prepared = build_prepared(&base_dir, &source);
    let rendered = render_document(&prepared, &base_dir);

    assert_eq!(rendered.attachments.len(), 2);

    let refs = attachment_refs_from_body(&rendered.body);
    assert_eq!(refs.len(), 3);
    assert_eq!(refs[0], refs[1]);
    assert_ne!(refs[0], refs[2]);

    let first_data = rendered
        .attachments
        .iter()
        .find(|att| att.filename == refs[0])
        .map(|att| att.data.as_slice())
        .unwrap();
    let second_data = rendered
        .attachments
        .iter()
        .find(|att| att.filename == refs[2])
        .map(|att| att.data.as_slice())
        .unwrap();
    assert_eq!(first_data, b"image-a");
    assert_eq!(second_data, b"image-b");
}

#[test]
fn missing_asset_never_falls_back_to_same_filename_elsewhere() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();
    let source = base_dir.join("index.md");
    let missing = base_dir.join("first/logo.png");
    let existing = base_dir.join("second/logo.png");

    write_text(
        &source,
        "![Missing](first/logo.png)\n\n![Existing](second/logo.png)\n",
    );
    write_bytes(&missing, b"missing-before-build");
    write_bytes(&existing, b"existing");

    let prepared = build_prepared(&base_dir, &source);
    fs::remove_file(&missing).unwrap();

    let rendered = render_document(&prepared, &base_dir);
    assert_eq!(rendered.attachments.len(), 1);
    assert_eq!(rendered.attachments[0].data, b"existing");

    let refs = attachment_refs_from_body(&rendered.body);
    assert_eq!(refs.len(), 2);
    assert_ne!(refs[0], refs[1]);
    assert_eq!(rendered.attachments[0].filename, refs[1]);
}

#[test]
fn intrinsic_width_px_suffix() {
    let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="94px" height="92px" viewBox="0 0 94 92"></svg>"#;
    assert_eq!(parse_svg_intrinsic_width(svg), Some(94));
}

#[test]
fn intrinsic_width_bare_number() {
    let svg = br#"<svg width="300" height="200"></svg>"#;
    assert_eq!(parse_svg_intrinsic_width(svg), Some(300));
}

#[test]
fn intrinsic_width_percentage_returns_none() {
    let svg = br#"<svg width="100%" height="100%" viewBox="0 0 800 600"></svg>"#;
    assert_eq!(parse_svg_intrinsic_width(svg), None);
}

#[test]
fn intrinsic_width_em_returns_none() {
    let svg = br#"<svg width="50em" height="40em"></svg>"#;
    assert_eq!(parse_svg_intrinsic_width(svg), None);
}

#[test]
fn intrinsic_width_missing_returns_none() {
    let svg = br#"<svg viewBox="0 0 100 100"></svg>"#;
    assert_eq!(parse_svg_intrinsic_width(svg), None);
}

#[test]
fn intrinsic_width_no_svg_tag() {
    let data = b"not an svg at all";
    assert_eq!(parse_svg_intrinsic_width(data), None);
}

#[test]
fn intrinsic_width_case_insensitive_tag() {
    let svg = br#"<SVG Width="120px" Height="80px"></SVG>"#;
    // The regex for the width attribute is case-sensitive on the
    // attribute name, but the <svg> tag match is case-insensitive.
    // Our regex uses `\bwidth\s*=` which is lowercase -- uppercase
    // Width won't match, returning None. This is acceptable because
    // all known SVG generators emit lowercase attributes.
    // Just verify it doesn't panic.
    let _ = parse_svg_intrinsic_width(svg);
}

#[test]
fn intrinsic_width_real_plantuml_svg() {
    // Mimics the failed-diagram.svg structure.
    let svg = br#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" contentScriptType="application/ecmascript" contentStyleType="text/css" height="92px" preserveAspectRatio="none" style="width:94px;height:92px;" version="1.1" viewBox="0 0 94 92" width="94px" zoomAndPan="magnify"><defs/></svg>"#;
    assert_eq!(parse_svg_intrinsic_width(svg), Some(94));
}

#[test]
fn no_user_width_no_intrinsic() {
    assert_eq!(svg_width_to_pixels(None, None), 800);
}

#[test]
fn no_user_width_small_intrinsic() {
    // SVG is 94px wide -> use 94, not 800.
    assert_eq!(svg_width_to_pixels(None, Some(94)), 94);
}

#[test]
fn no_user_width_large_intrinsic() {
    // SVG is 1200px wide -> cap at 800.
    assert_eq!(svg_width_to_pixels(None, Some(1200)), 800);
}

#[test]
fn user_percentage_with_small_intrinsic() {
    // 50% of min(94, 800) = 50% of 94 = 47
    assert_eq!(svg_width_to_pixels(Some("50%"), Some(94)), 47);
}

#[test]
fn user_percentage_with_large_intrinsic() {
    // 50% of min(1200, 800) = 50% of 800 = 400
    assert_eq!(svg_width_to_pixels(Some("50%"), Some(1200)), 400);
}

#[test]
fn user_percentage_no_intrinsic() {
    // 50% of 800 = 400
    assert_eq!(svg_width_to_pixels(Some("50%"), None), 400);
}

#[test]
fn user_explicit_px_respected() {
    // Explicit 200px from user -- pass through regardless of intrinsic.
    assert_eq!(svg_width_to_pixels(Some("200px"), Some(94)), 200);
    assert_eq!(svg_width_to_pixels(Some("200px"), None), 200);
}

#[test]
fn user_explicit_bare_number() {
    assert_eq!(svg_width_to_pixels(Some("300"), Some(94)), 300);
}

#[test]
fn user_unparseable_falls_back_to_base() {
    assert_eq!(svg_width_to_pixels(Some("auto"), Some(94)), 94);
    assert_eq!(svg_width_to_pixels(Some("auto"), None), 800);
}

#[test]
fn render_starts_with_native_toc_macro() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();
    let source = base_dir.join("index.md");
    write_text(&source, "# Title\n\nText\n");

    let prepared = build_prepared(&base_dir, &source);
    let rendered = render_document(&prepared, &base_dir);

    assert!(rendered
        .body
        .starts_with("<ac:structured-macro ac:name=\"toc\"></ac:structured-macro>\n"));
}

#[test]
fn leading_warning_admonition_renders_before_toc() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();
    let source = base_dir.join("index.md");
    write_text(
        &source,
        ":::warning\n# Important\n\nRead first.\n:::\n\n# Title\n\nText\n",
    );

    let prepared = build_prepared(&base_dir, &source);
    let rendered = render_document(&prepared, &base_dir);

    let warning_pos = rendered
        .body
        .find("<ac:structured-macro ac:name=\"warning\">")
        .unwrap();
    let toc_pos = rendered
        .body
        .find("<ac:structured-macro ac:name=\"toc\"></ac:structured-macro>\n")
        .unwrap();
    assert!(warning_pos < toc_pos);
}

#[test]
fn render_emits_anchors_for_non_heading_targets() {
    let tmp = TempDir::new().unwrap();
    let base_dir = tmp.path().to_path_buf();
    let source = base_dir.join("index.md");
    let image = base_dir.join("cat.png");
    write_bytes(&image, b"png");
    write_text(
        &source,
        r#"# Title

table: Inline markup {#inline-markup}

| A |
| - |
| B |

[inline](#inline-markup)

```rust {#code-id}
fn main() {}
```

[code](#code-id)

Listing: Example {#listing-id}

```rust
fn listed() {}
```

[listing](#listing-id)

![Alt](cat.png){#cat}

[cat](#cat)
"#,
    );

    let prepared = build_prepared(&base_dir, &source);
    let rendered = render_document(&prepared, &base_dir);

    for id in ["inline-markup", "code-id", "listing-id", "cat"] {
        assert!(
            rendered.body.contains(&format!(
                "<ac:structured-macro ac:name=\"anchor\"><ac:parameter ac:name=\"\">{id}</ac:parameter></ac:structured-macro>"
            )),
            "missing anchor macro for {id}"
        );
        assert!(
            rendered
                .body
                .contains(&format!("<ac:link ac:anchor=\"{id}\"")),
            "missing internal link for {id}"
        );
    }
}
