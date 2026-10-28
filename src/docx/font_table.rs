//! Font table (`word/fontTable.xml`) generator for the DOCX renderer.
//!
//! [`font_table_xml`] makes the font table from the [`DocxParams`] of a
//! theme. The font table contains one `w:font` element for each font name
//! in `fonts`, in name order. Two font roles with the same font name give
//! one element. Each element has only the `w:name` attribute. The XML has
//! no indentation.

use std::collections::BTreeSet;
use std::io::{self, Write};

use quick_xml::events::{BytesDecl, Event};
use quick_xml::Writer;

use super::params::DocxParams;
use super::stylesheet::W_NS;

/// Makes `word/fontTable.xml` from the font roles of the style parameters.
pub(crate) fn font_table_xml(params: &DocxParams) -> Vec<u8> {
    let mut w = Writer::new(Vec::new());
    write_font_table(&mut w, params).expect("a write to a Vec does not fail");
    w.into_inner()
}

fn write_font_table<W: Write>(w: &mut Writer<W>, params: &DocxParams) -> io::Result<()> {
    w.write_event(Event::Decl(BytesDecl::new(
        "1.0",
        Some("UTF-8"),
        Some("yes"),
    )))?;
    let names: BTreeSet<&str> = params.fonts.values().map(String::as_str).collect();
    w.create_element("w:fonts")
        .with_attribute(("xmlns:w", W_NS))
        .write_inner_content(|w| {
            for name in names {
                w.create_element("w:font")
                    .with_attribute(("w:name", name))
                    .write_empty()?;
            }
            Ok(())
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docx::stylesheet::styles_xml;
    use crate::theme::Theme;
    use quick_xml::Reader;

    const BLANK_YAML: &str = include_str!("../../themes/blank/docx.yaml");

    fn blank() -> String {
        let params = Theme::Embedded("blank").docx_params().unwrap();
        xml(&params)
    }

    /// Gives the parameters of the `blank` theme with the font role
    /// `extra` set to `name`. `name` is a YAML value.
    fn with_extra_font(name: &str) -> DocxParams {
        let yaml = BLANK_YAML.replace(
            "  mono: PT Mono\n",
            &format!("  mono: PT Mono\n  extra: {name}\n"),
        );
        DocxParams::from_yaml(&yaml).unwrap()
    }

    fn xml(params: &DocxParams) -> String {
        String::from_utf8(font_table_xml(params)).unwrap()
    }

    #[test]
    fn output_is_well_formed() {
        let xml = blank();
        assert!(xml.starts_with(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:fonts xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:font "#
        ));
        assert!(xml.ends_with("/></w:fonts>"));
        let mut reader = Reader::from_str(&xml);
        loop {
            match reader.read_event().unwrap() {
                Event::Eof => break,
                _ => continue,
            }
        }
    }

    #[test]
    fn xmllint_accepts_output() {
        use std::process::{Command, Stdio};

        // Skip if xmllint is not on PATH.
        if which::which("xmllint").is_err() {
            eprintln!("skipping xmllint_accepts_output: xmllint not found on PATH");
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
            .write_all(blank().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn one_entry_for_each_font_name() {
        let xml = blank();
        assert!(xml.ends_with(concat!(
            r#"<w:fonts xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
            r#"<w:font w:name="PT Mono"/><w:font w:name="PT Sans"/></w:fonts>"#,
        )));
        assert_eq!(xml.matches(r#"w:name="PT Sans""#).count(), 1);
    }

    #[test]
    fn unused_font_role_is_listed() {
        let xml = xml(&with_extra_font("Noto Serif"));
        assert!(xml.contains(concat!(
            r#"<w:font w:name="Noto Serif"/><w:font w:name="PT Mono"/>"#,
            r#"<w:font w:name="PT Sans"/></w:fonts>"#,
        )));
    }

    #[test]
    fn name_only() {
        let xml = blank();
        for part in [
            "w:altName",
            "w:charset",
            "w:family",
            "w:pitch",
            "w:panose1",
            "w:sig",
            "xmlns:r",
        ] {
            assert!(!xml.contains(part), "unexpected: {part}\nin: {xml}");
        }
    }

    #[test]
    fn names_are_escaped() {
        let xml = xml(&with_extra_font(r#"'A&B <C> "q"'"#));
        assert!(xml.contains(r#"<w:font w:name="A&amp;B &lt;C&gt; &quot;q&quot;"/>"#));
    }

    #[test]
    fn style_fonts_are_listed() {
        let params = Theme::Embedded("blank").docx_params().unwrap();
        let styles = String::from_utf8(styles_xml(&params, "en")).unwrap();
        let fonts = xml(&params);
        let mut count = 0;
        for slot in ["w:ascii", "w:hAnsi", "w:eastAsia", "w:cs"] {
            let attr = format!(" {slot}=\"");
            for (at, _) in styles.match_indices(&attr) {
                let rest = &styles[at + attr.len()..];
                let name = &rest[..rest.find('"').unwrap()];
                assert!(
                    fonts.contains(&format!("<w:font w:name=\"{name}\"/>")),
                    "missing {slot}: {name}\nin: {fonts}"
                );
                count += 1;
            }
        }
        assert!(count > 0);
    }
}
