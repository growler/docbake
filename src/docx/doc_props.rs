//! Core properties (`docProps/core.xml`) generator for the DOCX renderer.
//!
//! [`core_xml`] writes the values that `docx-rs` writes: the creation and
//! modification time 1970-01-01T00:00:00Z, the creator and the last editor
//! `unknown`, and the revision 1. Thus the same input gives the same part.
//! A document title goes in `dc:title`. The XML has no indentation.

use std::io::{self, Write};

use quick_xml::events::{BytesDecl, BytesText, Event};
use quick_xml::Writer;

/// The fixed creation and modification time.
const TIME: &str = "1970-01-01T00:00:00Z";

/// Makes `docProps/core.xml`. An empty `title` gives no `dc:title`.
pub(super) fn core_xml(title: &str) -> Vec<u8> {
    let mut w = Writer::new(Vec::new());
    write_core(&mut w, title).expect("a write to a Vec does not fail");
    w.into_inner()
}

fn write_core<W: Write>(w: &mut Writer<W>, title: &str) -> io::Result<()> {
    w.write_event(Event::Decl(BytesDecl::new(
        "1.0",
        Some("UTF-8"),
        Some("yes"),
    )))?;
    w.create_element("cp:coreProperties")
        .with_attributes([
            (
                "xmlns:cp",
                "http://schemas.openxmlformats.org/package/2006/metadata/core-properties",
            ),
            ("xmlns:dc", "http://purl.org/dc/elements/1.1/"),
            ("xmlns:dcterms", "http://purl.org/dc/terms/"),
            ("xmlns:dcmitype", "http://purl.org/dc/dcmitype/"),
            ("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance"),
        ])
        .write_inner_content(|w| {
            w.create_element("dcterms:created")
                .with_attribute(("xsi:type", "dcterms:W3CDTF"))
                .write_text_content(BytesText::new(TIME))?;
            w.create_element("dc:creator")
                .write_text_content(BytesText::new("unknown"))?;
            w.create_element("cp:lastModifiedBy")
                .write_text_content(BytesText::new("unknown"))?;
            w.create_element("dcterms:modified")
                .with_attribute(("xsi:type", "dcterms:W3CDTF"))
                .write_text_content(BytesText::new(TIME))?;
            w.create_element("cp:revision")
                .write_text_content(BytesText::new("1"))?;
            if !title.is_empty() {
                w.create_element("dc:title")
                    .write_text_content(BytesText::new(title))?;
            }
            Ok(())
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_title_matches_docx_rs() {
        let docx_rs = docx_rs::Docx::new().build().doc_props.core;
        assert_eq!(
            String::from_utf8(core_xml("")).unwrap(),
            String::from_utf8(docx_rs).unwrap()
        );
    }
}
