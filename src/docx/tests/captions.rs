//! Tests of the caption and diagram error output in `word/document.xml`.

use std::sync::LazyLock;

use regex::Regex;

use super::{
    assert_in_order, assert_runs_unformatted, field_parts, render_parts, text_paragraphs, PARAGRAPH,
};

/// The ID and the name of a bookmark start.
static BOOKMARK_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<w:bookmarkStart w:id="(\d+)" w:name="([^"]+)" />"#).unwrap());

/// The ID of a bookmark end.
static BOOKMARK_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<w:bookmarkEnd w:id="(\d+)" />"#).unwrap());

/// Renders `markdown` and gives `word/document.xml`.
fn render_document(markdown: &str) -> String {
    render_parts(markdown).unwrap()("word/document.xml")
}

/// Gives the paragraphs of `document` with the `Caption` style.
fn captions(document: &str) -> Vec<&str> {
    PARAGRAPH
        .find_iter(document)
        .map(|m| m.as_str())
        .filter(|p| p.contains(r#"<w:pStyle w:val="Caption" />"#))
        .collect()
}

/// The instruction of a `SEQ` field for the sequence `seq`.
fn seq_instr(seq: &str) -> String {
    format!(r"<w:instrText> SEQ {seq} \* ARABIC </w:instrText>")
}

/// The run of the text `text`.
fn text_run(text: &str) -> String {
    format!(r#"<w:t xml:space="preserve">{text}</w:t>"#)
}

/// Gives the ID and the name of the bookmark of `caption`, and asserts
/// that the bookmark holds all the caption content.
#[track_caller]
fn caption_bookmark(caption: &str) -> (String, String) {
    let start = BOOKMARK_START
        .captures(caption)
        .unwrap_or_else(|| panic!("no bookmark start in {caption}"));
    let end = BOOKMARK_END
        .captures(caption)
        .unwrap_or_else(|| panic!("no bookmark end in {caption}"));
    assert_eq!(start[1], end[1], "{caption}");
    assert!(
        caption.contains(&format!("</w:pPr>{}", &start[0])),
        "{caption}"
    );
    assert!(
        caption.ends_with(&format!("{}</w:p>", &end[0])),
        "{caption}"
    );
    (start[1].to_string(), start[2].to_string())
}

#[test]
fn captions_number_each_kind_with_seq_fields() {
    let document = render_document(
        "\
---
lang: en
---
Table: Alpha

| A | B |
|---|---|
| 1 | 2 |

Figure: Diagram

:::figure
Picture
:::

Table: Beta

| C | D |
|---|---|
| 3 | 4 |
",
    );
    let captions = captions(&document);
    assert_eq!(captions.len(), 3, "{document}");
    for (caption, word, seq, number, text) in [
        (captions[0], "Table ", "Table", "1", "Alpha"),
        (captions[1], "Figure ", "Figure", "1", "Diagram"),
        (captions[2], "Table ", "Table", "2", "Beta"),
    ] {
        let word = text_run(word);
        let instr = seq_instr(seq);
        let cached = text_run(number);
        let separator = text_run(": ");
        let text = text_run(text);
        let mut parts = vec![word.as_str()];
        parts.extend(field_parts(&instr, &cached));
        parts.extend([separator.as_str(), text.as_str()]);
        assert_in_order(caption, &parts);
    }

    let bookmarks: Vec<_> = captions.iter().map(|c| caption_bookmark(c)).collect();
    for (id, name) in &bookmarks {
        assert_eq!(*name, format!("_Ref{:09}", id.parse::<usize>().unwrap()));
    }
    assert_ne!(bookmarks[0].0, bookmarks[1].0);
    assert_ne!(bookmarks[1].0, bookmarks[2].0);

    // Each caption is before its block.
    assert_in_order(
        &document,
        &[
            ">Alpha<",
            "<w:tbl>",
            ">Diagram<",
            ">Picture<",
            ">Beta<",
            "<w:tbl>",
        ],
    );
    assert_runs_unformatted(&document, "Caption", &[]);
    for caption in captions {
        assert!(!caption.contains("<w:spacing"), "{caption}");
    }
}

#[test]
fn russian_captions_use_russian_words() {
    let document = render_document(
        "\
---
lang: ru-RU
---
Listing: Пример

```
code
```
",
    );
    let captions = captions(&document);
    assert_eq!(captions.len(), 1, "{document}");
    let word = text_run("Листинг ");
    let instr = seq_instr("Listing");
    let cached = text_run("1");
    let separator = text_run(". ");
    let text = text_run("Пример");
    let mut parts = vec![word.as_str()];
    parts.extend(field_parts(&instr, &cached));
    parts.extend([separator.as_str(), text.as_str()]);
    assert_in_order(captions[0], &parts);
}

#[test]
fn caption_without_text_has_no_separator() {
    let document = render_document(
        "\
---
lang: en
---
Table: {#values}

| A | B |
|---|---|
| 1 | 2 |
",
    );
    let captions = captions(&document);
    assert_eq!(captions.len(), 1, "{document}");
    let end = r#"<w:fldChar w:fldCharType="end" w:dirty="false" /></w:r><w:bookmarkEnd "#;
    assert!(captions[0].contains(end), "{}", captions[0]);
    assert!(!captions[0].contains(&text_run(": ")), "{}", captions[0]);
}

#[test]
fn caption_bookmark_differs_from_heading_bookmark() {
    let document = render_document(
        "\
---
lang: en
---
# Heading

Table: Values

| A | B |
|---|---|
| 1 | 2 |
",
    );
    let heading = PARAGRAPH
        .find_iter(&document)
        .map(|m| m.as_str())
        .find(|p| p.contains(r#"<w:pStyle w:val="Heading1" />"#))
        .unwrap();
    let heading_id = &BOOKMARK_START.captures(heading).unwrap()[1];
    let (caption_id, _) = caption_bookmark(captions(&document)[0]);
    assert_ne!(heading_id, caption_id);
}

#[test]
fn caption_in_admonition_uses_caption_style() {
    let document = render_document(
        "\
---
lang: en
---
:::note
Note text.

Table: Values

| A | B |
|---|---|
| 1 | 2 |
:::
",
    );
    let paragraphs = text_paragraphs(&document);
    let at = paragraphs
        .iter()
        .position(|p| p == "[AdmonitionNoteBody] Note text.")
        .unwrap();
    assert_eq!(paragraphs[at + 1], "[Caption] Table 1: Values");
    assert_runs_unformatted(&document, "Caption", &[]);
}

#[test]
fn diagram_error_uses_diagram_error_style() {
    let document = render_document(
        "\
---
lang: en
---
```typst
#panic(\"diagram error\")
```
",
    );
    let paragraphs = text_paragraphs(&document);
    assert!(
        paragraphs
            .iter()
            .any(|p| p.starts_with("[DiagramError] [Diagram rendering error: ")),
        "{paragraphs:?}"
    );
    assert_runs_unformatted(&document, "DiagramError", &[]);
}

#[test]
fn caption_of_hidden_block_is_not_rendered() {
    let document = render_document(
        "\
---
lang: en
---
Table: Alpha

| A |
|---|
| 1 |

Table: Hidden target

::: {if-format=\"html\"}
| B |
|---|
| 7 |
:::

Table: Beta

| C |
|---|
| 3 |
",
    );
    let paragraphs = text_paragraphs(&document);
    assert!(
        !paragraphs.iter().any(|p| p.contains("Hidden target")),
        "{paragraphs:?}"
    );
    let captions: Vec<_> = paragraphs
        .iter()
        .filter(|p| p.starts_with("[Caption] "))
        .collect();
    assert_eq!(
        captions,
        ["[Caption] Table 1: Alpha", "[Caption] Table 2: Beta"]
    );
}

#[test]
fn caption_text_can_start_with_code_or_emphasis() {
    let document = render_document(
        "\
---
lang: en
---
Table: `code` rest

| A |
|---|
| 1 |

Table: *emph* rest

| B |
|---|
| 2 |

Table: `code`

| C |
|---|
| 3 |

Table: *emph*

| D |
|---|
| 4 |
",
    );
    let captions = captions(&document);
    assert_eq!(captions.len(), 4, "{document}");
    let separator = text_run(": ");
    for (caption, text) in captions.iter().zip(["code", "emph", "code", "emph"]) {
        assert!(!caption.contains(&text_run("")), "{caption}");
        let (_, after) = caption.split_once(separator.as_str()).unwrap();
        let first_run = after.split("</w:r>").nth(1).unwrap();
        assert!(first_run.ends_with(&text_run(text)), "{caption}");
    }
    let paragraphs: Vec<_> = text_paragraphs(&document)
        .into_iter()
        .filter(|p| p.starts_with("[Caption] "))
        .collect();
    assert_eq!(
        paragraphs,
        [
            "[Caption] Table 1: code rest",
            "[Caption] Table 2: emph rest",
            "[Caption] Table 3: code",
            "[Caption] Table 4: emph",
        ]
    );
}
