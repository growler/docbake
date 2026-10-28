//! Lint of the rendered body.
//!
//! The lint renders one Markdown fixture for each group of elements and
//! does two checks on the parts:
//!
//! - Each style reference has a style definition in `word/styles.xml`.
//! - The body does not set a property that a style must carry. The lint
//!   does not check the groups in [`ALLOWED_GROUPS`]. The groups in
//!   [`MARKUP_GROUPS`] can set the properties from
//!   [`MARKUP_PROPERTIES`].
//!
//! A document without title-page front matter starts with the TOC and the
//! page break after it. These blocks are in the Paragraphs group in all
//! fixtures.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::ops::Range;
use std::sync::LazyLock;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::render_package;

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
];

/// The properties from [`STYLE_PROPERTIES`] that the body can set in all
/// groups.
const ALWAYS_ALLOWED: &[&str] = &[];

/// The groups that can set the properties from [`STYLE_PROPERTIES`].
const ALLOWED_GROUPS: &[Group] = &[Group::Lists];

/// The properties from [`STYLE_PROPERTIES`] that Markdown inline markup
/// sets on one run: bold, italic and strikethrough.
const MARKUP_PROPERTIES: &[&str] = &["w:b", "w:i", "w:strike"];

/// The groups with Markdown inline markup in the fixture. These groups can
/// set the properties from [`MARKUP_PROPERTIES`]. The Blocks fixture has
/// bold text in an admonition body.
const MARKUP_GROUPS: &[Group] = &[Group::Code, Group::Blocks];

/// The style IDs that a group can refer to without a style definition.
/// The IDs must match exactly.
const UNDEFINED_STYLES: &[(Group, &str)] = &[];

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

- [ ] Open task
- [x] Done task
";

const FIXTURES: &[(Group, &str)] = &[
    (Group::Paragraphs, PARAGRAPHS),
    (Group::Code, CODE),
    (Group::Blocks, BLOCKS),
    (Group::Tables, TABLES),
    (Group::Lists, LISTS),
];

