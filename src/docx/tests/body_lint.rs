//! Lint of the rendered body.
//!
//! The lint renders one Markdown fixture for each group of elements and
//! does two checks on the parts:
//!
//! - Each style reference has a style definition in `word/styles.xml`, and
//!   its ID is in [`REQUIRED_STYLES`] or in [`RESERVED_STYLES`].
//! - The body does not set a property that a style must carry, except the
//!   properties that [`permitted`] permits for the group.
//!
//! A document without title-page front matter and without headings has an
//! empty body. Thus all elements of a fixture are in the group of the
//! fixture. The lint also scans `word/footnotes.xml` of each fixture, and
//! the header and footer parts of the Paragraphs fixture. The Images
//! fixture has the PNG file `image.png` in its directory.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::ops::Range;
use std::sync::LazyLock;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::{png, render_package, render_with};
use crate::docx::params::REQUIRED_STYLES;
use crate::docx::stylesheet::RESERVED_STYLES;

/// A group of elements that one part of the renderer makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    /// Paragraphs, headings, title page, header, footer and TOC.
    Paragraphs,
    /// Code blocks, inline code, links and inline emphasis.
    Code,
    /// Admonitions, block quotes, definition lists and thematic breaks.
    Blocks,
    /// Tables.
    Tables,
    /// Lists and task lists.
    Lists,
    /// Captions.
    Captions,
    /// Diagrams.
    Diagrams,
    /// Footnotes.
    Footnotes,
    /// Images.
    Images,
}

/// The properties that a style must carry. The names must match exactly,
/// thus `w:bCs`, `w:iCs` and `w:szCs` are permitted.
const STYLE_PROPERTIES: &[&str] = &[
    "w:rFonts",
    "w:sz",
    "w:color",
    "w:shd",
    "w:pBdr",
    "w:b",
    "w:i",
    "w:strike",
    "w:u",
    "w:spacing",
    "w:vertAlign",
    "w:keepNext",
    "w:jc",
    "w:ind",
];

/// Tells if a fixture of the group `group` can set the property of `hit`.
///
/// Markdown inline markup sets bold, italic and strikethrough on one run.
/// The groups with inline markup in the fixture can set `w:b`, `w:i` and
/// `w:strike`. The Blocks fixture has bold text in an admonition body. The
/// Footnotes fixture has bold text in a footnote. A paragraph in a table
/// cell can set the alignment `w:jc` of its column.
fn permitted(group: Group, hit: &Hit) -> bool {
    match group {
        Group::Code | Group::Blocks | Group::Footnotes => {
            matches!(hit.name, "w:b" | "w:i" | "w:strike")
        }
        Group::Tables => hit.name == "w:jc" && hit.path.contains("/w:tc["),
        _ => false,
    }
}

/// The elements that refer to a style with the attribute `w:val`.
const STYLE_REFERENCES: &[&str] = &["w:pStyle", "w:rStyle", "w:tblStyle"];

/// A document with minimal front matter and no content.
const BASELINE: &str = "\
---
lang: en
---
";

const PARAGRAPHS: &str = "\
---
title: Title
subtitle: Subtitle
author: Author
version: 1.0
date: 2026-01-01
lang: en
---
# Heading 1

## Heading 2

### Heading 3

#### Heading 4

##### Heading 5

###### Heading 6

Text with a soft
line break and a hard\\
line break.
";

const CODE: &str = "\
---
lang: en
---
```
fn main() {}
```

