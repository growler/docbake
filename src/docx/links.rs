//! Clickable links.
//!
//! - An external link (a target with a URL scheme and `//`, or a `mailto:`
//!   target) is a `w:hyperlink` with a relationship to the URL. The URL is
//!   the same as the `href` of the HTML output. A part has one
//!   relationship for each distinct URL, with the ID `rIdLink{n}` in the
//!   order of first use. The links of the body go to
//!   `word/_rels/document.xml.rels`, the links of the footnotes go to
//!   `word/_rels/footnotes.xml.rels`.
//! - Each visible heading and caption has a bookmark. Its name is `_Id`
//!   and the first 16 hex digits of the BLAKE3 hash of the heading or
//!   caption ID. An internal link to a heading or a caption is a
//!   `w:hyperlink` with `w:anchor`. The TOC entries use the same bookmarks.
//! - A link to a different ID, to an unknown anchor or to a local file,
//!   and a link with no text give the link content with no link and no
//!   `Hyperlink` style.
//!
//! The runs of a link keep their formatting. A run with no character style
//! gets the `Hyperlink` style. A run with a picture is outside the
//! `w:hyperlink`, because docx-rs collects pictures only from the runs of a
//! paragraph.

use std::collections::HashMap;
use std::io::{self, Write};

use anyhow::{anyhow, Result};
use docx_rs::*;
use memchr::memmem;
use quick_xml::events::{BytesDecl, Event};
use quick_xml::Writer;
use rushdown::ast::{KindData, LinkKind, NodeRef};
use rushdown::util::{escape_url, EscapeUrlOptions};

use super::{DocxRenderer, RenderContext};

/// The relationship type of a hyperlink.
const HYPERLINK_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";

/// The end of a relationships part.
const RELATIONSHIPS_END: &str = "</Relationships>";

/// Gives the bookmark name of the heading or caption ID `id`.
pub(super) fn bookmark_name(id: &str) -> String {
    let name = format!("_Id{}", &blake3::hash(id.as_bytes()).to_hex()[..16]);
    debug_assert!(
        name.len() <= 40 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "{name}"
    );
    name
}

/// The URLs of the external links of one part.
#[derive(Default)]
pub(super) struct ExternalLinks {
    /// The number `n` of the relationship ID `rIdLink{n}` of each URL.
    ids: HashMap<String, usize>,
}

impl ExternalLinks {
    /// Gives the relationship ID of `url`.
    fn rid(&mut self, url: &str) -> String {
        let n = match self.ids.get(url) {
            Some(&n) => n,
            None => {
                let n = self.ids.len() + 1;
                self.ids.insert(url.to_string(), n);
                n
            }
        };
        format!("rIdLink{n}")
    }

    /// Tells if there is no URL.
    pub(super) fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Writes a `Relationship` element for each URL, in ID order.
    fn write_to<W: Write>(&self, w: &mut Writer<W>) -> io::Result<()> {
        let mut urls = vec![""; self.ids.len()];
        for (url, &n) in &self.ids {
            urls[n - 1] = url;
        }
        for (i, url) in urls.into_iter().enumerate() {
            w.create_element("Relationship")
                .with_attributes([
                    ("Id", format!("rIdLink{}", i + 1).as_str()),
                    ("Type", HYPERLINK_TYPE),
                    ("Target", url),
                    ("TargetMode", "External"),
                ])
                .write_empty()?;
        }
        Ok(())
    }
}

/// Replaces the hyperlink relationships in the relationships part `rels`
/// with the relationships of `links`. docx-rs writes the hyperlink
/// relationships after all other relationships, in the order of a
/// `HashMap`.
pub(super) fn replace_hyperlink_relationships(
    rels: &mut Vec<u8>,
    links: &ExternalLinks,
) -> Result<()> {
    let end = memmem::find(rels, RELATIONSHIPS_END.as_bytes())
        .ok_or_else(|| anyhow!("no {RELATIONSHIPS_END} in word/_rels/document.xml.rels"))?;
    let cut = match memmem::find(&rels[..end], HYPERLINK_TYPE.as_bytes()) {
        Some(at) => memmem::rfind(&rels[..at], b"<Relationship ")
            .ok_or_else(|| anyhow!("no relationship start before {HYPERLINK_TYPE}"))?,
        None => end,
    };
    rels.truncate(cut);
    links.write_to(&mut Writer::new(&mut *rels))?;
    rels.extend_from_slice(RELATIONSHIPS_END.as_bytes());
    Ok(())
}

/// Gives a relationships part with the relationships of `links`.
pub(super) fn relationships_part(links: &ExternalLinks) -> Vec<u8> {
    let mut w = Writer::new(Vec::new());
    w.write_event(Event::Decl(BytesDecl::new(
        "1.0",
        Some("UTF-8"),
        Some("yes"),
    )))
    .and_then(|()| {
        w.create_element("Relationships")
            .with_attribute((
                "xmlns",
                "http://schemas.openxmlformats.org/package/2006/relationships",
            ))
            .write_inner_content(|w| links.write_to(w))
            .map(|_| ())
    })
    .expect("a write to a Vec does not fail");
    w.into_inner()
}

