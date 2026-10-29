//! Tests of the footnote output: `word/footnotes.xml`, the references in
//! `word/document.xml`, and the package parts that refer to the footnotes.

use std::sync::LazyLock;

use docx_rs::{BuildXML, Footnote, Run};
use regex::Regex;

use super::{assert_in_order, between, parts, png, render_parts, render_with};

/// A footnote of `word/footnotes.xml` with its ID. The separator footnotes
/// have a `w:type` and do not match.
static FOOTNOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<w:footnote w:id="(\d+)">(.*?)</w:footnote>"#).unwrap());

/// A paragraph of `word/footnotes.xml`.
static PARAGRAPH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<w:p>.*?</w:p>").unwrap());

/// The style ID of a footnote paragraph.
static STYLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<w:pStyle w:val="([^"]+)"/>"#).unwrap());

/// The text of a run, or a tab.
static TEXT_OR_TAB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<w:t(?: [^>]*)?>([^<]*)</w:t>|<w:tab />").unwrap());

/// The footnote mark and the space after it.
const MARK: &str = concat!(
    r#"<w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r>"#,
    r#"<w:r><w:t xml:space="preserve"> </w:t></w:r>"#,
);

/// The footnote properties of `word/settings.xml`.
const FOOTNOTE_PROPERTIES: &str =
    r#"<w:footnotePr><w:footnote w:id="-1"/><w:footnote w:id="0"/></w:footnotePr>"#;

/// The content type override of the footnotes part.
const FOOTNOTES_OVERRIDE: &str = r#"<Override PartName="/word/footnotes.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml" />"#;

/// The relationship type of the footnotes part.
const FOOTNOTES_RELATIONSHIP: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";

/// The reference run of the footnote `id` in `word/document.xml`.
fn reference(id: usize) -> String {
    format!(
        r#"<w:r><w:rPr><w:rStyle w:val="FootnoteReference" /></w:rPr><w:footnoteReference w:id="{id}" /></w:r>"#
    )
}

/// Gives the ID and the content of each footnote of `xml`.
fn footnotes(xml: &str) -> Vec<(usize, &str)> {
    FOOTNOTE
        .captures_iter(xml)
        .map(|c| (c[1].parse().unwrap(), c.get(2).unwrap().as_str()))
        .collect()
}

/// Gives each paragraph of the footnote content `footnote` as
/// `[<style ID>] <text>`. A tab is `\t`.
fn paragraphs(footnote: &str) -> Vec<String> {
    PARAGRAPH
        .find_iter(footnote)
        .map(|p| {
            let p = p.as_str();
            let style = STYLE.captures(p).map_or("", |c| c.get(1).unwrap().as_str());
            let text: String = TEXT_OR_TAB
                .captures_iter(p)
                .map(|c| c.get(1).map_or("\t", |t| t.as_str()).to_string())
                .collect();
            format!("[{style}] {text}")
        })
        .collect()
}

