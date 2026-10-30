//! Tests of the links: `w:hyperlink` in `word/document.xml` and
//! `word/footnotes.xml`, the bookmarks of headings and captions, and the
//! hyperlink relationships.

use std::sync::LazyLock;

use regex::Regex;

use crate::docx::links::bookmark_name;

use super::{
    between, entry_names, parts, png, render_package, render_parts, render_with, PARAGRAPH,
};

/// A `w:hyperlink` element.
static HYPERLINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<w:hyperlink .*?</w:hyperlink>").unwrap());

/// The relationship ID and the target of a hyperlink relationship.
static RELATIONSHIP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"<Relationship Id="([^"]+)" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="([^"]*)" TargetMode="External"/>"#,
    )
    .unwrap()
});

/// The text of a run.
static TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<w:t(?: [^>]*)?>([^<]*)</w:t>").unwrap());

/// Gives the `w:hyperlink` elements of `xml`.
fn hyperlinks(xml: &str) -> Vec<&str> {
    HYPERLINK.find_iter(xml).map(|m| m.as_str()).collect()
}

/// Gives the text of `xml`.
fn text(xml: &str) -> String {
    TEXT.captures_iter(xml).map(|c| c[1].to_string()).collect()
}

/// Gives the ID and the target of each hyperlink relationship of `rels`.
fn relationships(rels: &str) -> Vec<(String, String)> {
    RELATIONSHIP
        .captures_iter(rels)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect()
}

/// The start of a link to the relationship `rid`.
fn external(rid: &str) -> String {
    format!(r#"<w:hyperlink r:id="{rid}" w:history="1">"#)
}

/// The start of a link to the heading or caption ID `id`.
fn internal(id: &str) -> String {
    format!(
        r#"<w:hyperlink w:anchor="{}" w:history="1">"#,
        bookmark_name(id)
    )
}

/// Asserts that `document` has one bookmark of the ID `id`.
#[track_caller]
fn assert_bookmark(document: &str, id: &str) {
    let name = format!(r#" w:name="{}" />"#, bookmark_name(id));
    assert_eq!(document.matches(&name).count(), 1, "{id}: {document}");
}

/// Gives the paragraph of `document` that contains `part`.
#[track_caller]
fn paragraph_with<'d>(document: &'d str, part: &str) -> &'d str {
    PARAGRAPH
        .find_iter(document)
        .map(|m| m.as_str())
        .find(|p| p.contains(part))
        .unwrap_or_else(|| panic!("no paragraph with {part} in {document}"))
}

#[test]
fn external_links_have_relationships() {
    let part = render_parts(
        "[web](https://example.com/a), [mail](mailto:a@example.com) and \
         <https://example.com/b>.\n",
    )
    .unwrap();
    let document = part("word/document.xml");
    let links = hyperlinks(&document);
    let texts: Vec<_> = links.iter().map(|l| text(l)).collect();
    assert_eq!(texts, ["web", "mail", "https://example.com/b"]);
    for (i, link) in links.iter().enumerate() {
        assert!(
            link.starts_with(&external(&format!("rIdLink{}", i + 1))),
            "{link}"
        );
    }
    let rels = part("word/_rels/document.xml.rels");
    assert_eq!(
        relationships(&rels),
        [
            ("rIdLink1", "https://example.com/a"),
            ("rIdLink2", "mailto:a@example.com"),
            ("rIdLink3", "https://example.com/b"),
        ]
        .map(|(id, target)| (id.to_string(), target.to_string()))
    );
    assert!(!rels.contains("rIdHyperlink"), "{rels}");
}

#[test]
fn equal_urls_share_one_relationship() {
    let part =
        render_parts("[a](https://x.org), [b](https://y.org) and [c](https://x.org).\n").unwrap();
    let document = part("word/document.xml");
    let starts: Vec<_> = hyperlinks(&document)
        .iter()
        .map(|l| &l[..l.find('>').unwrap() + 1])
        .collect();
    assert_eq!(
        starts,
        [
            external("rIdLink1"),
            external("rIdLink2"),
            external("rIdLink1")
        ]
    );
    let rels = part("word/_rels/document.xml.rels");
    assert_eq!(relationships(&rels).len(), 2, "{rels}");
}