impl<'a> DocxRenderer<'a> {
    /// Render a link: the children of the link in a `w:hyperlink`, or the
    /// children alone for a link that is not clickable.
    pub(super) fn render_link(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<Vec<ParagraphChild>> {
        let children = self.collect_child_runs(context, node_ref)?;
        Ok(self.render_link_with(context, node_ref, children))
    }

    /// Render the link `node_ref` with the content `children`: `children`
    /// in a `w:hyperlink`, or `children` alone for a link that is not
    /// clickable or that has only pictures.
    pub(super) fn render_link_with(
        &self,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
        children: Vec<ParagraphChild>,
    ) -> Vec<ParagraphChild> {
        if children.iter().all(has_picture) {
            return children;
        }
        let target = context
            .graph
            .link_target(context.doc_id, node_ref)
            .unwrap_or_default()
            .trim()
            .trim_matches(['<', '>']);
        let link = if let Some(id) = target.strip_prefix('#') {
            let name = bookmark_name(id);
            if self
                .link_targets
                .borrow()
                .get(&name)
                .is_none_or(|known| known != id)
            {
                return children;
            }
            HyperlinkData::Anchor { anchor: name }
        } else if is_external(target) {
            let autolink = matches!(
                context.arena[node_ref].kind_data(),
                KindData::Link(link) if matches!(link.link_kind(), LinkKind::Auto(_))
            );
            let mut links = if context.footnote {
                self.footnote_links.borrow_mut()
            } else {
                self.document_links.borrow_mut()
            };
            HyperlinkData::External {
                rid: links.rid(&external_url(target, autolink)),
                // The renderer writes the hyperlink relationships.
                path: String::new(),
            }
        } else {
            return children;
        };
        wrap_link(children, &link)
    }
}

/// Tells if `target` is an external URL: a URL with a scheme and `//`
/// after the colon, or a `mailto:` URL.
fn is_external(target: &str) -> bool {
    let Some((scheme, rest)) = target.split_once(':') else {
        return false;
    };
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
        && (rest.starts_with("//") || scheme.eq_ignore_ascii_case("mailto"))
}

/// Gives the URL of the external link target `target`, the same as the
/// `href` of the HTML output: the backslash escapes and the character
/// references are resolved, except in an autolink, and the URL is
/// percent-encoded.
fn external_url(target: &str, autolink: bool) -> String {
    let url = escape_url(
        target.as_bytes(),
        &EscapeUrlOptions {
            resolves_refs: !autolink,
            ..EscapeUrlOptions::for_url()
        },
    );
    String::from_utf8_lossy(&url).into_owned()
}

/// Tells if `child` is a run with a picture.
fn has_picture(child: &ParagraphChild) -> bool {
    matches!(child, ParagraphChild::Run(run)
        if run.children.iter().any(|c| matches!(c, RunChild::Drawing(_))))
}

/// Puts `children` in `w:hyperlink` elements to `link`. A run with a
/// picture stays between the elements. A run with no character style gets
/// the `Hyperlink` style.
fn wrap_link(children: Vec<ParagraphChild>, link: &HyperlinkData) -> Vec<ParagraphChild> {
    let mut result = Vec::new();
    let mut open = Vec::new();
    for mut child in children {
        if has_picture(&child) {
            close_link(&mut result, &mut open, link);
            result.push(child);
            continue;
        }
        if let ParagraphChild::Run(run) = &mut child {
            if run.run_property.style.is_none() {
                run.run_property.style = Some(RunStyle::new("Hyperlink"));
            }
        }
        open.push(child);
    }
    close_link(&mut result, &mut open, link);
    result
}

/// Moves the children `open` to a `w:hyperlink` to `link` at the end of
/// `result`. Does nothing if `open` is empty.
fn close_link(
    result: &mut Vec<ParagraphChild>,
    open: &mut Vec<ParagraphChild>,
    link: &HyperlinkData,
) {
    if !open.is_empty() {
        // A struct literal: `Hyperlink::new` takes an ID from the
        // process-global counter of docx-rs.
        result.push(ParagraphChild::Hyperlink(Hyperlink {
            link: link.clone(),
            history: None,
            children: std::mem::take(open),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_name_is_short_and_valid() {
        for id in ["intro", "заголовки", "a-b_c.d", "", &"x".repeat(500)] {
            let name = bookmark_name(id);
            assert_eq!(name.len(), 19, "{name}");
            assert!(name.starts_with("_Id"), "{name}");
            assert!(name[3..].bytes().all(|b| b.is_ascii_hexdigit()), "{name}");
        }
    }

    #[test]
    fn bookmark_names_of_different_ids_differ() {
        let ids = ["заголовки", "заголовки-1", "заголовок", "таблицы", "intro"];
        let names: std::collections::HashSet<_> = ids.iter().map(|id| bookmark_name(id)).collect();
        assert_eq!(names.len(), ids.len());
        assert_eq!(bookmark_name("заголовки"), bookmark_name("заголовки"));
    }

    #[test]
    fn equal_urls_share_one_relationship() {
        let mut links = ExternalLinks::default();
        assert_eq!(links.rid("https://b.org"), "rIdLink1");
        assert_eq!(links.rid("https://a.org"), "rIdLink2");
        assert_eq!(links.rid("https://b.org"), "rIdLink1");
        let part = String::from_utf8(relationships_part(&links)).unwrap();
        assert_eq!(
            part,
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
                r#"<Relationship Id="rIdLink1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://b.org" TargetMode="External"/>"#,
                r#"<Relationship Id="rIdLink2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://a.org" TargetMode="External"/>"#,
                r#"</Relationships>"#,
            )
        );
    }

    #[test]
    fn external_target_has_scheme_and_slashes_or_is_mailto() {
        for target in [
            "https://x.org",
            "HTTP://X.ORG",
            "ftp://h/f",
            "a+b-c.d1://h",
            "mailto:a@b.org",
            "MAILTO:a@b.org",
        ] {
            assert!(is_external(target), "{target}");
        }
        for target in [
            "docs/page.html?u=https://y.org",
            "page.html#https://y.org",
            "1a://h",
            "a_b://h",
            "://h",
            "urn:isbn:1",
            "https:x.org",
            "mailto",
            "#x",
            "",
        ] {
            assert!(!is_external(target), "{target}");
        }
    }

    #[test]
    fn target_is_escaped() {
        let mut links = ExternalLinks::default();
        links.rid(r#"https://x.org/?a=1&b="2"<3>"#);
        let part = String::from_utf8(relationships_part(&links)).unwrap();
        assert!(
            part.contains(r#"Target="https://x.org/?a=1&amp;b=&quot;2&quot;&lt;3&gt;""#),
            "{part}"
        );
    }

    /// docx-rs writes the hyperlink relationships after all other
    /// relationships. The replacement depends on this order.
    #[test]
    fn docx_rs_writes_hyperlinks_last() {
        let mut docx = Docx::new()
            .add_paragraph(
                Paragraph::new()
                    .add_hyperlink(
                        Hyperlink::new("https://a.org", HyperlinkType::External)
                            .add_run(Run::new().add_text("a")),
                    )
                    .add_run(Run::new().add_image(Pic::new_with_dimensions(vec![0; 4], 1, 1))),
            )
            .add_paragraph(
                Paragraph::new().add_hyperlink(
                    Hyperlink::new("https://b.org", HyperlinkType::External)
                        .add_run(Run::new().add_text("b")),
                ),
            )
            .header(Header::new())
            .footer(Footer::new());
        docx.document_rels.has_numberings = true;
        docx.document_rels.has_footnotes = true;
        let rels = docx.build().document_rels;
        let rels = std::str::from_utf8(&rels).unwrap();
        let first = rels.find(HYPERLINK_TYPE).unwrap();
        let start = rels[..first].rfind("<Relationship ").unwrap();
        let tail = &rels[start..];
        assert_eq!(tail.matches("<Relationship ").count(), 2, "{rels}");
        assert_eq!(tail.matches(HYPERLINK_TYPE).count(), 2, "{rels}");
        assert!(rels[..start].contains("/image\""), "{rels}");
        assert!(rels[..start].contains("/header\""), "{rels}");
        assert!(rels[..start].contains("/footer\""), "{rels}");
        assert!(rels[..start].contains("/numbering\""), "{rels}");
        assert!(rels[..start].contains("/footnotes\""), "{rels}");

        let mut rels = rels.as_bytes().to_vec();
        let mut links = ExternalLinks::default();
        links.rid("https://c.org");
        replace_hyperlink_relationships(&mut rels, &links).unwrap();
        let rels = String::from_utf8(rels).unwrap();
        assert_eq!(rels.matches(HYPERLINK_TYPE).count(), 1, "{rels}");
        assert!(
            rels.ends_with(concat!(
                r#"<Relationship Id="rIdLink1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://c.org" TargetMode="External"/>"#,
                "</Relationships>"
            )),
            "{rels}"
        );
        assert!(!rels.contains("a.org"), "{rels}");
    }

    #[test]
    fn replacement_without_docx_rs_hyperlinks_adds_links_at_end() {
        let mut rels = br#"<Relationships><Relationship Id="rId1" /></Relationships>"#.to_vec();
        let mut links = ExternalLinks::default();
        links.rid("https://c.org");
        replace_hyperlink_relationships(&mut rels, &links).unwrap();
        let rels = String::from_utf8(rels).unwrap();
        assert!(
            rels.starts_with(
                r#"<Relationships><Relationship Id="rId1" /><Relationship Id="rIdLink1" "#
            ),
            "{rels}"
        );
        assert!(rels.ends_with("/></Relationships>"), "{rels}");
    }
}