/// An element from [`STYLE_PROPERTIES`] or [`STYLE_REFERENCES`].
#[derive(Debug, PartialEq)]
struct Hit {
    /// The position of the top-level body child that holds the element.
    /// It is `None` for an element out of `w:body`.
    block: Option<usize>,
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
            let block = body.map(|i| stack.get(i + 1).map_or(position, |(_, p)| *p));
            result.hits.push(Hit {
                block,
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

/// Gives the text of the top-level body children `blocks` of `xml`
/// without the values of `w14:paraId`. These values change from one
/// render to the next.
fn blocks_text(xml: &[u8], blocks: &[Range<usize>]) -> String {
    let (Some(first), Some(last)) = (blocks.first(), blocks.last()) else {
        return String::new();
    };
    let text = std::str::from_utf8(&xml[first.start..last.end]).unwrap();
    let mut pieces = text.split("w14:paraId=\"");
    let mut result = pieces.next().unwrap().to_string();
    for piece in pieces {
        result.push_str("w14:paraId=\"");
        result.push_str(&piece[piece.find('"').unwrap()..]);
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

/// Gives the group of a hit in the fixture of `fixture`. The first
/// `prefix` top-level body children and the elements out of `w:body` are
/// in the Paragraphs group.
fn group_of(fixture: Group, block: Option<usize>, prefix: usize) -> Group {
    match block {
        Some(block) if block >= prefix => fixture,
        _ => Group::Paragraphs,
    }
}

/// The scan of one part of a rendered fixture.
struct Part {
    group: Group,
    name: String,
    scan: Scan,
    /// The text of the first top-level body children, as many as in
    /// [`BASELINE`]. See [`blocks_text`].
    prefix: String,
}

/// The rendered fixtures.
struct Rendered {
    /// The number of top-level body children in [`BASELINE`].
    prefix: usize,
    /// The text of the top-level body children in [`BASELINE`]. See
    /// [`blocks_text`].
    baseline: String,
    /// The style IDs in `word/styles.xml`.
    style_ids: HashSet<String>,
    /// `word/document.xml` of each fixture, and the header and footer
    /// parts of the Paragraphs fixture.
    parts: Vec<Part>,
}

/// Renders the fixtures one time for all tests.
static RENDERED: LazyLock<Rendered> = LazyLock::new(|| {
    let package = render_package(BASELINE).unwrap();
    let xml = package.xml();
    let blocks = scan(&xml.document).blocks;
    let prefix = blocks.len();
    let baseline = blocks_text(&xml.document, &blocks);
    // The style sheet depends on `lang`. All fixtures must use the same
    // `lang` as the baseline.
    let style_ids = style_ids(&xml.styles);
    let mut parts = Vec::new();
    for &(group, markdown) in FIXTURES {
        let package = render_package(markdown).unwrap();
        let xml = package.xml();
        let document = scan(&xml.document);
        let blocks = &document.blocks[..prefix.min(document.blocks.len())];
        parts.push(Part {
            group,
            name: "word/document.xml".to_string(),
            prefix: blocks_text(&xml.document, blocks),
            scan: document,
        });
        if group != Group::Paragraphs {
            continue;
        }
        for (kind, list) in [("header", &xml.headers), ("footer", &xml.footers)] {
            for (i, part) in list.iter().enumerate() {
                parts.push(Part {
                    group,
                    name: format!("word/{kind}{}.xml", i + 1),
                    scan: scan(part),
                    prefix: String::new(),
                });
            }
        }
    }
    Rendered {
        prefix,
        baseline,
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
            let group = group_of(part.group, hit.block, rendered.prefix);
            if let Some(text) = check(group, hit) {
                findings
                    .entry(group)
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
fn fixtures_start_with_baseline_blocks() {
    let rendered = &*RENDERED;
    assert!(rendered.prefix > 0);
    for part in &rendered.parts {
        if part.group != Group::Paragraphs {
            assert_eq!(part.prefix, rendered.baseline, "{:?}", part.group);
            assert!(part.scan.blocks.len() > rendered.prefix, "{:?}", part.group);
        }
    }
}

#[test]
fn style_references_are_defined() {
    let rendered = &*RENDERED;
    let findings = findings(rendered, |group, hit| {
        let id = hit.val.as_deref()?;
        if rendered.style_ids.contains(id) || UNDEFINED_STYLES.contains(&(group, id)) {
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
fn body_has_no_direct_formatting() {
    let findings = findings(&RENDERED, |group, hit| {
        if hit.val.is_some()
            || ALWAYS_ALLOWED.contains(&hit.name)
            || ALLOWED_GROUPS.contains(&group)
            || (MARKUP_GROUPS.contains(&group) && MARKUP_PROPERTIES.contains(&hit.name))
        {
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
            block: Some(0),
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
    let refs: Vec<_> = hits.iter().map(|h| (h.block, h.val.as_deref())).collect();
    assert_eq!(refs, [(None, Some("Header")), (None, Some("Strong"))]);
}

#[test]
fn blocks_text_removes_paragraph_ids() {
    let xml = br#"<w:document><w:body><w:p w14:paraId="00000001"><w:r/></w:p><w:p w14:paraId="0000000a"/><w:sectPr/></w:body></w:document>"#;
    let blocks = scan(xml).blocks;
    assert_eq!(blocks.len(), 2);
    assert_eq!(
        blocks_text(xml, &blocks),
        r#"<w:p w14:paraId=""><w:r/></w:p><w:p w14:paraId=""/>"#
    );
}

#[test]
fn prefix_blocks_are_paragraphs() {
    assert_eq!(group_of(Group::Code, Some(0), 2), Group::Paragraphs);
    assert_eq!(group_of(Group::Code, Some(1), 2), Group::Paragraphs);
    assert_eq!(group_of(Group::Code, Some(2), 2), Group::Code);
    assert_eq!(group_of(Group::Tables, None, 2), Group::Paragraphs);
    assert_eq!(group_of(Group::Paragraphs, Some(5), 2), Group::Paragraphs);
}