#[test]
fn relationship_order_does_not_change() {
    let markdown: String = (0..20)
        .map(|i| format!("[{i}](https://example.com/{i})\n\n"))
        .collect();
    let rels = |_| render_parts(&markdown).unwrap()("word/_rels/document.xml.rels");
    let first = rels(());
    let targets: Vec<_> = relationships(&first).into_iter().map(|(_, t)| t).collect();
    let expected: Vec<_> = (0..20)
        .map(|i| format!("https://example.com/{i}"))
        .collect();
    assert_eq!(targets, expected);
    for _ in 0..3 {
        assert_eq!(rels(()), first);
    }
}

#[test]
fn url_is_escaped_in_relationship() {
    let part = render_parts("[q](<https://x.org/?a=1&b=\"2\">)\n").unwrap();
    let rels = part("word/_rels/document.xml.rels");
    assert_eq!(
        relationships(&rels),
        [(
            "rIdLink1".to_string(),
            "https://x.org/?a=1&amp;b=%222%22".to_string()
        )]
    );
    assert_well_formed("word/_rels/document.xml.rels", &rels);
}

/// The URLs are the same as the `href` values of the HTML output.
#[test]
fn urls_are_decoded_and_percent_encoded() {
    let part = render_parts(
        "[amp](https://x.org/?a=1&amp;b=2) [bs](https://x.org/a\\_b) \
         [sp](<https://x.org/c d>) [same](https://x.org/a_b) \
         <https://x.org/?a&amp;b>\n",
    )
    .unwrap();
    let document = part("word/document.xml");
    let starts: Vec<_> = hyperlinks(&document)
        .iter()
        .map(|l| &l[..l.find('>').unwrap() + 1])
        .collect();
    assert_eq!(
        starts,
        ["rIdLink1", "rIdLink2", "rIdLink3", "rIdLink2", "rIdLink4"].map(external)
    );
    assert_eq!(
        relationships(&part("word/_rels/document.xml.rels")),
        [
            ("rIdLink1", "https://x.org/?a=1&amp;b=2"),
            ("rIdLink2", "https://x.org/a_b"),
            ("rIdLink3", "https://x.org/c%20d"),
            ("rIdLink4", "https://x.org/?a&amp;amp%3Bb"),
        ]
        .map(|(id, target)| (id.to_string(), target.to_string()))
    );
}

#[test]
fn relative_link_with_url_in_query_is_plain_text() {
    let part =
        render_parts("[rel](docs/page.html?u=https://y.org) and [ftp](ftp://h/f).\n").unwrap();
    let document = part("word/document.xml");
    let links = hyperlinks(&document);
    assert_eq!(links.iter().map(|l| text(l)).collect::<Vec<_>>(), ["ftp"]);
    assert_eq!(
        relationships(&part("word/_rels/document.xml.rels")),
        [("rIdLink1".to_string(), "ftp://h/f".to_string())]
    );
    assert_eq!(
        text(paragraph_with(&document, ">rel</w:t>")),
        "rel and ftp."
    );
}

#[test]
fn links_without_text_have_no_relationship() {
    let package = render_with(
        "[![pic](pic.png)](https://only-img.org) and [](https://empty.org).\n",
        &[("pic.png", png(4, 2))],
        |_| {},
    )
    .unwrap();
    let part = parts(package.into_bytes().unwrap());
    let document = part("word/document.xml");
    assert!(hyperlinks(&document).is_empty(), "{document}");
    assert!(document.contains("<w:drawing"), "{document}");
    let rels = part("word/_rels/document.xml.rels");
    assert!(!rels.contains("rIdLink"), "{rels}");
    assert!(!rels.contains("/hyperlink\""), "{rels}");
}