#[test]
fn reference_and_text_are_in_package() {
    let part = render_parts("Text with a note[^a].\n\n[^a]: Footnote.\n").unwrap();
    let document = part("word/document.xml");
    assert!(document.contains(&reference(1)), "{document}");
    assert!(!document.contains("Footnote."), "{document}");

    let xml = part("word/footnotes.xml");
    let notes = footnotes(&xml);
    assert_eq!(notes.len(), 1, "{xml}");
    assert_eq!(notes[0].0, 1);
    assert_in_order(
        notes[0].1,
        &[
            r#"<w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr>"#,
            MARK,
            r#"<w:t xml:space="preserve">Footnote.</w:t>"#,
            "</w:p>",
        ],
    );
    assert_eq!(paragraphs(notes[0].1), ["[FootnoteText]  Footnote."]);

    let content_types = part("[Content_Types].xml");
    assert!(
        content_types.contains(FOOTNOTES_OVERRIDE),
        "{content_types}"
    );
    let rels = part("word/_rels/document.xml.rels");
    assert!(rels.contains(FOOTNOTES_RELATIONSHIP), "{rels}");
    assert!(rels.contains(r#"Target="footnotes.xml""#), "{rels}");
    let settings = part("word/settings.xml");
    assert!(
        settings.contains(&format!("{FOOTNOTE_PROPERTIES}<w:compat>")),
        "{settings}"
    );
    assert_eq!(settings.matches("<w:footnotePr>").count(), 1);
}

#[test]
fn multi_paragraph_footnote_has_code_block_lines() {
    let xml = render_parts(
        "\
Text[^b].

[^b]:
    First paragraph.

    ```
    let a = 1;
    let b = 2;
    ```

    Last paragraph.
",
    )
    .unwrap()("word/footnotes.xml");
    let notes = footnotes(&xml);
    assert_eq!(notes.len(), 1, "{xml}");
    assert_eq!(
        paragraphs(notes[0].1),
        [
            "[FootnoteText]  First paragraph.",
            "[CodeBlock] let a = 1;",
            "[CodeBlock] let b = 2;",
            "[FootnoteText] Last paragraph.",
        ]
    );
    assert_eq!(notes[0].1.matches("<w:footnoteRef/>").count(), 1);
    assert!(notes[0].1.starts_with(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr>{MARK}"#
    )));
}

#[test]
fn reference_in_table_cell_gets_footnote() {
    let part = render_parts(
        "\
| Name | Value |
|------|-------|
| one  | 1[^c] |

[^c]: Table note.
",
    )
    .unwrap();
    let document = part("word/document.xml");
    let cell = Regex::new(r"(?s)<w:tc>.*?</w:tc>").unwrap();
    let cells: Vec<_> = cell
        .find_iter(&document)
        .map(|m| m.as_str())
        .filter(|c| c.contains(&reference(1)))
        .collect();
    assert_eq!(cells.len(), 1, "{document}");
    assert_eq!(document.matches("<w:footnoteReference ").count(), 1);
    assert_eq!(
        paragraphs(footnotes(&part("word/footnotes.xml"))[0].1),
        ["[FootnoteText]  Table note."]
    );
    assert!(part("[Content_Types].xml").contains(FOOTNOTES_OVERRIDE));
    assert!(part("word/_rels/document.xml.rels").contains(FOOTNOTES_RELATIONSHIP));
    assert!(part("word/settings.xml").contains(FOOTNOTE_PROPERTIES));
}

#[test]
fn repeated_label_gives_new_footnotes() {
    let part = render_parts("One[^a] and two[^a].\n\n[^a]: Same note.\n").unwrap();
    let document = part("word/document.xml");
    assert_in_order(&document, &[&reference(1), &reference(2)]);
    let xml = part("word/footnotes.xml");
    let notes = footnotes(&xml);
    assert_eq!(notes.len(), 2, "{xml}");
    for (i, (id, content)) in notes.iter().enumerate() {
        assert_eq!(*id, i + 1);
        assert_eq!(paragraphs(content), ["[FootnoteText]  Same note."]);
    }
}

#[test]
fn separators_come_first() {
    let xml = render_parts("Text[^a].\n\n[^a]: Note.\n").unwrap()("word/footnotes.xml");
    assert!(xml.starts_with(concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
        r#"<w:footnote w:type="separator" w:id="-1"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr>"#,
        r#"<w:r><w:separator/></w:r></w:p></w:footnote>"#,
        r#"<w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr>"#,
        r#"<w:r><w:continuationSeparator/></w:r></w:p></w:footnote>"#,
        r#"<w:footnote w:id="1">"#,
    )));
    assert!(xml.ends_with("</w:footnote></w:footnotes>"), "{xml}");
}

#[test]
fn document_without_footnotes_has_no_footnote_references() {
    let part = render_parts("Text.\n").unwrap();
    assert!(!part("[Content_Types].xml").contains("footnotes"));
    assert!(!part("word/_rels/document.xml.rels").contains("footnotes"));
    assert!(!part("word/settings.xml").contains("footnotePr"));
    assert!(!part("word/document.xml").contains("footnoteReference"));
    let xml = part("word/footnotes.xml");
    assert!(!xml.contains("<w:footnote "), "{xml}");
    assert!(xml.contains("<w:footnotes "), "{xml}");
}

