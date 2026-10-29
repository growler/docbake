use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use docx_rs::Pic;
use regex::Regex;
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

use super::media::logo_at_height;
use super::package::Package;
use super::params::Twips;
use super::DocxRenderer;

mod body_lint;
mod captions;
mod lists;
mod tables;

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
fn theme_logo_skips_png_file_that_is_not_an_image() {
    let logo = (b"this is not an image".to_vec(), "png".to_string());
    let pic = with_renderer(Some(logo), |r| r.theme_logo());
    assert!(pic.is_none());
}

#[test]
fn theme_logo_skips_png_with_zero_dimension() {
    for at in [16, 20] {
        let mut data = png_header_with_corrupt_data();
        data[at..at + 4].copy_from_slice(&0u32.to_be_bytes());
        let pic = with_renderer(Some((data, "png".to_string())), |r| r.theme_logo());
        assert!(pic.is_none(), "{:?}", pic.map(|p| p.size));
    }
}

#[test]
fn theme_logo_rasterises_svg() {
    let logo = Theme::Embedded("blank").logo_asset().unwrap();
    let pic = with_renderer(logo, |r| r.theme_logo()).unwrap();
    assert_eq!(pic.size, (532 * 9525, 103 * 9525));
}

#[test]
fn logo_at_height_keeps_aspect_ratio() {
    let pic = Pic::new_with_dimensions(b"png".to_vec(), 400, 100);
    let sized = logo_at_height(&pic, Twips(100));
    assert_eq!(sized.size, (254_000, 63_500));
    assert_eq!(sized.image, pic.image);

    let pic = Pic::new_with_dimensions(Vec::new(), 532, 103);
    assert_eq!(logo_at_height(&pic, Twips(210)).size, (688_759, 133_350));
}

/// The blank logo, rasterised to PNG one time for all renders.
static BLANK_LOGO_PNG: LazyLock<(Vec<u8>, String)> = LazyLock::new(|| {
    let (svg, _) = Theme::Embedded("blank").logo_asset().unwrap().unwrap();
    let png = with_renderer(None, |r| r.svg_to_png(&svg)).unwrap();
    (png, "png".to_string())
});

/// Renders `markdown` with the blank theme and its logo.
fn render_package(markdown: &str) -> Result<Package> {
    render_package_with_logo(markdown, Some(BLANK_LOGO_PNG.clone()))
}

/// Renders `markdown` with the blank theme and the logo `logo_asset`.
fn render_package_with_logo(
    markdown: &str,
    logo_asset: Option<(Vec<u8>, String)>,
) -> Result<Package> {
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
    with_renderer(logo_asset, |r| r.render(&prepared, &metadata))
}

/// Renders `markdown` with the blank theme and its logo, and gives the
/// parts of the DOCX file by name.
fn render_parts(markdown: &str) -> Result<impl Fn(&str) -> String> {
    Ok(parts(render_package(markdown)?.into_bytes()?))
}

/// Gives the parts of the DOCX file `bytes` by name.
fn parts(bytes: Vec<u8>) -> impl Fn(&str) -> String {
    move |name: &str| {
        let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut text = String::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }
}

/// Gives the names of the entries of the DOCX file `bytes`.
fn entry_names(bytes: &[u8]) -> Vec<String> {
    let archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    archive.file_names().map(str::to_string).collect()
}

/// Gives the text of `xml` from the first occurrence of `start` to the
/// first `end` after it, both included.
fn between<'x>(xml: &'x str, start: &str, end: &str) -> &'x str {
    let from = xml
        .find(start)
        .unwrap_or_else(|| panic!("no {start} in {xml}"));
    let to = from + xml[from..].find(end).unwrap() + end.len();
    &xml[from..to]
}

/// Asserts that `xml` contains each item of `parts` in this order.
#[track_caller]
fn assert_in_order(xml: &str, parts: &[&str]) {
    let mut rest = xml;
    for part in parts {
        let at = rest
            .find(part)
            .unwrap_or_else(|| panic!("no {part} in order in {xml}"));
        rest = &rest[at + part.len()..];
    }
}

/// The runs of a field with the instruction `instr` and the cached result
/// `cached`.
fn field_parts<'p>(instr: &'p str, cached: &'p str) -> [&'p str; 5] {
    [
        r#"<w:fldChar w:fldCharType="begin" w:dirty="false" />"#,
        instr,
        r#"<w:fldChar w:fldCharType="separate" w:dirty="false" />"#,
        cached,
        r#"<w:fldChar w:fldCharType="end" w:dirty="false" />"#,
    ]
}