#[test]
fn links_in_admonition_title_are_clickable() {
    let part = render_parts(
        "\
# Top

:::note
## See [the *docs*](https://x.org/docs) and [top](#top)
Body.
:::

:::tip
## A *plain* `title`
Body.
:::
",
    )
    .unwrap();
    let document = part("word/document.xml");
    let title = paragraph_with(&document, r#"<w:pStyle w:val="AdmonitionNoteHeader" />"#);
    assert_eq!(text(title), "See the docs and top");
    let links = hyperlinks(title);
    assert_eq!(links.len(), 2, "{title}");
    assert_eq!(
        links[0],
        [
            external("rIdLink1").as_str(),
            r#"<w:r><w:rPr><w:rStyle w:val="Hyperlink" /></w:rPr><w:t xml:space="preserve">the docs</w:t></w:r>"#,
            "</w:hyperlink>",
        ]
        .concat()
    );
    assert!(links[1].starts_with(&internal("top")), "{title}");
    assert_eq!(
        relationships(&part("word/_rels/document.xml.rels")),
        [("rIdLink1".to_string(), "https://x.org/docs".to_string())]
    );
    let plain = paragraph_with(&document, r#"<w:pStyle w:val="AdmonitionTipHeader" />"#);
    assert!(
        plain.ends_with(
            r#"<w:r><w:rPr /><w:t xml:space="preserve">A plain title</w:t></w:r></w:p>"#
        ),
        "{plain}"
    );
}

#[test]
fn internal_links_go_to_heading_and_caption_bookmarks() {
    let document = render_parts(
        "\
---
lang: en
---
See [the end](#end), [deep](#deep) and [the table](#values).

# Start

##### Deep

Table: Values

| A |
|---|
| 1 |

# End

Back to [start](#start), [deep](#deep) and [the table](#values).
",
    )
    .unwrap()("word/document.xml");
    for id in ["start", "deep", "values", "end"] {
        assert_bookmark(&document, id);
    }
    let body = &document[document.find("</w:sdt>").unwrap()..];
    let starts: Vec<_> = hyperlinks(body)
        .iter()
        .map(|l| &l[..l.find('>').unwrap() + 1])
        .collect();
    assert_eq!(
        starts,
        ["end", "deep", "values", "start", "deep", "values"].map(internal)
    );
    let caption = paragraph_with(&document, r#"<w:pStyle w:val="Caption" />"#);
    assert!(
        caption.contains(&format!(r#"w:name="{}""#, bookmark_name("values"))),
        "{caption}"
    );
}

#[test]
fn links_to_other_ids_are_plain_text() {
    let package = render_with(
        "\
Links: [code](#example-code), [div](#box), [image](#pic), [note](#note-heading),
[hidden](#hidden), [missing](#missing), [file](data.txt) and [empty]().

```rust Example code
fn main() {}
```

::: {#box}
Box text.
:::

![Picture](image.png){#pic}

::: {if-format=\"html\"}
## Hidden
:::

Text[^n].

[^n]:
    ## Note heading

    Note text.
",
        &[("image.png", png(4, 2)), ("data.txt", b"data".to_vec())],
        |_| {},
    )
    .unwrap();
    let part = parts(package.into_bytes().unwrap());
    let document = part("word/document.xml");
    assert!(hyperlinks(&document).is_empty(), "{document}");
    assert!(!document.contains("Hyperlink"), "{document}");
    let links = paragraph_with(&document, ">Links: </w:t>");
    assert_eq!(
        text(links),
        "Links: code, div, image, note, hidden, missing, file and empty."
    );
    assert!(!part("word/footnotes.xml").contains("w:hyperlink"));
    assert!(
        !document.contains(&bookmark_name("note-heading")),
        "{document}"
    );
    assert!(!document.contains(&bookmark_name("hidden")), "{document}");
}

#[test]
fn emphasis_around_link_formats_link_runs() {
    let document = render_parts("*[a **b**](https://x.org)* and ~~[c](#c)~~\n\n# C\n").unwrap()(
        "word/document.xml",
    );
    let body = &document[document.find("</w:sdt>").unwrap()..];
    let links = hyperlinks(body);
    assert_eq!(links.len(), 2, "{body}");
    let run = |properties: &str, text: &str| {
        format!(
            r#"<w:r><w:rPr>{properties}<w:rStyle w:val="Hyperlink" /></w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>"#
        )
    };
    assert_eq!(
        links[0],
        [
            external("rIdLink1"),
            run("<w:i /><w:iCs />", "a "),
            run("<w:b /><w:bCs /><w:i /><w:iCs />", "b"),
            "</w:hyperlink>".to_string(),
        ]
        .concat()
    );
    assert_eq!(
        links[1],
        [
            internal("c"),
            run("<w:strike />", "c"),
            "</w:hyperlink>".to_string()
        ]
        .concat()
    );
}

#[test]
fn picture_in_link_is_outside_the_hyperlink() {
    let package = render_with(
        "[before ![Alt](image.png) after](https://x.org)\n",
        &[("image.png", png(4, 2))],
        |_| {},
    )
    .unwrap();
    let bytes = package.into_bytes().unwrap();
    assert!(
        entry_names(&bytes)
            .iter()
            .any(|n| n.starts_with("word/media/")),
        "{:?}",
        entry_names(&bytes)
    );
    let part = parts(bytes);
    let document = part("word/document.xml");
    let links = hyperlinks(&document);
    let texts: Vec<_> = links.iter().map(|l| text(l)).collect();
    assert_eq!(texts, ["before ", " after"]);
    assert!(links.iter().all(|l| !l.contains("<w:drawing")), "{links:?}");
    let paragraph = paragraph_with(&document, "<w:drawing");
    let picture = paragraph.find("<w:drawing").unwrap();
    assert!(
        paragraph[..picture].ends_with(r#"</w:hyperlink><w:r><w:rPr />"#),
        "{paragraph}"
    );
    let rid = Regex::new(r#"r:embed="([^"]+)""#).unwrap();
    let rid = &rid.captures(paragraph).unwrap()[1];
    let rels = part("word/_rels/document.xml.rels");
    assert!(rels.contains(&format!(r#"Id="{rid}""#)), "{rels}");
}

#[test]
fn links_in_blocks_are_clickable() {
    let document = render_parts(
        "\
---
lang: en
---
| [cell](https://x.org/cell) |
|---|
| 1 |

- [item](https://x.org/item)

[term](https://x.org/term)
: Definition with [body](https://x.org/body).

Table: Caption with [link](https://x.org/caption)

| A |
|---|
| 1 |

:::note
Admonition with [link](https://x.org/note).
:::
",
    )
    .unwrap()("word/document.xml");
    for (style, text) in [
        ("TableHeader", "cell"),
        ("ListParagraph", "item"),
        ("DefinitionTerm", "term"),
        ("DefinitionBody", "body"),
        ("Caption", "link"),
        ("AdmonitionNoteBody", "link"),
    ] {
        let style = format!(r#"<w:pStyle w:val="{style}""#);
        let paragraph = paragraph_with(&document, &style);
        let links = hyperlinks(paragraph);
        assert_eq!(links.len(), 1, "{paragraph}");
        assert!(links[0].contains(&format!(">{text}</w:t>")), "{paragraph}");
    }
}

#[test]
fn footnote_links_are_clickable() {
    let part = render_parts(
        "\
---
lang: en
---
# Target

Text[^a] with [a body link](https://x.org/body).

[^a]: See [the target](#target), [the site](https://x.org/note) and
    [again](https://x.org/note).
",
    )
    .unwrap();
    let xml = part("word/footnotes.xml");
    let links = hyperlinks(&xml);
    assert_eq!(links.len(), 3, "{xml}");
    assert!(links[0].starts_with(&internal("target")), "{xml}");
    assert!(links[1].starts_with(&external("rIdLink1")), "{xml}");
    assert!(links[2].starts_with(&external("rIdLink1")), "{xml}");
    let rels = part("word/_rels/footnotes.xml.rels");
    assert_eq!(
        relationships(&rels),
        [("rIdLink1".to_string(), "https://x.org/note".to_string())]
    );
    let document_rels = part("word/_rels/document.xml.rels");
    assert_eq!(
        relationships(&document_rels),
        [("rIdLink1".to_string(), "https://x.org/body".to_string())]
    );
    assert_well_formed("word/footnotes.xml", &xml);
    assert_well_formed("word/_rels/footnotes.xml.rels", &rels);
}

#[test]
fn footnotes_without_external_links_have_no_relationships_part() {
    let package = render_package("Text[^a].\n\n[^a]: See [x](#x).\n\n# X\n").unwrap();
    let names = entry_names(&package.into_bytes().unwrap());
    assert!(
        names.contains(&"word/footnotes.xml".to_string()),
        "{names:?}"
    );
    assert!(
        !names.contains(&"word/_rels/footnotes.xml.rels".to_string()),
        "{names:?}"
    );
}

#[test]
fn toc_and_heading_use_one_bookmark() {
    let document = render_parts("# One\n\nSee [one](#one).\n").unwrap()("word/document.xml");
    assert_bookmark(&document, "one");
    let toc = between(&document, "<w:sdt>", "</w:sdt>");
    assert!(toc.contains(&internal("one")), "{toc}");
    let body = &document[document.find("</w:sdt>").unwrap()..];
    assert_eq!(hyperlinks(body).len(), 1, "{body}");
    assert!(body.contains(&internal("one")), "{body}");
}

/// Asserts that `xmllint` accepts the part `name` with the text `xml`
/// with no message. A namespace error gives a message and exit status 0.
/// Does nothing if `xmllint` is not on PATH.
#[track_caller]
fn assert_well_formed(name: &str, xml: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if which::which("xmllint").is_err() {
        eprintln!("skipping the xmllint check of {name}: xmllint not found on PATH");
        return;
    }
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
        .write_all(xml.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