#[test]
fn hidden_reference_gives_no_footnote() {
    let part = render_parts(
        "\
::: {if-format=\"html\"}
Hidden[^a].
:::

Shown[^b].

[^a]: Hidden note.

[^b]: Shown note.
",
    )
    .unwrap();
    let document = part("word/document.xml");
    assert_eq!(document.matches("<w:footnoteReference ").count(), 1);
    assert!(document.contains(&reference(1)), "{document}");
    let xml = part("word/footnotes.xml");
    let notes = footnotes(&xml);
    assert_eq!(notes.len(), 1, "{xml}");
    assert_eq!(paragraphs(notes[0].1), ["[FootnoteText]  Shown note."]);
}

#[test]
fn image_in_footnote_gives_alt_text_and_nested_reference_gives_nothing() {
    let png = png(2, 2);
    let package = render_with(
        "\
Body ![body image](image.png) text[^a].

[^a]: See ![alt text](image.png) and a nested note[^b].

[^b]: Nested note.
",
        &[("image.png", png)],
        |_| {},
    )
    .unwrap();
    let part = parts(package.into_bytes().unwrap());
    let document = part("word/document.xml");
    assert_eq!(document.matches("<w:drawing>").count(), 1, "{document}");
    assert_eq!(document.matches("<w:footnoteReference ").count(), 1);
    let xml = part("word/footnotes.xml");
    let notes = footnotes(&xml);
    assert_eq!(notes.len(), 1, "{xml}");
    assert_eq!(
        paragraphs(notes[0].1),
        ["[FootnoteText]  See alt text and a nested note."]
    );
    assert!(!xml.contains("footnoteReference"), "{xml}");
    assert!(!xml.contains("<w:drawing"), "{xml}");
}

#[test]
fn blocks_in_footnote_become_paragraphs() {
    let xml = render_parts(
        "\
Text[^a].

[^a]:
    # Heading

    > Quote.

    - Item
      1. Nested first
      2. Nested second

    3. Third

    | A | B |
    |---|---|
    | 1 | 2 |

    :::note
    Note text.
    :::

    Term
    : Definition.

    ***

    ```mermaid
    graph TD
    ```
",
    )
    .unwrap()("word/footnotes.xml");
    let notes = footnotes(&xml);
    assert_eq!(notes.len(), 1, "{xml}");
    assert_eq!(
        paragraphs(notes[0].1),
        [
            "[FootnoteText]  Heading",
            "[FootnoteText] Quote.",
            "[FootnoteText] • Item",
            "[FootnoteText] 1. Nested first",
            "[FootnoteText] 2. Nested second",
            "[FootnoteText] 3. Third",
            "[FootnoteText] A\tB",
            "[FootnoteText] 1\t2",
            "[FootnoteText] Note",
            "[FootnoteText] Note text.",
            "[FootnoteText] Term",
            "[FootnoteText] Definition.",
        ]
    );
    assert!(!xml.contains("<w:bookmarkStart"), "{xml}");
}

#[test]
fn heading_in_footnote_takes_no_toc_entry() {
    let document = render_parts(
        "\
# One

Text[^a].

[^a]:
    ## Note heading

    Note text.

## Two
",
    )
    .unwrap()("word/document.xml");
    let sdt = between(&document, "<w:sdt>", "</w:sdt>");
    assert!(!sdt.contains("Note heading"), "{sdt}");
    assert_in_order(sdt, &[">One<", ">Two<"]);
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
    assert_eq!(anchors.len(), 2);
    assert_eq!(anchors, bookmarks);
}

#[test]
fn missing_definition_is_an_error() {
    let err = render_with("Text[^a].\n\n[^a]: Note.\n", &[], |prepared| {
        let root = prepared.graph().root_id;
        prepared
            .graph_mut()
            .document_mut(root)
            .footnote_definitions
            .clear();
    })
    .err()
    .unwrap();
    assert_eq!(err.to_string(), "missing footnote definition for `a`");
}

#[test]
fn docx_rs_reference_run_is_unchanged() {
    let footnote = Footnote {
        id: 7,
        content: Vec::new(),
    };
    let run = Run::new().add_footnote_reference(footnote).build();
    assert_eq!(String::from_utf8(run).unwrap(), reference(7));
}

