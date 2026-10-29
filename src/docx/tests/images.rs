//! Tests of the image and diagram extents, and of the `ImageBlock`
//! paragraph style.

use tempfile::TempDir;

use crate::cache::Cache;

use super::super::media::DiagramJob;
use super::{parts, png, render_in, render_with, text_paragraphs};

/// The text width and the text height of the blank theme in EMU.
const TEXT_WIDTH: u32 = 6_480_175;
const TEXT_HEIGHT: u32 = 9_251_950;

/// The `ImageBlock` paragraph style.
const IMAGE_BLOCK: &str = r#"<w:pStyle w:val="ImageBlock" />"#;

/// The extent element of an image of `cx` x `cy` EMU.
fn extent(cx: u32, cy: u32) -> String {
    format!(r#"<wp:extent cx="{cx}" cy="{cy}" />"#)
}

/// Renders `markdown` with a PNG image `image.png` of `width` x `height`
/// pixels, and gives `word/document.xml`.
fn document_with_png(markdown: &str, width: u32, height: u32) -> String {
    let package = render_with(markdown, &[("image.png", png(width, height))], |_| {}).unwrap();
    parts(package.into_bytes().unwrap())("word/document.xml")
}

/// Asserts that the image of `markdown` on a PNG of `width` x `height`
/// pixels has the extent `cx` x `cy` EMU.
#[track_caller]
fn assert_extent(markdown: &str, (width, height): (u32, u32), (cx, cy): (u32, u32)) {
    let document = document_with_png(markdown, width, height);
    assert_eq!(document.matches("<wp:extent ").count(), 1, "{document}");
    assert!(document.contains(&extent(cx, cy)), "{markdown}: {document}");
}

#[test]
fn width_sets_extent_and_keeps_aspect_ratio() {
    assert_extent(
        "![a](image.png){width=5cm}\n",
        (400, 100),
        (1_800_000, 450_000),
    );
}

#[test]
fn image_wider_than_text_width_is_limited() {
    assert_extent("![a](image.png)\n", (2000, 400), (TEXT_WIDTH, 1_296_035));
    assert_extent(
        "![a](image.png){width=30cm}\n",
        (400, 100),
        (TEXT_WIDTH, 1_620_044),
    );
}

#[test]
fn image_taller_than_text_height_is_limited() {
    // 2312987.5 EMU rounds up.
    assert_extent(
        "![a](image.png){height=30cm}\n",
        (100, 400),
        (2_312_988, TEXT_HEIGHT),
    );
    assert_extent(
        "![a](image.png){height=100%}\n",
        (100, 400),
        (2_312_988, TEXT_HEIGHT),
    );
}

#[test]
fn image_without_attributes_has_pixel_size_at_96_dpi() {
    assert_extent("![a](image.png)\n", (400, 100), (3_810_000, 952_500));
    assert_extent(
        "![a](image.png){width=auto height=auto}\n",
        (400, 100),
        (3_810_000, 952_500),
    );
}

#[test]
fn width_percent_is_part_of_text_width() {
    for markdown in [
        "![a](image.png){width=50%}\n",
        "![a](image.png){width=\"50%\"}\n",
        "![a](image.png){width=\" 50% \"}\n",
    ] {
        assert_extent(markdown, (400, 100), (3_240_088, 810_022));
    }
}

#[test]
fn height_sets_extent_and_keeps_aspect_ratio() {
    assert_extent(
        "![a](image.png){height=1in}\n",
        (400, 100),
        (3_657_600, 914_400),
    );
}

#[test]
fn width_and_height_are_used_exactly() {
    assert_extent(
        "![a](image.png){width=2cm height=3cm}\n",
        (400, 100),
        (720_000, 1_080_000),
    );
}

#[test]
fn em_is_normal_font_size() {
    // Normal is 11 pt = 139700 EMU.
    assert_extent(
        "![a](image.png){width=4em}\n",
        (400, 100),
        (558_800, 139_700),
    );
}

#[test]
fn invalid_length_is_an_error() {
    for (attributes, message) in [
        (
            "width=abc",
            "width: invalid number in 'abc'; use a number and a unit such as 5cm",
        ),
        ("width=0", "width: length '0' must be more than 0"),
        ("height=-1cm", "height: negative length '-1cm'"),
        (
            "width=\"50% + 1cm\"",
            "width: unknown unit '% + 1cm' in '50% + 1cm'; use %, px, pt, cm, mm, in or em",
        ),
        (
            "width=3xx",
            "width: unknown unit 'xx' in '3xx'; use %, px, pt, cm, mm, in or em",
        ),
    ] {
        let markdown = format!("![a](image.png){{{attributes}}}\n");
        let err = render_with(&markdown, &[("image.png", png(4, 1))], |_| {})
            .err()
            .unwrap()
            .to_string();
        let expected = format!("index.md: image `image.png`: {message}");
        assert!(err.ends_with(&expected), "{err}\nexpected: {expected}");
    }
}

/// Renders `markdown` with a FIFO `name` in the directory and an artifact
/// cache in a temporary directory, and tells if the render opened the
/// FIFO. Gives the render error, if any. A render that does not end in 60
/// seconds makes the test fail.
#[cfg(unix)]
fn opens_fifo(markdown: &str, name: &str) -> (bool, Option<String>) {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    let temp = TempDir::new().unwrap();
    let fifo = temp.path().join(name);
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    // The writer opens the FIFO each time a reader opens it, and then
    // closes it, thus each reader gets an empty file. After `stop`, the
    // writer ends at the next open.
    let opens = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (fifo, opens, stop) = (fifo.clone(), opens.clone(), stop.clone());
        std::thread::spawn(move || loop {
            let file = std::fs::OpenOptions::new().write(true).open(&fifo);
            if stop.load(Ordering::SeqCst) {
                break;
            }
            opens.fetch_add(1, Ordering::SeqCst);
            drop(file);
        })
    };
    let (sender, receiver) = mpsc::channel();
    {
        let (dir, markdown) = (temp.path().to_owned(), markdown.to_owned());
        std::thread::spawn(move || {
            let cache = Cache::new(Some(dir.join("cache")), false).unwrap();
            let err = render_in(&dir, &markdown, &cache, |_| {})
                .err()
                .map(|e| e.to_string());
            sender.send(err).unwrap();
        });
    }
    let err = receiver
        .recv_timeout(Duration::from_secs(60))
        .expect("the render did not end in 60 seconds");
    // Release the writer.
    stop.store(true, Ordering::SeqCst);
    drop(std::fs::File::open(&fifo).unwrap());
    writer.join().unwrap();
    (opens.load(Ordering::SeqCst) > 0, err)
}