Text with `code`, a [link](https://example.com), *em*, **strong** and
~~strike~~.

A [link with *em*](https://example.com).
";

const BLOCKS: &str = "\
---
lang: en
---
:::tip
## Admonition title
Admonition text with **strong** text.
:::

:::note
Admonition text without a title.

```
code
```

- Item

Admonition text after a list.
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

:::related
Related text.
:::

> Block quote.
>
> > Nested block quote.

Term
Second term
: Definition.

  Second definition paragraph.

***
";

const TABLES: &str = "\
---
lang: en
---
| Name | Value |
|------|------:|
| one  | 1     |
| two  | 2     |

+------+-------+-------+
| Group                |
+------+-------+-------+
| Left | Mid   | Right |
+:=====+:=====:+======:+
| a    | b     | c     |
+------+-------+-------+

-----  -----
one    1

two    2
-----  -----
";

const LISTS: &str = "\
---
lang: en
---
- Item
  - Nested item
    - Nested item

1. First
2. Second

Text.

3. Third

- Item
  1. Nested ordered item

Text.

- [ ] Open task
- [x] Done task
  - [ ] Nested open task
  - [x] Nested done task
";

const CAPTIONS: &str = "\
---
lang: en
---
Table: Table caption

| Name | Value |
|------|-------|
| one  | 1     |

Figure: Figure caption

:::figure
Figure text.
:::

Listing: Listing caption with `code`

```
fn main() {}
```

:::note
Table: Table caption in an admonition

| Name | Value |
|------|-------|
| two  | 2     |
:::

See [the table](#table-caption).
";

/// The diagram always fails: `panic` stops the Typst compilation, and
/// without `typst` the diagram has no renderer.
const DIAGRAMS: &str = "\
---
lang: en
---
```typst
#panic(\"diagram error\")
```
";

const FOOTNOTES: &str = "\
---
lang: en
---
Text with a note[^a] and a second note[^b].

| Name | Value |
|------|-------|
| one  | 1[^c] |

[^a]: Note with `code`, a [link](https://example.com) and **strong** text.

[^b]:
    First paragraph.

    ```
    code
    ```

    Last paragraph.

[^c]: Table note.
";

const IMAGES: &str = "\
---
lang: en
---
![Block image](image.png)

![Sized image](image.png){width=50% height=2cm}

Text with an ![inline image](image.png) image.

> ![Quoted image](image.png)
";

const FIXTURES: &[(Group, &str)] = &[
    (Group::Paragraphs, PARAGRAPHS),
    (Group::Code, CODE),
    (Group::Blocks, BLOCKS),
    (Group::Tables, TABLES),
    (Group::Lists, LISTS),
    (Group::Captions, CAPTIONS),
    (Group::Diagrams, DIAGRAMS),
    (Group::Footnotes, FOOTNOTES),
    (Group::Images, IMAGES),
];

/// An element from [`STYLE_PROPERTIES`] or [`STYLE_REFERENCES`].
#[derive(Debug, PartialEq)]
struct Hit {
    /// The qualified name of the element.
    name: &'static str,
    /// The value of the attribute `w:val`.
    val: Option<String>,
    /// The element path from the root. The number in brackets is the
    /// position of the element among the element children of its parent,
    /// from 0.
    path: String,
}

/// The result of a scan of one part.
#[derive(Debug, Default)]
struct Scan {
    hits: Vec<Hit>,
    /// The byte ranges of the top-level body children, without
    /// `w:sectPr`.
    blocks: Vec<Range<usize>>,
}

/// Finds the elements from [`STYLE_PROPERTIES`] and [`STYLE_REFERENCES`]
/// and the top-level body children in the XML part `xml`.
fn scan(xml: &[u8]) -> Scan {
    let mut reader = Reader::from_reader(xml);
    let mut result = Scan::default();
    // The open elements: the qualified name and the position.
    let mut stack: Vec<(String, usize)> = Vec::new();
    // The number of element children of each open element, and of the
    // document.
    let mut counts = vec![0usize];
    // The start of the open top-level body child.
    let mut block_start = None;
    // The position of `w:body` in `stack` while `w:body` is open.
    let mut body = None;
    loop {
        let before = reader.buffer_position() as usize;
        let (start, open) = match reader.read_event().unwrap() {
            Event::Start(e) => (e, true),
            Event::Empty(e) => (e, false),
            Event::End(_) => {
                if stack.pop().is_some_and(|(n, _)| n == "w:body") {
                    body = None;
                }
                counts.pop();
                if stack.last().is_some_and(|(n, _)| n == "w:body") {
                    if let Some(start) = block_start.take() {
                        result.blocks.push(start..reader.buffer_position() as usize);
                    }
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let name = std::str::from_utf8(start.name().into_inner()).unwrap();
        let count = counts.last_mut().unwrap();
        let position = *count;
        *count += 1;
        if body.is_some_and(|i| i + 1 == stack.len()) && name != "w:sectPr" {
            if open {
                block_start = Some(before);
            } else {
                result
                    .blocks
                    .push(before..reader.buffer_position() as usize);
            }
        }
        let property = STYLE_PROPERTIES.iter().find(|&&n| n == name);
        let reference = STYLE_REFERENCES.iter().find(|&&n| n == name);
        if let Some(&matched) = property.or(reference) {
            result.hits.push(Hit {
                path: path(&stack, name),
                val: reference.is_some().then(|| val(&start)),
                name: matched,
            });
        }
        if open {
            if name == "w:body" {
                body = Some(stack.len());
            }
            stack.push((name.to_string(), position));
            counts.push(0);
        }
    }
    result
}

/// Gives the path of the element `name` below the open elements `stack`.
fn path(stack: &[(String, usize)], name: &str) -> String {
    let mut path = String::new();
    for (i, (n, p)) in stack.iter().enumerate() {
        if i == 0 {
            path.push_str(n);
        } else {
            write!(path, "{n}[{p}]").unwrap();
        }
        path.push('/');
    }
    path.push_str(name);
    path
}

/// Gives the value of the attribute `w:val` of `element`.
fn val(element: &BytesStart<'_>) -> String {
    element
        .try_get_attribute("w:val")
        .unwrap()
        .unwrap_or_else(|| panic!("no w:val in {element:?}"))
        .unescape_value()
        .unwrap()
        .into_owned()
}

/// Gives the IDs of the styles in the style sheet `xml`.
fn style_ids(xml: &[u8]) -> HashSet<String> {
    let mut reader = Reader::from_reader(xml);
    let mut ids = HashSet::new();
    loop {
        match reader.read_event().unwrap() {
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"w:style" => {
                if let Some(id) = e.try_get_attribute("w:styleId").unwrap() {
                    ids.insert(id.unescape_value().unwrap().into_owned());
                }
            }
            Event::Eof => break,
            _ => continue,
        }
    }
    ids
}

/// The scan of one part of a rendered fixture.
struct Part {
    group: Group,
    name: String,
    /// The text of the part.
    text: String,
    scan: Scan,
}

/// The rendered fixtures.
struct Rendered {
    /// The number of top-level body children in [`BASELINE`].
    baseline_blocks: usize,
    /// The style IDs in `word/styles.xml`.
    style_ids: HashSet<String>,
    /// `word/document.xml` and `word/footnotes.xml` of each fixture, and
    /// the header and footer parts of the Paragraphs fixture.
    parts: Vec<Part>,
}

/// Renders the fixtures one time for all tests.
static RENDERED: LazyLock<Rendered> = LazyLock::new(|| {
    let package = render_package(BASELINE).unwrap();
    let xml = package.xml();
    let baseline_blocks = scan(&xml.document).blocks.len();
    // The style sheet depends on `lang`. All fixtures must use the same
    // `lang` as the baseline.
    let style_ids = style_ids(&xml.styles);
    let mut parts = Vec::new();
    for &(group, markdown) in FIXTURES {
        let package = match group {
            Group::Images => render_with(markdown, &[("image.png", png(40, 10))], |_| {}),
            _ => render_package(markdown),
        }
        .unwrap();
        let xml = package.xml();
        parts.push(Part {
            group,
            name: "word/document.xml".to_string(),
            text: String::from_utf8(xml.document.clone()).unwrap(),
            scan: scan(&xml.document),
        });
        parts.push(Part {
            group,
            name: "word/footnotes.xml".to_string(),
            text: String::from_utf8(xml.footnotes.clone()).unwrap(),
            scan: scan(&xml.footnotes),
        });
        if group != Group::Paragraphs {
            continue;
        }
        for (kind, list) in [("header", &xml.headers), ("footer", &xml.footers)] {
            for (i, part) in list.iter().enumerate() {
                parts.push(Part {
                    group,
                    name: format!("word/{kind}{}.xml", i + 1),
                    text: String::from_utf8(part.clone()).unwrap(),
                    scan: scan(part),
                });
            }
        }
    }
    Rendered {
        baseline_blocks,
        style_ids,
        parts,
    }
});

/// Gives the findings of `rendered` as lines grouped by group. `check`
/// gives a finding text for a hit, or `None` when the hit is permitted.
fn findings(
    rendered: &Rendered,
    check: impl Fn(Group, &Hit) -> Option<String>,
) -> BTreeMap<Group, Vec<String>> {
    let mut findings: BTreeMap<Group, Vec<String>> = BTreeMap::new();
    for part in &rendered.parts {
        for hit in &part.scan.hits {
            if let Some(text) = check(part.group, hit) {
                findings
                    .entry(part.group)
                    .or_default()
                    .push(format!("{}: {text}", part.name));
            }
        }
    }
    findings
}

/// Gives `findings` as a failure message.
fn report(findings: &BTreeMap<Group, Vec<String>>) -> String {
    let mut text = String::new();
    for (group, lines) in findings {
        text.push_str(&format!("{group:?}:\n"));
        for line in lines {
            text.push_str(&format!("  {line}\n"));
        }
    }
    text
}

#[test]
fn baseline_body_is_empty() {
    assert_eq!(RENDERED.baseline_blocks, 0);
}

#[test]
fn footnotes_fixture_footnotes_part_has_footnotes() {
    let part = RENDERED
        .parts
        .iter()
        .find(|p| p.group == Group::Footnotes && p.name == "word/footnotes.xml")
        .unwrap();
    assert!(
        part.text.contains(r#"<w:footnote w:id="3">"#),
        "{}",
        part.text
    );
    assert!(part.text.contains(">Table<"), "{}", part.text);
    assert!(!part.scan.hits.is_empty());
}

#[test]
fn images_fixture_has_images() {
    let part = RENDERED
        .parts
        .iter()
        .find(|p| p.group == Group::Images && p.name == "word/document.xml")
        .unwrap();
    assert_eq!(part.text.matches("<w:drawing>").count(), 4, "{}", part.text);
    assert_eq!(
        part.text
            .matches(r#"<w:pStyle w:val="ImageBlock" />"#)
            .count(),
        2,
        "{}",
        part.text
    );
}

#[test]
fn style_references_are_defined() {
    let rendered = &*RENDERED;
    let findings = findings(rendered, |_, hit| {
        let id = hit.val.as_deref()?;
        if rendered.style_ids.contains(id) {
            return None;
        }
        Some(format!("{} '{id}'", hit.path))
    });
    assert!(
        findings.is_empty(),
        "undefined styles:\n{}",
        report(&findings)
    );
}

#[test]
fn style_references_are_required_or_generated() {
    let findings = findings(&RENDERED, |_, hit| {
        let id = hit.val.as_deref()?;
        if REQUIRED_STYLES.contains(&id) || RESERVED_STYLES.contains(&id) {
            return None;
        }
        Some(format!("{} '{id}'", hit.path))
    });
    assert!(
        findings.is_empty(),
        "styles not in REQUIRED_STYLES:\n{}",
        report(&findings)
    );
}

#[test]
fn body_has_no_direct_formatting() {
    let findings = findings(&RENDERED, |group, hit| {
        if hit.val.is_some() || permitted(group, hit) {
            return None;
        }
        Some(hit.path.clone())
    });
    assert!(
        findings.is_empty(),
        "direct formatting:\n{}",
        report(&findings)
    );
}

#[test]
fn scan_matches_exact_names() {
    let xml = br#"<w:document><w:body><w:p><w:pPr><w:rPr><w:bCs/><w:b/><w:iCs/><w:szCs w:val="20"/></w:rPr></w:pPr></w:p><w:sectPr/></w:body></w:document>"#;
    let result = scan(xml);
    assert_eq!(result.blocks.len(), 1);
    assert_eq!(
        result.hits,
        [Hit {
            name: "w:b",
            val: None,
            path: "w:document/w:body[0]/w:p[0]/w:pPr[0]/w:rPr[0]/w:b".to_string(),
        }]
    );
}

#[test]
fn scan_finds_spacing_in_paragraph_and_run_properties() {
    let xml = br#"<w:document><w:body><w:p/><w:p><w:pPr><w:spacing w:after="0"/></w:pPr><w:r><w:rPr><w:spacing w:val="2"/></w:rPr></w:r></w:p></w:body></w:document>"#;
    let paths: Vec<_> = scan(xml).hits.into_iter().map(|h| h.path).collect();
    assert_eq!(
        paths,
        [
            "w:document/w:body[0]/w:p[1]/w:pPr[0]/w:spacing",
            "w:document/w:body[0]/w:p[1]/w:r[1]/w:rPr[0]/w:spacing",
        ]
    );
}

#[test]
fn scan_reads_style_references() {
    let xml = br#"<w:hdr><w:p><w:pPr><w:pStyle w:val="Header"/></w:pPr><w:r><w:rPr><w:rStyle w:val="Strong"/></w:rPr></w:r></w:p></w:hdr>"#;
    let hits = scan(xml).hits;
    let refs: Vec<_> = hits.iter().map(|h| h.val.as_deref()).collect();
    assert_eq!(refs, [Some("Header"), Some("Strong")]);
}

#[test]
fn alignment_is_permitted_only_in_table_cells() {
    let xml = br#"<w:document><w:body><w:p><w:pPr><w:keepNext/><w:jc w:val="center"/></w:pPr></w:p><w:tbl><w:tr><w:tc><w:p><w:pPr><w:jc w:val="right"/></w:pPr></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#;
    let findings: Vec<_> = scan(xml)
        .hits
        .into_iter()
        .filter(|h| !permitted(Group::Tables, h))
        .map(|h| h.path)
        .collect();
    assert_eq!(
        findings,
        [
            "w:document/w:body[0]/w:p[0]/w:pPr[0]/w:keepNext",
            "w:document/w:body[0]/w:p[0]/w:pPr[0]/w:jc",
        ]
    );
}