#[test]
fn footnotes_and_settings_are_well_formed() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // Skip if xmllint is not on PATH.
    if which::which("xmllint").is_err() {
        eprintln!("skipping footnotes_and_settings_are_well_formed: xmllint not found on PATH");
        return;
    }
    let part = render_parts(
        "\
Text[^a] & more[^b].

[^a]: Note with `code <x>`, a [link](https://example.com) & **strong** text.

[^b]:
    First.

    ```
    a < b && c
    ```
",
    )
    .unwrap();
    for name in [
        "word/footnotes.xml",
        "word/settings.xml",
        "word/document.xml",
    ] {
        let mut child = Command::new("xmllint")
            .args(["--noout", "-"])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(part(name).as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The `Hyperlink` run with the text `text`.
fn hyperlink_run(text: &str) -> String {
    format!(
        r#"<w:r><w:rPr><w:rStyle w:val="Hyperlink" /></w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>"#
    )
}

#[test]
fn reference_in_link_text_gets_footnote() {
    let part =
        render_parts("A [link with note[^lk]](https://x.org) end.\n\n[^lk]: Link note.\n").unwrap();
    let document = part("word/document.xml");
    assert_in_order(
        &document,
        &[&hyperlink_run("link with note"), &reference(1), " end."],
    );
    let xml = part("word/footnotes.xml");
    let notes = footnotes(&xml);
    assert_eq!(notes.len(), 1, "{xml}");
    assert_eq!(paragraphs(notes[0].1), ["[FootnoteText]  Link note."]);
}

#[test]
fn link_text_is_one_hyperlink_run() {
    let document = render_parts("[a *b* `c` **d**](https://x.org)\n").unwrap()("word/document.xml");
    let paragraph = format!("<w:pPr><w:rPr /></w:pPr>{}</w:p>", hyperlink_run("a b c d"));
    assert!(document.contains(&paragraph), "{document}");
}

#[test]
fn reference_in_admonition_title_gets_footnote() {
    let part = render_parts(
        "\
:::note
## Title note[^at]
Body.
:::

[^at]: Admonition title note.
",
    )
    .unwrap();
    let document = part("word/document.xml");
    let title = between(
        &document,
        r#"<w:pStyle w:val="AdmonitionNoteHeader" />"#,
        "</w:p>",
    );
    assert_in_order(
        title,
        &[
            r#"<w:t xml:space="preserve">Title note</w:t>"#,
            &reference(1),
        ],
    );
    let xml = part("word/footnotes.xml");
    let notes = footnotes(&xml);
    assert_eq!(notes.len(), 1, "{xml}");
    assert_eq!(
        paragraphs(notes[0].1),
        ["[FootnoteText]  Admonition title note."]
    );
}

#[test]
fn reference_in_admonition_title_in_footnote_gives_nothing() {
    let xml = render_parts(
        "\
Text[^a].

[^a]:
    :::note
    ## Title note[^b] and [link[^b]](https://x.org)
    Body.
    :::

[^b]: Nested note.
",
    )
    .unwrap()("word/footnotes.xml");
    let notes = footnotes(&xml);
    assert_eq!(notes.len(), 1, "{xml}");
    assert_eq!(
        paragraphs(notes[0].1),
        [
            "[FootnoteText]  Title note and link",
            "[FootnoteText] Body."
        ]
    );
    assert!(!xml.contains("footnoteReference"), "{xml}");
}

#[test]
fn missing_definition_in_link_or_admonition_title_is_an_error() {
    for markdown in [
        "[link[^a]](https://x.org)\n\n[^a]: Note.\n",
        ":::note\n## Title[^a]\nBody.\n:::\n\n[^a]: Note.\n",
    ] {
        let err = render_with(markdown, &[], |prepared| {
            let root = prepared.graph().root_id;
            prepared
                .graph_mut()
                .document_mut(root)
                .footnote_definitions
                .clear();
        })
        .err()
        .unwrap_or_else(|| panic!("no error for {markdown:?}"));
        assert_eq!(err.to_string(), "missing footnote definition for `a`");
    }
}

#[test]
fn task_list_in_footnote_has_task_boxes() {
    let xml = render_parts(
        "\
Text[^a].

[^a]:
    - [x] Done
    - [ ] Open
    - Plain
",
    )
    .unwrap()("word/footnotes.xml");
    let notes = footnotes(&xml);
    assert_eq!(notes.len(), 1, "{xml}");
    assert_eq!(
        paragraphs(notes[0].1),
        [
            "[FootnoteText]  ☒ Done",
            "[FootnoteText] ☐ Open",
            "[FootnoteText] • Plain",
        ]
    );
}