const TITLE_INSTR: &str = r"<w:instrText> TITLE  \* MERGEFORMAT </w:instrText>";

const SAMPLE: &str = "\
---
title: Sample
version: 2.1
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

const TITLE_PAGE: &str = "\
---
title: A & B
subtitle: Subtitle
author: Author
version: 1.0
date: 2026-01-01
lang: en
---
# One
";

#[test]
fn title_page_uses_styles_and_title_field() {
    let document = render_parts(TITLE_PAGE).unwrap()("word/document.xml");
    let title = between(&document, r#"<w:pStyle w:val="Title" />"#, "</w:p>");
    assert_in_order(
        title,
        &field_parts(TITLE_INSTR, r#"<w:t xml:space="preserve">A &amp; B</w:t>"#),
    );
    for (style, text) in [
        ("Subtitle", "Subtitle"),
        ("DocumentVersion", "Version 1.0"),
        ("DocumentAuthor", "Author"),
        ("DocumentDate", "2026-01-01"),
    ] {
        let p = between(
            &document,
            &format!(r#"<w:pStyle w:val="{style}" />"#),
            "</w:p>",
        );
        assert!(
            p.contains(&format!(r#"<w:t xml:space="preserve">{text}</w:t>"#)),
            "{p}"
        );
    }

    // The logo paragraph is the first body child. The Title paragraph
    // follows it.
    let logo = between(&document, "<w:body>", "</w:p>");
    assert!(
        logo.contains(r#"<wp:extent cx="1712058" cy="331470" />"#),
        "{logo}"
    );
    let start = document.find(logo).unwrap() + logo.len();
    let to_title = &document[start..document.find(r#"w:val="Title""#).unwrap()];
    assert_eq!(to_title.matches("<w:p ").count(), 1, "{to_title}");
}

#[test]
fn core_properties_have_title() {
    let core = render_parts(TITLE_PAGE).unwrap()("docProps/core.xml");
    assert!(
        core.contains("<cp:revision>1</cp:revision><dc:title>A &amp; B</dc:title>"),
        "{core}"
    );

    let core = render_parts("# One\n").unwrap()("docProps/core.xml");
    assert!(!core.contains("dc:title"), "{core}");
}

#[test]
fn header_has_logo_and_title_field() {
    let bytes = render_package(SAMPLE).unwrap().into_bytes().unwrap();
    let names = entry_names(&bytes);
    let part = parts(bytes);

    let header = part("word/header1.xml");
    assert!(
        header.contains(r#"<w:pStyle w:val="Header" />"#),
        "{header}"
    );
    assert!(header.contains("<w:drawing>"), "{header}");
    assert!(
        header.contains(r#"<wp:extent cx="688759" cy="133350" />"#),
        "{header}"
    );
    let mut header_parts = vec!["</w:drawing>", "<w:tab />", "<w:tab />"];
    header_parts.extend(field_parts(
        TITLE_INSTR,
        r#"<w:t xml:space="preserve">Sample</w:t>"#,
    ));
    assert_in_order(&header, &header_parts);

    let embed = Regex::new(r#"r:embed="([^"]+)""#).unwrap();
    let id = &embed.captures(&header).unwrap()[1];
    let rels = part("word/_rels/header1.xml.rels");
    let target = format!(
        r#"Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/{id}.png""#
    );
    assert!(rels.contains(&target), "{rels}");
    assert!(id.starts_with("header"), "{id}");
    assert!(names.contains(&format!("word/media/{id}.png")), "{names:?}");

    let footer = part("word/footer1.xml");
    assert!(
        footer.contains(r#"<w:pStyle w:val="Footer" />"#),
        "{footer}"
    );
    let mut footer_parts = field_parts(
        r"<w:instrText> DOCPROPERTY Version \* MERGEFORMAT </w:instrText>",
        r#"<w:t xml:space="preserve">2.1</w:t>"#,
    )
    .to_vec();
    footer_parts.extend(["<w:tab />", "<w:tab />"]);
    footer_parts.extend(field_parts(
        "<w:instrText>PAGE</w:instrText>",
        r#"<w:t xml:space="preserve">1</w:t>"#,
    ));
    assert_in_order(&footer, &footer_parts);
}

#[test]
fn header_without_logo() {
    let bytes = render_package_with_logo(SAMPLE, None)
        .unwrap()
        .into_bytes()
        .unwrap();
    let names = entry_names(&bytes);
    let part = parts(bytes);

    let header = part("word/header1.xml");
    assert!(!header.contains("<w:drawing>"), "{header}");
    assert_in_order(&header, &field_parts(TITLE_INSTR, "Sample"));
    let rels = part("word/_rels/header1.xml.rels");
    assert!(!rels.contains("image"), "{rels}");
    assert!(
        !names.iter().any(|n| n.starts_with("word/media/")),
        "{names:?}"
    );
    let document = part("word/document.xml");
    assert!(!document.contains("<w:drawing>"), "{document}");
}

#[test]
fn empty_toc_has_two_paragraphs_and_no_sdt() {
    let document = render_parts("#### Four\n").unwrap()("word/document.xml");
    assert!(!document.contains("<w:sdt>"), "{document}");
    let body = between(&document, "<w:body>", "<w:sectPr");
    let paragraphs: Vec<_> = PARAGRAPH.find_iter(body).map(|m| m.as_str()).collect();
    assert_in_order(
        paragraphs[0],
        &[
            r#"<w:fldChar w:fldCharType="begin" w:dirty="true" />"#,
            r#"<w:instrText>TOC \o &quot;1-3&quot; \h \u \z</w:instrText>"#,
            r#"<w:fldChar w:fldCharType="separate" w:dirty="false" />"#,
        ],
    );
    assert!(
        !paragraphs[0].contains("fldCharType=\"end\""),
        "{}",
        paragraphs[0]
    );
    assert!(
        paragraphs[1].contains(r#"<w:fldChar w:fldCharType="end" w:dirty="false" />"#),
        "{}",
        paragraphs[1]
    );
    assert!(
        paragraphs[2].contains(r#"<w:br w:type="page" />"#),
        "{}",
        paragraphs[2]
    );
    assert_eq!(body.matches("TOC").count(), 1, "{body}");
}

#[test]
fn toc_uses_toc_styles_in_sdt() {
    let document = render_parts(SAMPLE).unwrap()("word/document.xml");
    assert!(!document.contains("ToC"), "{document}");
    let sdt = between(&document, "<w:sdt>", "</w:sdt>");
    let styles = Regex::new(r#"<w:pStyle w:val="(TOC\d)" />"#).unwrap();
    let in_sdt: Vec<_> = styles
        .captures_iter(sdt)
        .map(|c| c[1].to_string())
        .collect();
    assert_eq!(in_sdt, ["TOC1", "TOC2", "TOC3", "TOC3"]);
    assert_eq!(styles.captures_iter(&document).count(), in_sdt.len());
    assert!(
        sdt.contains(r#"<w:instrText>TOC \o &quot;1-3&quot; \h \u \z</w:instrText>"#),
        "{sdt}"
    );

    let anchor = Regex::new(r#"<w:hyperlink w:anchor="([^"]+)""#).unwrap();
    let anchors: Vec<_> = anchor
        .captures_iter(sdt)
        .map(|c| c[1].to_string())
        .collect();
    let bookmark = Regex::new(r#"<w:bookmarkStart w:id="\d+" w:name="([^"]+)""#).unwrap();
    let bookmarks: Vec<_> = bookmark
        .captures_iter(&document)
        .map(|c| c[1].to_string())
        .collect();
    assert_eq!(anchors.len(), 3);
    assert_eq!(anchors, bookmarks);
}

#[test]
fn code_block_lines_use_code_block_style() {
    let document =
        render_parts("```\nfn main() {\n\n    x();\n}\n```\n").unwrap()("word/document.xml");
    let style = r#"<w:pStyle w:val="CodeBlock" />"#;
    assert_eq!(document.matches(style).count(), 4, "{document}");
    assert_in_order(
        &document,
        &[
            style,
            ">fn main() {</w:t>",
            style,
            style,
            r#"<w:t xml:space="preserve">    x();</w:t>"#,
            style,
            ">}</w:t>",
        ],
    );
    for part in [
        "<w:rFonts",
        "<w:sz ",
        "<w:shd",
        "<w:pBdr",
        "<w:spacing",
        "<w:b ",
        "<w:i ",
        "<w:strike",
        "<w:u ",
    ] {
        assert!(!document.contains(part), "{part} in {document}");
    }
}

#[test]
fn inline_code_and_links_use_character_styles() {
    let document = render_parts(
        "Text with `code`, a [link](https://example.com), **`bold code`** and ~~s~~.\n",
    )
    .unwrap()("word/document.xml");
    let code = r#"<w:rStyle w:val="InlineCode" />"#;
    let link = r#"<w:rStyle w:val="Hyperlink" />"#;
    assert_eq!(document.matches(code).count(), 2, "{document}");
    assert_eq!(document.matches(link).count(), 1, "{document}");
    for run in [
        r#"<w:rPr><w:rStyle w:val="InlineCode" /></w:rPr><w:t xml:space="preserve">code</w:t>"#,
        r#"<w:rPr><w:rStyle w:val="Hyperlink" /></w:rPr><w:t xml:space="preserve">link</w:t>"#,
    ] {
        assert!(document.contains(run), "{run} not in {document}");
    }
    let bold = between(&document, "<w:b />", "</w:rPr>");
    assert!(bold.contains(code), "{bold}");
    assert!(document.contains("<w:strike />"), "{document}");
    for part in ["<w:rFonts", "<w:shd", "<w:color", "<w:u "] {
        assert!(!document.contains(part), "{part} in {document}");
    }
}

/// A paragraph of `word/document.xml`.
static PARAGRAPH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<w:p .*?</w:p>").unwrap());

/// The style ID of a paragraph.
static PARAGRAPH_STYLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<w:pStyle w:val="([^"]+)" />"#).unwrap());

/// The text of a run.
static RUN_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<w:t(?: [^>]*)?>([^<]*)</w:t>").unwrap());

/// Gives each paragraph of `document` that has text as
/// `[<style ID>] <text>`. The style ID is empty for a paragraph without
/// `w:pStyle`.
fn text_paragraphs(document: &str) -> Vec<String> {
    PARAGRAPH
        .find_iter(document)
        .filter_map(|p| {
            let p = p.as_str();
            let t: String = RUN_TEXT
                .captures_iter(p)
                .map(|c| c[1].to_string())
                .collect();
            let s = PARAGRAPH_STYLE
                .captures(p)
                .map_or("", |c| c.get(1).unwrap().as_str());
            (!t.is_empty()).then(|| format!("[{s}] {t}"))
        })
        .collect()
}

/// Asserts that the runs of each paragraph of `document` that has a style
/// ID that starts with `style_prefix` have no direct formatting. Skips the
/// paragraphs that contain one of `skip`.
#[track_caller]
fn assert_runs_unformatted(document: &str, style_prefix: &str, skip: &[&str]) {
    let style = format!(r#"<w:pStyle w:val="{style_prefix}"#);
    let mut count = 0;
    for p in PARAGRAPH.find_iter(document).map(|m| m.as_str()) {
        if !p.contains(&style) || skip.iter().any(|s| p.contains(s)) {
            continue;
        }
        assert!(!p.contains("<w:rPr>"), "direct formatting in {p}");
        count += 1;
    }
    assert!(count > 0, "no {style_prefix} paragraph in {document}");
}

#[test]
fn admonitions_use_type_styles() {
    let document = render_parts(
        "\
:::note
## Custom title
Note text with **strong** text.
:::

:::warning
Warning text.
:::

:::important
Important text.
:::

:::caution
Caution text.
:::

:::tip
## Tip title
Tip text.
:::

:::related
Related text.
:::
",
    )
    .unwrap()("word/document.xml");
    assert_eq!(
        text_paragraphs(&document),
        [
            "[AdmonitionNoteHeader] Custom title",
            "[AdmonitionNoteBody] Note text with strong text.",
            "[AdmonitionWarningHeader] Warning",
            "[AdmonitionWarningBody] Warning text.",
            "[AdmonitionImportantHeader] Important",
            "[AdmonitionImportantBody] Important text.",
            "[AdmonitionCautionHeader] Caution",
            "[AdmonitionCautionBody] Caution text.",
            "[AdmonitionTipHeader] Tip title",
            "[AdmonitionTipBody] Tip text.",
            "[AdmonitionRelatedHeader] Related",
            "[AdmonitionRelatedBody] Related text.",
        ]
    );
    // The bold run of the strong text is the only direct formatting.
    assert_runs_unformatted(&document, "Admonition", &[">strong</w:t>"]);
    assert_eq!(document.matches("<w:b />").count(), 1, "{document}");
    for part in [
        "<w:i />",
        "<w:strike />",
        "<w:shd",
        "<w:pBdr",
        "<w:spacing",
        "<w:sz ",
        "<w:color",
    ] {
        assert!(!document.contains(part), "{part} in {document}");
    }
}

#[test]
fn admonition_nested_blocks_keep_their_styles() {
    let document = render_parts(
        "\
::::note
Before.

```
code
```

- Item

  :::aside
  In a list.
  :::

> Quoted.

:::aside
In a plain div.
:::

After.
::::
",
    )
    .unwrap()("word/document.xml");
    assert_eq!(
        text_paragraphs(&document),
        [
            "[AdmonitionNoteHeader] Note",
            "[AdmonitionNoteBody] Before.",
            "[CodeBlock] code",
            "[ListParagraph] Item",
            "[] In a list.",
            "[Quote] Quoted.",
            "[AdmonitionNoteBody] In a plain div.",
            "[AdmonitionNoteBody] After.",
        ]
    );
}

#[test]
fn block_quote_paragraphs_use_quote_style() {
    let document = render_parts(
        "\
> Quoted.
>
> > Nested.
>
> ```
> code
> ```

After.
",
    )
    .unwrap()("word/document.xml");
    assert_eq!(
        text_paragraphs(&document),
        [
            "[Quote] Quoted.",
            "[Quote] Nested.",
            "[CodeBlock] code",
            "[] After.",
        ]
    );
}

#[test]
fn definition_lists_use_definition_styles() {
    let document = render_parts(
        "\
Term
Second term
: First paragraph.

  Second paragraph.
",
    )
    .unwrap()("word/document.xml");
    assert_eq!(
        text_paragraphs(&document),
        [
            "[DefinitionTerm] Term",
            "[DefinitionTerm] Second term",
            "[DefinitionBody] First paragraph.",
            "[DefinitionBody] Second paragraph.",
        ]
    );
    assert_runs_unformatted(&document, "Definition", &[]);
    for part in ["<w:ind", "<w:b />", "<w:i />", "<w:strike />"] {
        assert!(!document.contains(part), "{part} in {document}");
    }
}

#[test]
fn definition_body_gives_alt_text_of_unresolved_image() {
    let document = render_parts("Term\n: ![Remote def alt](https://example.com/y.png)\n").unwrap()(
        "word/document.xml",
    );
    assert_eq!(
        text_paragraphs(&document),
        ["[DefinitionTerm] Term", "[DefinitionBody] Remote def alt"]
    );
    assert!(!document.contains("<w:drawing>"), "{document}");
}

#[test]
fn definition_body_renders_nested_blocks() {
    let document = render_parts(
        "\
Term
: Text.

  ```
  code
  ```

  Inner term
  : Inner text.
",
    )
    .unwrap()("word/document.xml");
    assert_eq!(
        text_paragraphs(&document),
        [
            "[DefinitionTerm] Term",
            "[DefinitionBody] Text.",
            "[CodeBlock] code",
            "[DefinitionTerm] Inner term",
            "[DefinitionBody] Inner text.",
        ]
    );
}

#[test]
fn thematic_break_uses_thematic_break_style() {
    let document = render_parts("Before.\n\n***\n\nAfter.\n").unwrap()("word/document.xml");
    let style = r#"<w:pStyle w:val="ThematicBreak" />"#;
    assert_eq!(document.matches(style).count(), 1, "{document}");
    assert_in_order(&document, &[">Before.</w:t>", style, ">After.</w:t>"]);
    assert!(!document.contains("<w:pBdr"), "{document}");
}

/// Gives the cache tag and the cache key of each diagram job that the
/// prefetch walk collects from `markdown`.
fn diagram_jobs(markdown: &str) -> Vec<(&'static str, Vec<u8>)> {
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
    let graph = prepared.graph();
    let mut jobs = Vec::new();
    with_renderer(None, |r| {
        r.collect_graph_diagrams(graph, graph.root_id, &mut jobs)
    })
    .unwrap();
    jobs.iter()
        .map(|job| (job.cache_tag(), job.cache_key().to_vec()))
        .collect()
}

#[test]
fn prefetch_collects_diagram_in_definition_body() {
    let diagram = "```mermaid\ngraph LR\n  A --> B\n```\n";
    let top_level = diagram_jobs(diagram);
    assert_eq!(top_level.len(), 1);
    let indented = diagram.replace('\n', "\n  ");
    let in_definition = diagram_jobs(&format!("Term\n: Text.\n\n  {indented}"));
    assert_eq!(in_definition, top_level);
}