#[cfg(unix)]
#[test]
fn invalid_length_does_not_read_image_file() {
    let (opened, err) = opens_fifo("![a](image.png){width=abc}\n", "image.png");
    assert!(!opened);
    assert!(err.unwrap().contains("width: invalid number"));
    // The check of the helper: a valid image reads the file.
    let (opened, err) = opens_fifo("![a](image.png){width=1cm}\n", "image.png");
    assert!(opened);
    assert_eq!(err, None);
}

#[cfg(unix)]
#[test]
fn invalid_diagram_length_does_not_read_drawio_file() {
    let (opened, err) = opens_fifo("![a](flow.drawio){width=abc}\n", "flow.drawio");
    assert!(!opened);
    let err = err.unwrap();
    assert!(
        err.ends_with(
            "index.md: diagram `flow.drawio`: width: invalid number in 'abc'; \
             use a number and a unit such as 5cm"
        ),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn svg_image_is_skipped_without_read() {
    let (opened, err) = opens_fifo("![SVG alt](image.svg)\n", "image.svg");
    assert!(!opened);
    assert_eq!(err, None);
}

#[test]
fn svg_image_gives_alt_text() {
    let package = render_with(
        "![SVG alt](image.svg){width=1cm}\n",
        &[("image.svg", b"<svg/>".to_vec())],
        |_| {},
    )
    .unwrap();
    let document = parts(package.into_bytes().unwrap())("word/document.xml");
    assert!(!document.contains("<w:drawing>"), "{document}");
    assert_eq!(text_paragraphs(&document), ["[ImageBlock] SVG alt"]);
}

#[test]
fn sole_image_has_image_block_style() {
    let document = document_with_png("Text.\n\n![a](image.png)\n\nText.\n", 4, 1);
    assert_eq!(document.matches(IMAGE_BLOCK).count(), 1, "{document}");
    let paragraph = super::between(&document, IMAGE_BLOCK, "</w:p>");
    assert!(paragraph.contains("<w:drawing>"), "{paragraph}");
}

#[test]
fn image_in_text_has_normal_font_height() {
    let document = document_with_png("Text ![a](image.png) text.\n", 400, 100);
    assert!(!document.contains(IMAGE_BLOCK), "{document}");
    assert!(document.contains(&extent(558_800, 139_700)), "{document}");
    assert_eq!(text_paragraphs(&document), ["[] Text  text."]);

    let document = document_with_png("Text ![a](image.png){width=2cm} text.\n", 400, 100);
    assert!(document.contains(&extent(720_000, 180_000)), "{document}");
    let document = document_with_png("Text ![a](image.png) text.\n", 4000, 1);
    assert!(document.contains(&extent(TEXT_WIDTH, 1620)), "{document}");
}

#[test]
fn image_in_block_quote_keeps_quote_style() {
    let document = document_with_png("> ![a](image.png)\n", 4, 1);
    assert!(!document.contains(IMAGE_BLOCK), "{document}");
    let paragraph = super::between(&document, r#"<w:pStyle w:val="Quote" />"#, "</w:p>");
    assert!(paragraph.contains("<w:drawing>"), "{paragraph}");
}

/// The source of the Typst diagrams.
const TYPST_SOURCE: &str = "#rect()\n";

/// Renders `markdown` with the artifact cache that holds `png` for a Typst
/// diagram with [`TYPST_SOURCE`], and gives `word/document.xml`.
fn document_with_typst_png(markdown: &str, png: &[u8]) -> String {
    let temp = TempDir::new().unwrap();
    let cache = Cache::new(Some(temp.path().join("cache")), false).unwrap();
    cache
        .store(TYPST_SOURCE.as_bytes(), "typst.png", png)
        .unwrap();
    let package = render_in(temp.path(), markdown, &cache, |_| {}).unwrap();
    parts(package.into_bytes().unwrap())("word/document.xml")
}

#[test]
fn diagram_width_sets_extent_and_image_block_style() {
    let document = document_with_typst_png(
        &format!("```typst {{width=5cm}}\n{TYPST_SOURCE}```\n"),
        &png(400, 100),
    );
    assert!(document.contains(&extent(1_800_000, 450_000)), "{document}");
    let paragraph = super::between(&document, IMAGE_BLOCK, "</w:p>");
    assert!(paragraph.contains("<w:drawing>"), "{paragraph}");
}

#[test]
fn typst_diagram_uses_144_ppi() {
    let document =
        document_with_typst_png(&format!("```typst\n{TYPST_SOURCE}```\n"), &png(288, 144));
    assert!(document.contains(&extent(1_828_800, 914_400)), "{document}");
}

#[test]
fn diagram_in_block_quote_has_quote_style() {
    let document = document_with_typst_png(
        &format!("> ```typst\n> {TYPST_SOURCE}> ```\n"),
        &png(288, 144),
    );
    assert!(document.contains(&extent(1_828_800, 914_400)), "{document}");
    assert!(!document.contains(IMAGE_BLOCK), "{document}");
    let paragraph = super::between(&document, r#"<w:pStyle w:val="Quote" />"#, "</w:p>");
    assert!(paragraph.contains("<w:drawing>"), "{paragraph}");
}

#[test]
fn diagram_in_admonition_has_body_style() {
    let document = document_with_typst_png(
        &format!(
            "\
---
lang: en
---
:::note
Figure: Flow

```typst
{TYPST_SOURCE}```
:::
"
        ),
        &png(288, 144),
    );
    assert!(!document.contains(IMAGE_BLOCK), "{document}");
    assert_eq!(
        text_paragraphs(&document),
        ["[AdmonitionNoteHeader] Note", "[Caption] Figure 1: Flow"]
    );
    let paragraph = super::between(
        &document,
        r#"<w:pStyle w:val="AdmonitionNoteBody" />"#,
        "</w:p>",
    );
    assert!(paragraph.contains("<w:drawing>"), "{paragraph}");
}

/// The error of a `height=1tw` attribute after the document name and the
/// diagram name.
const DIAGRAM_HEIGHT_ERROR: &str =
    "height: unknown unit 'tw' in '1tw'; use %, px, pt, cm, mm, in or em";

#[test]
fn invalid_diagram_length_is_an_error() {
    let err = render_with("```typst {height=1tw}\n#rect()\n```\n", &[], |_| {})
        .err()
        .unwrap()
        .to_string();
    let expected = format!("index.md: typst diagram: {DIAGRAM_HEIGHT_ERROR}");
    assert!(err.ends_with(&expected), "{err}\nexpected: {expected}");

    let err = render_with(
        "![a](flow.puml){height=1tw}\n",
        &[("flow.puml", b"@startuml\nA -> B\n@enduml\n".to_vec())],
        |_| {},
    )
    .err()
    .unwrap()
    .to_string();
    let expected = format!("index.md: diagram `flow.puml`: {DIAGRAM_HEIGHT_ERROR}");
    assert!(err.ends_with(&expected), "{err}\nexpected: {expected}");
}

#[test]
fn invalid_diagram_length_writes_no_cache_entry() {
    let temp = TempDir::new().unwrap();
    let cache_dir = temp.path().join("cache");
    let cache = Cache::new(Some(cache_dir.clone()), false).unwrap();
    let render = |height: &str| {
        render_in(
            temp.path(),
            &format!("```typst {{height={height}}}\n#rect()\n```\n"),
            &cache,
            |_| {},
        )
    };
    let err = render("1tw").err().unwrap().to_string();
    // The message is the same as without the cache.
    let expected = format!("index.md: typst diagram: {DIAGRAM_HEIGHT_ERROR}");
    assert!(err.ends_with(&expected), "{err}\nexpected: {expected}");
    let entries = || std::fs::read_dir(&cache_dir).map_or(0, |d| d.count());
    assert_eq!(entries(), 0);
    // The check of the test: a valid diagram writes a cache entry.
    if which::which("typst").is_err() {
        eprintln!("skipping the cache entry check: typst not found on PATH");
        return;
    }
    render("1cm").unwrap();
    assert_eq!(entries(), 1);
}

#[test]
fn diagram_pixels_per_inch() {
    let ppi = |job: DiagramJob| job.ppi();
    assert_eq!(ppi(DiagramJob::Typst(String::new())), 144);
    assert_eq!(ppi(DiagramJob::D2(String::new())), 192);
    assert_eq!(ppi(DiagramJob::Mermaid(String::new())), 96);
    assert_eq!(ppi(DiagramJob::PlantUml(String::new())), 96);
}
