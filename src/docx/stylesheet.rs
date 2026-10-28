//! Style sheet (`word/styles.xml`) generator for the DOCX renderer.
//!
//! [`styles_xml`] makes the style sheet from the [`DocxParams`] of a theme.
//! The style sheet contains:
//!
//! - `w:docDefaults` from the `Normal` style, the body font and the
//!   document language
//! - the base styles [`DEFAULT_PARAGRAPH_FONT`], [`TABLE_NORMAL`] and
//!   [`NO_LIST`]
//! - one paragraph or character style for each entry of `styles`, in ID
//!   order
//! - the table style [`TABLE_STYLE`] from the table parameters
//!
//! The elements in each property group have the order of the OOXML schema
//! (ECMA-376 part 1: `CT_Style`, `CT_PPrBase`, `EG_RPrBase`,
//! `CT_TblPrBase`). The XML has no indentation.

use std::borrow::Cow;
use std::fmt::Display;
use std::io::{self, Write};

use quick_xml::events::{BytesDecl, Event};
use quick_xml::Writer;

use super::params::{Align, Border, DocxParams, LineSpacing, Rgb, Style, StyleKind, TableParams};

pub(crate) const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// The ID of the default character style.
pub(crate) const DEFAULT_PARAGRAPH_FONT: &str = "DefaultParagraphFont";

/// The ID of the default table style.
pub(crate) const TABLE_NORMAL: &str = "TableNormal";

/// The ID of the default numbering style.
pub(crate) const NO_LIST: &str = "NoList";

/// The ID of the table style that the generator makes from the table
/// parameters.
pub(crate) const TABLE_STYLE: &str = "DataTable";

/// The IDs of the styles that the generator makes in addition to the theme
/// styles. A theme cannot define them.
pub(crate) const RESERVED_STYLES: [&str; 4] =
    [DEFAULT_PARAGRAPH_FONT, TABLE_NORMAL, NO_LIST, TABLE_STYLE];

/// Makes `word/styles.xml` from the style parameters. `lang` is the language
/// tag of the document, for example `ru-RU`. It must be a valid BCP 47 tag.
/// The caller checks it.
pub(crate) fn styles_xml(params: &DocxParams, lang: &str) -> Vec<u8> {
    let mut w = Writer::new(Vec::new());
    write_styles(&mut w, params, lang).expect("a write to a Vec does not fail");
    w.into_inner()
}

fn write_styles<W: Write>(w: &mut Writer<W>, params: &DocxParams, lang: &str) -> io::Result<()> {
    w.write_event(Event::Decl(BytesDecl::new(
        "1.0",
        Some("UTF-8"),
        Some("yes"),
    )))?;
    w.create_element("w:styles")
        .with_attribute(("xmlns:w", W_NS))
        .write_inner_content(|w| {
            doc_defaults(w, params, lang)?;
            base_styles(w)?;
            for (id, style) in &params.styles {
                theme_style(w, id, style)?;
            }
            table_style(w, &params.table)
        })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Document defaults and base styles
// ---------------------------------------------------------------------------

/// Writes `w:docDefaults`: the body font, the size of `Normal` and the
/// language for runs, and the spacing of `Normal` for paragraphs.
fn doc_defaults<W: Write>(w: &mut Writer<W>, params: &DocxParams, lang: &str) -> io::Result<()> {
    // The loader makes sure that `Normal` and the font role `body` exist.
    let normal = &params.styles["Normal"];
    let body = params.fonts["body"].as_str();
    w.create_element("w:docDefaults").write_inner_content(|w| {
        w.create_element("w:rPrDefault").write_inner_content(|w| {
            w.create_element("w:rPr").write_inner_content(|w| {
                fonts(w, body)?;
                if let Some(size) = normal.size {
                    font_size(w, size.0)?;
                }
                val(w, "w:lang", lang)
            })?;
            Ok(())
        })?;
        if has_spacing(normal) {
            w.create_element("w:pPrDefault").write_inner_content(|w| {
                w.create_element("w:pPr")
                    .write_inner_content(|w| spacing(w, normal))?;
                Ok(())
            })?;
        }
        Ok(())
    })?;
    Ok(())
}

/// Writes the base styles of Word that the theme styles and the document
/// body refer to.
fn base_styles<W: Write>(w: &mut Writer<W>) -> io::Result<()> {
    style_element(w, "character", DEFAULT_PARAGRAPH_FONT, None, |_, _| Ok(()))?;
    style_element(w, "table", TABLE_NORMAL, None, |w, _| {
        w.create_element("w:tblPr").write_inner_content(|w| {
            w.create_element("w:tblInd")
                .with_attribute(("w:w", "0"))
                .with_attribute(("w:type", "dxa"))
                .write_empty()?;
            cell_margins(w, [0, 108, 0, 108])
        })?;
        Ok(())
    })?;
    style_element(w, "numbering", NO_LIST, None, |_, _| Ok(()))
}

// ---------------------------------------------------------------------------
// Style names and flags
// ---------------------------------------------------------------------------

/// The `w:name` and the user interface flags of a style.
struct StyleInfo {
    name: &'static str,
    /// `w:customStyle`: Word does not know the style.
    custom: bool,
    /// `w:next` is `Normal`.
    next_normal: bool,
    ui_priority: Option<u8>,
    semi_hidden: bool,
    unhide_when_used: bool,
    q_format: bool,
    /// The outline level of a paragraph style (0 for `Heading1`, 9 for body
    /// text).
    outline: Option<u8>,
}

const PLAIN: StyleInfo = StyleInfo {
    name: "",
    custom: false,
    next_normal: false,
    ui_priority: None,
    semi_hidden: false,
    unhide_when_used: false,
    q_format: false,
    outline: None,
};

/// The flags of a style that is not built into Word. The name comes from
/// [`display_name`].
const CUSTOM: StyleInfo = StyleInfo {
    custom: true,
    q_format: true,
    ..PLAIN
};

const fn heading(name: &'static str, level: u8) -> StyleInfo {
    StyleInfo {
        name,
        next_normal: true,
        ui_priority: Some(9),
        unhide_when_used: level > 0,
        q_format: true,
        outline: Some(level),
        ..PLAIN
    }
}

const fn toc(name: &'static str) -> StyleInfo {
    StyleInfo {
        name,
        ui_priority: Some(39),
        unhide_when_used: true,
        ..PLAIN
    }
}

/// A style that Word shows in the style list after a document uses it.
const fn on_use(name: &'static str) -> StyleInfo {
    StyleInfo {
        name,
        ui_priority: Some(99),
        unhide_when_used: true,
        ..PLAIN
    }
}

const fn hidden(name: &'static str) -> StyleInfo {
    StyleInfo {
        semi_hidden: true,
        ..on_use(name)
    }
}

const fn quick(name: &'static str, ui_priority: u8) -> StyleInfo {
    StyleInfo {
        name,
        ui_priority: Some(ui_priority),
        q_format: true,
        ..PLAIN
    }
}

/// The styles with known names and flags: the Word built-in styles and the
/// styles that the generator makes.
const BUILTIN: &[(&str, StyleInfo)] = &[
    (
        DEFAULT_PARAGRAPH_FONT,
        StyleInfo {
            ui_priority: Some(1),
            ..hidden("Default Paragraph Font")
        },
    ),
    (TABLE_NORMAL, hidden("Normal Table")),
    (NO_LIST, hidden("No List")),
    (
        TABLE_STYLE,
        StyleInfo {
            custom: true,
            ..quick("Data Table", 59)
        },
    ),
    (
        "Normal",
        StyleInfo {
            name: "Normal",
            q_format: true,
            ..PLAIN
        },
    ),
    (
        "Title",
        StyleInfo {
            next_normal: true,
            ..quick("Title", 10)
        },
    ),
    (
        "Subtitle",
        StyleInfo {
            next_normal: true,
            ..quick("Subtitle", 11)
        },
    ),
    ("Heading1", heading("heading 1", 0)),
    ("Heading2", heading("heading 2", 1)),
    ("Heading3", heading("heading 3", 2)),
    ("Heading4", heading("heading 4", 3)),
    ("Heading5", heading("heading 5", 4)),
    ("Heading6", heading("heading 6", 5)),
    ("TOC1", toc("toc 1")),
    ("TOC2", toc("toc 2")),
    ("TOC3", toc("toc 3")),
    ("TOC4", toc("toc 4")),
    ("TOC5", toc("toc 5")),
    ("TOC6", toc("toc 6")),
    ("TOC7", toc("toc 7")),
    ("TOC8", toc("toc 8")),
    ("TOC9", toc("toc 9")),
    (
        // Outline level 9 keeps the heading of the table of contents out of
        // the table of contents.
        "TOCHeading",
        StyleInfo {
            q_format: true,
            outline: Some(9),
            ..toc("TOC Heading")
        },
    ),
    ("Header", on_use("header")),
    ("Footer", on_use("footer")),
    ("Hyperlink", on_use("Hyperlink")),
    ("Strong", quick("Strong", 22)),
    ("Emphasis", quick("Emphasis", 20)),
    ("Quote", quick("Quote", 29)),
    ("ListParagraph", quick("List Paragraph", 34)),
    (
        "Caption",
        StyleInfo {
            unhide_when_used: true,
            ..quick("caption", 35)
        },
    ),
    ("FootnoteText", hidden("footnote text")),
    ("FootnoteReference", hidden("footnote reference")),
];

fn builtin(id: &str) -> Option<&'static StyleInfo> {
    BUILTIN
        .iter()
        .find(|(builtin, _)| *builtin == id)
        .map(|(_, info)| info)
}

/// Gives the ID and the name of each style in [`BUILTIN`].
pub(crate) fn builtin_styles() -> impl Iterator<Item = (&'static str, &'static str)> {
    BUILTIN.iter().map(|(id, info)| (*id, info.name))
}

/// Gives the `w:name` of a style ID.
pub(crate) fn style_name(id: &str) -> Cow<'static, str> {
    match builtin(id) {
        Some(info) => Cow::Borrowed(info.name),
        None => Cow::Owned(display_name(id)),
    }
}

/// Makes the name of a custom style ID. A space goes before each capital
/// letter that follows a lowercase letter or a digit: `CodeBlock` gives
/// `Code Block`.
fn display_name(id: &str) -> String {
    let mut name = String::with_capacity(id.len() + 4);
    let mut after_word = false;
    for c in id.chars() {
        if after_word && c.is_ascii_uppercase() {
            name.push(' ');
        }
        after_word = c.is_ascii_lowercase() || c.is_ascii_digit();
        name.push(c);
    }
    name
}

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

/// Writes one `w:style` element: the attributes, the name, the base style
/// and the user interface flags. `properties` writes the property groups
/// after them.
fn style_element<W, F>(
    w: &mut Writer<W>,
    kind: &str,
    id: &str,
    based_on: Option<&str>,
    properties: F,
) -> io::Result<()>
where
    W: Write,
    F: FnOnce(&mut Writer<W>, &StyleInfo) -> io::Result<()>,
{
    let (info, name) = match builtin(id) {
        Some(info) => (info, Cow::Borrowed(info.name)),
        None => (&CUSTOM, Cow::Owned(display_name(id))),
    };
    let mut element = w.create_element("w:style").with_attribute(("w:type", kind));
    if id == "Normal" || id == DEFAULT_PARAGRAPH_FONT || id == TABLE_NORMAL || id == NO_LIST {
        element = element.with_attribute(("w:default", "1"));
    }
    if info.custom {
        element = element.with_attribute(("w:customStyle", "1"));
    }
    element
        .with_attribute(("w:styleId", id))
        .write_inner_content(|w| {
            val(w, "w:name", &name)?;
            if let Some(base) = based_on {
                val(w, "w:basedOn", base)?;
            }
            if info.next_normal {
                val(w, "w:next", "Normal")?;
            }
            if let Some(priority) = info.ui_priority {
                number(w, "w:uiPriority", priority)?;
            }
            if info.semi_hidden {
                empty(w, "w:semiHidden")?;
            }
            if info.unhide_when_used {
                empty(w, "w:unhideWhenUsed")?;
            }
            if info.q_format {
                empty(w, "w:qFormat")?;
            }
            properties(w, info)
        })?;
    Ok(())
}

/// Writes one theme style.
fn theme_style<W: Write>(w: &mut Writer<W>, id: &str, style: &Style) -> io::Result<()> {
    let default_base = match style.kind {
        StyleKind::Paragraph => None,
        StyleKind::Character => Some(DEFAULT_PARAGRAPH_FONT),
    };
    let based_on = style.based_on.as_deref().or(default_base);
    style_element(w, style.kind.as_str(), id, based_on, |w, info| {
        if style.kind == StyleKind::Paragraph {
            paragraph_properties(w, style, info.outline)?;
        }
        run_properties(w, style)
    })
}

fn has_spacing(style: &Style) -> bool {
    style.before.is_some() || style.after.is_some() || style.line.is_some()
}

fn has_borders(style: &Style) -> bool {
    let b = &style.borders;
    b.top.is_some() || b.left.is_some() || b.bottom.is_some() || b.right.is_some()
}

/// Writes `w:pPr` of a paragraph style. Writes nothing if the style sets no
/// paragraph property.
fn paragraph_properties<W: Write>(
    w: &mut Writer<W>,
    style: &Style,
    outline: Option<u8>,
) -> io::Result<()> {
    let s = style;
    let set = s.keep_next.is_some()
        || has_borders(s)
        || s.fill.is_some()
        || s.hyphenate.is_some()
        || s.snap_to_grid.is_some()
        || has_spacing(s)
        || s.indent.is_some()
        || s.contextual_spacing.is_some()
        || s.align.is_some()
        || outline.is_some();
    if !set {
        return Ok(());
    }
    w.create_element("w:pPr").write_inner_content(|w| {
        on_off(w, "w:keepNext", s.keep_next)?;
        if has_borders(s) {
            let b = &s.borders;
            w.create_element("w:pBdr").write_inner_content(|w| {
                let sides = [
                    ("w:top", b.top),
                    ("w:left", b.left),
                    ("w:bottom", b.bottom),
                    ("w:right", b.right),
                ];
                for (side, border) in sides {
                    if let Some(border) = border {
                        border_element(w, side, border)?;
                    }
                }
                Ok(())
            })?;
        }
        if let Some(fill) = s.fill {
            shading(w, fill)?;
        }
        on_off(w, "w:suppressAutoHyphens", s.hyphenate.map(|h| !h))?;
        on_off(w, "w:snapToGrid", s.snap_to_grid)?;
        spacing(w, s)?;
        if let Some(indent) = s.indent {
            w.create_element("w:ind")
                .with_attribute(("w:left", indent.0.to_string().as_str()))
                .write_empty()?;
        }
        on_off(w, "w:contextualSpacing", s.contextual_spacing)?;
        if let Some(align) = s.align {
            let jc = match align {
                Align::Left => "left",
                Align::Center => "center",
                Align::Right => "right",
                Align::Justify => "both",
            };
            val(w, "w:jc", jc)?;
        }
        if let Some(level) = outline {
            number(w, "w:outlineLvl", level)?;
        }
        Ok(())
    })?;
    Ok(())
}

/// Writes `w:spacing` of a paragraph. Writes nothing if the style sets no
/// spacing.
fn spacing<W: Write>(w: &mut Writer<W>, style: &Style) -> io::Result<()> {
    if !has_spacing(style) {
        return Ok(());
    }
    let mut element = w.create_element("w:spacing");
    if let Some(before) = style.before {
        element = element.with_attribute(("w:before", before.0.to_string().as_str()));
    }
    if let Some(after) = style.after {
        element = element.with_attribute(("w:after", after.0.to_string().as_str()));
    }
    if let Some(line) = style.line {
        let (value, rule) = match line {
            LineSpacing::Auto(value) => (value, "auto"),
            LineSpacing::Exact(twips) => (twips.0, "exact"),
        };
        element = element
            .with_attribute(("w:line", value.to_string().as_str()))
            .with_attribute(("w:lineRule", rule));
    }
    element.write_empty()?;
    Ok(())
}

/// Writes `w:rPr` of a style. Writes nothing if the style sets no run
/// property. The fill of a paragraph style is a paragraph property.
fn run_properties<W: Write>(w: &mut Writer<W>, style: &Style) -> io::Result<()> {
    let s = style;
    let fill = match s.kind {
        StyleKind::Character => s.fill,
        StyleKind::Paragraph => None,
    };
    let set = s.font.is_some()
        || s.bold.is_some()
        || s.italic.is_some()
        || s.proofing.is_some()
        || s.color.is_some()
        || s.char_spacing.is_some()
        || s.kern.is_some()
        || s.size.is_some()
        || fill.is_some();
    if !set {
        return Ok(());
    }
    w.create_element("w:rPr").write_inner_content(|w| {
        if let Some(font) = s.font.as_deref() {
            fonts(w, font)?;
        }
        on_off(w, "w:b", s.bold)?;
        on_off(w, "w:bCs", s.bold)?;
        on_off(w, "w:i", s.italic)?;
        on_off(w, "w:iCs", s.italic)?;
        on_off(w, "w:noProof", s.proofing.map(|p| !p))?;
        if let Some(color) = s.color {
            val(w, "w:color", &color.to_string())?;
        }
        if let Some(spacing) = s.char_spacing {
            number(w, "w:spacing", spacing.0)?;
        }
        if let Some(kern) = s.kern {
            number(w, "w:kern", kern.0)?;
        }
        if let Some(size) = s.size {
            font_size(w, size.0)?;
        }
        if let Some(fill) = fill {
            shading(w, fill)?;
        }
        Ok(())
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Table style
// ---------------------------------------------------------------------------

/// Writes the table style [`TABLE_STYLE`]: the borders and the cell margins
/// of the table, and the format of the header row.
fn table_style<W: Write>(w: &mut Writer<W>, table: &TableParams) -> io::Result<()> {
    style_element(w, "table", TABLE_STYLE, Some(TABLE_NORMAL), |w, _| {
        w.create_element("w:tblPr").write_inner_content(|w| {
            w.create_element("w:tblBorders").write_inner_content(|w| {
                for side in [
                    "w:top",
                    "w:left",
                    "w:bottom",
                    "w:right",
                    "w:insideH",
                    "w:insideV",
                ] {
                    border_element(w, side, table.border)?;
                }
                Ok(())
            })?;
            let m = &table.cell_margins;
            cell_margins(w, [m.top.0, m.left.0, m.bottom.0, m.right.0])
        })?;
        w.create_element("w:tblStylePr")
            .with_attribute(("w:type", "firstRow"))
            .write_inner_content(|w| {
                if table.header.bold {
                    w.create_element("w:rPr").write_inner_content(|w| {
                        empty(w, "w:b")?;
                        empty(w, "w:bCs")
                    })?;
                }
                w.create_element("w:tcPr")
                    .write_inner_content(|w| shading(w, table.header.fill))?;
                Ok(())
            })?;
        Ok(())
    })
}

/// Writes `w:tblCellMar` with the margins top, left, bottom and right in
/// twips.
fn cell_margins<W: Write>(w: &mut Writer<W>, margins: [u32; 4]) -> io::Result<()> {
    w.create_element("w:tblCellMar").write_inner_content(|w| {
        for (side, width) in ["w:top", "w:left", "w:bottom", "w:right"]
            .into_iter()
            .zip(margins)
        {
            w.create_element(side)
                .with_attribute(("w:w", width.to_string().as_str()))
                .with_attribute(("w:type", "dxa"))
                .write_empty()?;
        }
        Ok(())
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Element helpers
// ---------------------------------------------------------------------------

fn empty<W: Write>(w: &mut Writer<W>, name: &str) -> io::Result<()> {
    w.create_element(name).write_empty()?;
    Ok(())
}

/// Writes an element with one `w:val` attribute.
fn val<W: Write>(w: &mut Writer<W>, name: &str, value: &str) -> io::Result<()> {
    w.create_element(name)
        .with_attribute(("w:val", value))
        .write_empty()?;
    Ok(())
}

/// Writes an element with one numeric `w:val` attribute.
fn number<W: Write>(w: &mut Writer<W>, name: &str, value: impl Display) -> io::Result<()> {
    val(w, name, &value.to_string())
}

/// Writes an on/off property. `true` gives the empty element, `false` gives
/// `w:val="0"`, and `None` writes nothing.
fn on_off<W: Write>(w: &mut Writer<W>, name: &str, value: Option<bool>) -> io::Result<()> {
    match value {
        Some(true) => empty(w, name),
        Some(false) => val(w, name, "0"),
        None => Ok(()),
    }
}

/// Writes `w:rFonts` with the font in all four script slots.
fn fonts<W: Write>(w: &mut Writer<W>, font: &str) -> io::Result<()> {
    w.create_element("w:rFonts")
        .with_attribute(("w:ascii", font))
        .with_attribute(("w:hAnsi", font))
        .with_attribute(("w:eastAsia", font))
        .with_attribute(("w:cs", font))
        .write_empty()?;
    Ok(())
}

/// Writes `w:sz` and `w:szCs` in half-points.
fn font_size<W: Write>(w: &mut Writer<W>, half_points: u32) -> io::Result<()> {
    number(w, "w:sz", half_points)?;
    number(w, "w:szCs", half_points)
}

/// Writes `w:shd` with a solid fill.
fn shading<W: Write>(w: &mut Writer<W>, fill: Rgb) -> io::Result<()> {
    w.create_element("w:shd")
        .with_attribute(("w:val", "clear"))
        .with_attribute(("w:color", "auto"))
        .with_attribute(("w:fill", fill.to_string().as_str()))
        .write_empty()?;
    Ok(())
}

/// Writes one single-line border edge, for example `w:top`.
fn border_element<W: Write>(w: &mut Writer<W>, name: &str, border: Border) -> io::Result<()> {
    w.create_element(name)
        .with_attribute(("w:val", "single"))
        .with_attribute(("w:sz", border.width.0.to_string().as_str()))
        .with_attribute(("w:space", border.space.0.to_string().as_str()))
        .with_attribute(("w:color", border.color.to_string().as_str()))
        .write_empty()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use quick_xml::Reader;

    const BASE: &str = r#"
page:
  size: a4
  margins: {top: 2cm, bottom: 2cm, left: 2cm, right: 1cm, header: 1.5cm, footer: 1.75cm}
fonts: {body: PT Sans, heading: PT Sans, mono: PT Mono}
colors: {accent: EE721C}
table:
  cell_margins: {top: 0tw, bottom: 0tw, left: 10tw, right: 10tw}
  border: {width: 0.25pt, color: "000000", space: 0pt}
  header: {bold: false, fill: F2F2F2}
lists:
  indent: 720tw
  hanging: 360tw
  bullets: ["•", "◦", "▪", "•", "◦", "▪"]
  numbers: [decimal, lower_letter, lower_roman, decimal, lower_letter, lower_roman]
styles:
"#;

    fn blank() -> String {
        let params = Theme::Embedded("blank").docx_params().unwrap();
        xml(&params, "ru-RU")
    }

    fn sample(styles: &str) -> String {
        let params = DocxParams::from_yaml(&format!("{BASE}{styles}")).unwrap();
        xml(&params, "en")
    }

    fn xml(params: &DocxParams, lang: &str) -> String {
        String::from_utf8(styles_xml(params, lang)).unwrap()
    }

    /// Gives the `w:style` element with the ID `id`.
    #[track_caller]
    fn style<'a>(xml: &'a str, id: &str) -> &'a str {
        let at = xml
            .find(&format!(" w:styleId=\"{id}\""))
            .unwrap_or_else(|| panic!("no style {id}"));
        let start = xml[..at].rfind("<w:style ").unwrap();
        let end = at + xml[at..].find("</w:style>").unwrap() + "</w:style>".len();
        &xml[start..end]
    }

    #[track_caller]
    fn assert_has(xml: &str, part: &str) {
        assert!(xml.contains(part), "missing: {part}\nin: {xml}");
    }

    #[track_caller]
    fn assert_lacks(xml: &str, part: &str) {
        assert!(!xml.contains(part), "unexpected: {part}\nin: {xml}");
    }

    #[test]
    fn output_is_well_formed() {
        let xml = blank();
        assert!(xml.starts_with(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults>"#
        ));
        assert!(xml.ends_with("</w:style></w:styles>"));
        let mut reader = Reader::from_str(&xml);
        loop {
            match reader.read_event().unwrap() {
                Event::Eof => break,
                _ => continue,
            }
        }
        let params = Theme::Embedded("blank").docx_params().unwrap();
        assert_eq!(xml.matches("<w:style ").count(), params.styles.len() + 4);
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
    fn all_styles_present_with_type() {
        let xml = blank();
        let params = Theme::Embedded("blank").docx_params().unwrap();
        for (id, s) in &params.styles {
            let kind = s.kind.as_str();
            assert!(
                style(&xml, id).starts_with(&format!("<w:style w:type=\"{kind}\"")),
                "{id}"
            );
        }
        for (id, kind) in [
            ("InlineCode", "character"),
            ("Hyperlink", "character"),
            ("FootnoteReference", "character"),
            ("Heading1", "paragraph"),
            ("DefaultParagraphFont", "character"),
            ("TableNormal", "table"),
            ("NoList", "numbering"),
            ("DataTable", "table"),
        ] {
            assert!(
                style(&xml, id).starts_with(&format!("<w:style w:type=\"{kind}\"")),
                "{id}"
            );
        }
    }

    #[test]
    fn doc_defaults_from_normal() {
        let xml = blank();
        assert_has(
            &xml,
            concat!(
                "<w:docDefaults><w:rPrDefault><w:rPr>",
                r#"<w:rFonts w:ascii="PT Sans" w:hAnsi="PT Sans" w:eastAsia="PT Sans" w:cs="PT Sans"/>"#,
                r#"<w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="ru-RU"/>"#,
                "</w:rPr></w:rPrDefault><w:pPrDefault><w:pPr>",
                r#"<w:spacing w:after="120" w:line="240" w:lineRule="auto"/>"#,
                "</w:pPr></w:pPrDefault></w:docDefaults>",
            ),
        );
        let normal = style(&xml, "Normal");
        assert_has(
            normal,
            r#"<w:spacing w:after="120" w:line="240" w:lineRule="auto"/>"#,
        );
        assert_has(normal, r#"<w:jc w:val="both"/>"#);

        let xml = sample("  Normal: {}\n");
        assert_has(
            &xml,
            r#"<w:lang w:val="en"/></w:rPr></w:rPrDefault></w:docDefaults>"#,
        );
    }

    #[test]
    fn default_and_custom_flags() {
        let xml = blank();
        assert_eq!(xml.matches(r#"w:default="1""#).count(), 4);
        for id in ["Normal", "DefaultParagraphFont", "TableNormal", "NoList"] {
            assert_has(style(&xml, id), r#"w:default="1""#);
        }
        assert_has(style(&xml, "CodeBlock"), r#"w:customStyle="1""#);
        assert_has(style(&xml, "DataTable"), r#"w:customStyle="1""#);
        assert_lacks(style(&xml, "Heading1"), "w:customStyle");
        for (id, name) in [
            ("Normal", "Normal"),
            ("Heading1", "heading 1"),
            ("TOC9", "toc 9"),
            ("Caption", "caption"),
            ("FootnoteText", "footnote text"),
            ("Header", "header"),
            ("ListParagraph", "List Paragraph"),
            ("CodeBlock", "Code Block"),
            ("AdmonitionNoteHeader", "Admonition Note Header"),
            ("DataTable", "Data Table"),
        ] {
            assert_has(style(&xml, id), &format!("<w:name w:val=\"{name}\"/>"));
        }
        assert_eq!(display_name("Heading7x2Y"), "Heading7x2 Y");
        assert_eq!(style_name("Heading1"), "heading 1");
        assert_eq!(style_name("CodeBlock"), "Code Block");
    }

    #[test]
    fn based_on_and_next() {
        let xml = blank();
        for (id, base) in [
            ("CodeBlock", "Normal"),
            ("AdmonitionNoteBody", "AdmonitionBody"),
            ("TableHeader", "TableCell"),
            ("InlineCode", "DefaultParagraphFont"),
            ("FootnoteReference", "DefaultParagraphFont"),
            ("DataTable", "TableNormal"),
        ] {
            assert_has(style(&xml, id), &format!("<w:basedOn w:val=\"{base}\"/>"));
        }
        assert_lacks(style(&xml, "Normal"), "w:basedOn");
        for id in ["Title", "Subtitle", "Heading1", "Heading6"] {
            assert_has(style(&xml, id), r#"<w:next w:val="Normal"/>"#);
        }
        for id in ["TOC1", "CodeBlock", "AdmonitionNoteHeader"] {
            assert_lacks(style(&xml, id), "<w:next ");
        }
        assert_lacks(&xml, "<w:link ");
        assert_lacks(&xml, "latentStyles");
    }

    #[test]
    fn heading1_properties_in_schema_order() {
        let xml = blank();
        let h1 = style(&xml, "Heading1");
        assert_has(
            h1,
            concat!(
                r#"<w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/>"#,
                r#"<w:uiPriority w:val="9"/><w:qFormat/>"#,
            ),
        );
        assert_has(
            h1,
            concat!(
                "<w:pPr><w:keepNext/>",
                r#"<w:pBdr><w:bottom w:val="single" w:sz="4" w:space="1" w:color="EE721C"/></w:pBdr>"#,
                r#"<w:spacing w:before="360" w:after="120" w:line="240" w:lineRule="auto"/>"#,
                r#"<w:jc w:val="left"/><w:outlineLvl w:val="0"/></w:pPr>"#,
            ),
        );
        assert_has(
            h1,
            concat!(
                "<w:rPr>",
                r#"<w:rFonts w:ascii="PT Sans" w:hAnsi="PT Sans" w:eastAsia="PT Sans" w:cs="PT Sans"/>"#,
                r#"<w:b/><w:bCs/><w:color w:val="EE721C"/><w:sz w:val="56"/><w:szCs w:val="56"/>"#,
                "</w:rPr></w:style>",
            ),
        );
        let h2 = style(&xml, "Heading2");
        assert_has(
            h2,
            r#"<w:uiPriority w:val="9"/><w:unhideWhenUsed/><w:qFormat/>"#,
        );
        assert_has(style(&xml, "Heading6"), r#"<w:outlineLvl w:val="5"/>"#);
    }

    #[test]
    fn code_block_properties() {
        let xml = blank();
        let code = style(&xml, "CodeBlock");
        assert_has(
            code,
            concat!(
                "<w:pPr><w:pBdr>",
                r#"<w:top w:val="single" w:sz="4" w:space="1" w:color="D9D9D9"/>"#,
                r#"<w:left w:val="single" w:sz="4" w:space="4" w:color="D9D9D9"/>"#,
                r#"<w:bottom w:val="single" w:sz="4" w:space="1" w:color="D9D9D9"/>"#,
                r#"<w:right w:val="single" w:sz="4" w:space="4" w:color="D9D9D9"/>"#,
                "</w:pBdr>",
                r#"<w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/>"#,
                "<w:suppressAutoHyphens/>",
                r#"<w:spacing w:after="0"/><w:contextualSpacing/><w:jc w:val="left"/></w:pPr>"#,
            ),
        );
        assert_has(
            code,
            concat!(
                "<w:rPr>",
                r#"<w:rFonts w:ascii="PT Mono" w:hAnsi="PT Mono" w:eastAsia="PT Mono" w:cs="PT Mono"/>"#,
                r#"<w:noProof/><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr>"#,
            ),
        );
    }

    #[test]
    fn contextual_spacing() {
        let xml = blank();
        for id in ["CodeBlock", "AdmonitionBody", "Title"] {
            assert_has(style(&xml, id), "<w:contextualSpacing/>");
        }
        for id in ["AdmonitionNoteBody", "Normal", "Heading1"] {
            assert_lacks(style(&xml, id), "contextualSpacing");
        }
        let xml = sample("  Normal: {contextual_spacing: false}\n");
        assert_has(
            style(&xml, "Normal"),
            r#"<w:pPr><w:contextualSpacing w:val="0"/></w:pPr>"#,
        );
    }

    #[test]
    fn character_style_properties() {
        let xml = blank();
        let code = style(&xml, "InlineCode");
        assert_lacks(code, "<w:pPr>");
        assert_has(
            code,
            concat!(
                r#"<w:name w:val="Inline Code"/><w:basedOn w:val="DefaultParagraphFont"/><w:qFormat/>"#,
                "<w:rPr>",
                r#"<w:rFonts w:ascii="PT Mono" w:hAnsi="PT Mono" w:eastAsia="PT Mono" w:cs="PT Mono"/>"#,
                r#"<w:b w:val="0"/><w:bCs w:val="0"/><w:i w:val="0"/><w:iCs w:val="0"/><w:noProof/>"#,
                r#"<w:sz w:val="20"/><w:szCs w:val="20"/>"#,
                r#"<w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/></w:rPr></w:style>"#,
            ),
        );
        assert_has(
            style(&xml, "Hyperlink"),
            r#"<w:unhideWhenUsed/><w:rPr><w:color w:val="0563C1"/></w:rPr></w:style>"#,
        );
        assert_has(
            style(&xml, "FootnoteReference"),
            concat!(
                r#"<w:name w:val="footnote reference"/><w:basedOn w:val="DefaultParagraphFont"/>"#,
                r#"<w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/></w:style>"#,
            ),
        );
    }

    #[test]
    fn title_and_toc_properties() {
        let xml = blank();
        let title = style(&xml, "Title");
        assert_has(
            title,
            r#"<w:spacing w:val="-10"/><w:kern w:val="28"/><w:sz w:val="56"/>"#,
        );
        assert_has(title, r#"<w:b w:val="0"/><w:bCs w:val="0"/>"#);
        assert_has(
            style(&xml, "TOC2"),
            concat!(
                r#"<w:pPr><w:snapToGrid w:val="0"/>"#,
                r#"<w:spacing w:after="100" w:line="276" w:lineRule="auto"/>"#,
                r#"<w:ind w:left="200"/><w:jc w:val="both"/></w:pPr>"#,
            ),
        );
        assert_has(
            style(&xml, "TOC1"),
            r#"<w:name w:val="toc 1"/><w:basedOn w:val="Normal"/><w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:pPr>"#,
        );
    }

    #[test]
    fn data_table_style() {
        let xml = blank();
        let table = style(&xml, "DataTable");
        let edge = r#" w:val="single" w:sz="2" w:space="0" w:color="000000"/>"#;
        let borders: String = ["top", "left", "bottom", "right", "insideH", "insideV"]
            .iter()
            .map(|side| format!("<w:{side}{edge}"))
            .collect();
        assert_has(
            table,
            &format!(
                concat!(
                    r#"<w:uiPriority w:val="59"/><w:qFormat/><w:tblPr><w:tblBorders>{}</w:tblBorders>"#,
                    "<w:tblCellMar>",
                    r#"<w:top w:w="0" w:type="dxa"/><w:left w:w="10" w:type="dxa"/>"#,
                    r#"<w:bottom w:w="0" w:type="dxa"/><w:right w:w="10" w:type="dxa"/>"#,
                    "</w:tblCellMar></w:tblPr>",
                    r#"<w:tblStylePr w:type="firstRow"><w:rPr><w:b/><w:bCs/></w:rPr>"#,
                    r#"<w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/></w:tcPr>"#,
                    "</w:tblStylePr></w:style>",
                ),
                borders
            ),
        );
        assert_has(
            style(&xml, "TableNormal"),
            r#"<w:left w:w="108" w:type="dxa"/>"#,
        );

        let xml = sample("  Normal: {}\n");
        assert_has(
            style(&xml, "DataTable"),
            r#"<w:tblStylePr w:type="firstRow"><w:tcPr>"#,
        );
    }

    #[test]
    fn inverted_and_false_values() {
        let xml = sample(
            r#"
  Normal:
    line: 14pt
    hyphenate: true
    proofing: true
    keep_next: false
    snap_to_grid: true
    bold: false
"#,
        );
        let normal = style(&xml, "Normal");
        assert_has(
            normal,
            concat!(
                r#"<w:pPr><w:keepNext w:val="0"/><w:suppressAutoHyphens w:val="0"/><w:snapToGrid/>"#,
                r#"<w:spacing w:line="280" w:lineRule="exact"/></w:pPr>"#,
                r#"<w:rPr><w:b w:val="0"/><w:bCs w:val="0"/><w:noProof w:val="0"/></w:rPr>"#,
            ),
        );
        assert_has(
            &xml,
            r#"<w:pPrDefault><w:pPr><w:spacing w:line="280" w:lineRule="exact"/></w:pPr></w:pPrDefault>"#,
        );

        let xml = sample("  Normal: {}\n");
        assert_lacks(&xml, "w:pPrDefault");
        assert_has(
            &xml,
            r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>"#,
        );
    }

    #[test]
    fn values_are_escaped() {
        let params = DocxParams::from_yaml(&format!(
            "{}  Normal: {{font: amp}}\n",
            BASE.replace("mono: PT Mono", "mono: PT Mono, amp: \"A&B <C>\"")
        ))
        .unwrap();
        let xml = xml(&params, "x\"y");
        assert_has(&xml, r#"w:ascii="A&amp;B &lt;C&gt;""#);
        assert_has(&xml, r#"<w:lang w:val="x&quot;y"/>"#);
    }

    #[test]
    fn more_builtin_styles() {
        let xml = sample(concat!(
            "  Normal: {}\n",
            "  Heading1: {based_on: Normal}\n",
            "  TOCHeading: {based_on: Heading1}\n",
            "  Strong: {bold: true}\n",
            "  Emphasis: {italic: true}\n",
        ));
        assert_has(
            &xml,
            concat!(
                r#"<w:style w:type="paragraph" w:styleId="TOCHeading">"#,
                r#"<w:name w:val="TOC Heading"/><w:basedOn w:val="Heading1"/>"#,
                r#"<w:uiPriority w:val="39"/><w:unhideWhenUsed/><w:qFormat/>"#,
                r#"<w:pPr><w:outlineLvl w:val="9"/></w:pPr></w:style>"#,
            ),
        );
        assert_has(
            &xml,
            concat!(
                r#"<w:style w:type="character" w:styleId="Strong">"#,
                r#"<w:name w:val="Strong"/><w:basedOn w:val="DefaultParagraphFont"/>"#,
                r#"<w:uiPriority w:val="22"/><w:qFormat/><w:rPr><w:b/><w:bCs/></w:rPr></w:style>"#,
            ),
        );
        assert_has(
            &xml,
            concat!(
                r#"<w:style w:type="character" w:styleId="Emphasis">"#,
                r#"<w:name w:val="Emphasis"/><w:basedOn w:val="DefaultParagraphFont"/>"#,
                r#"<w:uiPriority w:val="20"/><w:qFormat/><w:rPr><w:i/><w:iCs/></w:rPr></w:style>"#,
            ),
        );
    }

    /// The child order of each checked element in the OOXML schema
    /// (ECMA-376 part 1).
    const SCHEMA_ORDER: &[(&str, &[&str])] = &[
        ("w:styles", &["w:docDefaults", "w:latentStyles", "w:style"]),
        ("w:docDefaults", &["w:rPrDefault", "w:pPrDefault"]),
        (
            "w:style",
            &[
                "w:name",
                "w:aliases",
                "w:basedOn",
                "w:next",
                "w:link",
                "w:autoRedefine",
                "w:hidden",
                "w:uiPriority",
                "w:semiHidden",
                "w:unhideWhenUsed",
                "w:qFormat",
                "w:locked",
                "w:personal",
                "w:personalCompose",
                "w:personalReply",
                "w:rsid",
                "w:pPr",
                "w:rPr",
                "w:tblPr",
                "w:trPr",
                "w:tcPr",
                "w:tblStylePr",
            ],
        ),
        (
            "w:pPr",
            &[
                "w:pStyle",
                "w:keepNext",
                "w:keepLines",
                "w:pageBreakBefore",
                "w:framePr",
                "w:widowControl",
                "w:numPr",
                "w:suppressLineNumbers",
                "w:pBdr",
                "w:shd",
                "w:tabs",
                "w:suppressAutoHyphens",
                "w:kinsoku",
                "w:wordWrap",
                "w:overflowPunct",
                "w:topLinePunct",
                "w:autoSpaceDE",
                "w:autoSpaceDN",
                "w:bidi",
                "w:adjustRightInd",
                "w:snapToGrid",
                "w:spacing",
                "w:ind",
                "w:contextualSpacing",
                "w:mirrorIndents",
                "w:suppressOverlap",
                "w:jc",
                "w:textDirection",
                "w:textAlignment",
                "w:textboxTightWrap",
                "w:outlineLvl",
                "w:divId",
                "w:cnfStyle",
            ],
        ),
        (
            "w:rPr",
            &[
                "w:rStyle",
                "w:rFonts",
                "w:b",
                "w:bCs",
                "w:i",
                "w:iCs",
                "w:caps",
                "w:smallCaps",
                "w:strike",
                "w:dstrike",
                "w:outline",
                "w:shadow",
                "w:emboss",
                "w:imprint",
                "w:noProof",
                "w:snapToGrid",
                "w:vanish",
                "w:webHidden",
                "w:color",
                "w:spacing",
                "w:w",
                "w:kern",
                "w:position",
                "w:sz",
                "w:szCs",
                "w:highlight",
                "w:u",
                "w:effect",
                "w:bdr",
                "w:shd",
                "w:fitText",
                "w:vertAlign",
                "w:rtl",
                "w:cs",
                "w:em",
                "w:lang",
                "w:eastAsianLayout",
                "w:specVanish",
                "w:oMath",
            ],
        ),
        (
            "w:tblPr",
            &[
                "w:tblStyle",
                "w:tblpPr",
                "w:tblOverlap",
                "w:bidiVisual",
                "w:tblStyleRowBandSize",
                "w:tblStyleColBandSize",
                "w:tblW",
                "w:jc",
                "w:tblCellSpacing",
                "w:tblInd",
                "w:tblBorders",
                "w:shd",
                "w:tblLayout",
                "w:tblCellMar",
                "w:tblLook",
            ],
        ),
        (
            "w:tblStylePr",
            &["w:pPr", "w:rPr", "w:tblPr", "w:trPr", "w:tcPr"],
        ),
        (
            "w:tcPr",
            &[
                "w:cnfStyle",
                "w:tcW",
                "w:gridSpan",
                "w:hMerge",
                "w:vMerge",
                "w:tcBorders",
                "w:shd",
                "w:noWrap",
                "w:tcMar",
                "w:textDirection",
                "w:tcFitText",
                "w:vAlign",
                "w:hideMark",
            ],
        ),
        (
            "w:pBdr",
            &[
                "w:top",
                "w:left",
                "w:bottom",
                "w:right",
                "w:between",
                "w:bar",
            ],
        ),
        (
            "w:tblBorders",
            &[
                "w:top",
                "w:left",
                "w:bottom",
                "w:right",
                "w:insideH",
                "w:insideV",
            ],
        ),
        ("w:tblCellMar", &["w:top", "w:left", "w:bottom", "w:right"]),
    ];

    /// Checks that the children of each element in [`SCHEMA_ORDER`] have
    /// the schema order, and that each child is in the order list.
    #[track_caller]
    fn assert_schema_order(xml: &str) {
        // The open elements, with the order index of the last child.
        let mut open: Vec<(String, usize)> = Vec::new();
        let mut reader = Reader::from_str(xml);
        loop {
            let (start, is_empty) = match reader.read_event().unwrap() {
                Event::Start(e) => (e, false),
                Event::Empty(e) => (e, true),
                Event::End(_) => {
                    open.pop();
                    continue;
                }
                Event::Eof => break,
                _ => continue,
            };
            let name = String::from_utf8(start.name().as_ref().to_vec()).unwrap();
            if let Some((parent, last)) = open.last_mut() {
                if let Some((_, order)) = SCHEMA_ORDER.iter().find(|(p, _)| p == parent) {
                    let index = order
                        .iter()
                        .position(|child| *child == name)
                        .unwrap_or_else(|| panic!("{name} is not a child of {parent}"));
                    assert!(index >= *last, "{name} is after a later child in {parent}");
                    *last = index;
                }
            }
            if !is_empty {
                open.push((name, 0));
            }
        }
    }

    #[test]
    fn schema_order() {
        assert_schema_order(&blank());
        let xml = sample(concat!(
            "  Normal:\n",
            "    font: mono\n",
            "    size: 11pt\n",
            "    bold: true\n",
            "    italic: true\n",
            "    color: accent\n",
            "    align: justify\n",
            "    before: 6pt\n",
            "    after: 6pt\n",
            "    line: 1.15\n",
            "    indent: 12pt\n",
            "    keep_next: true\n",
            "    fill: accent\n",
            "    border: {width: 0.5pt, color: accent, space: 1pt 4pt}\n",
            "    hyphenate: false\n",
            "    char_spacing: -0.5pt\n",
            "    kern: 14pt\n",
            "    proofing: false\n",
            "    snap_to_grid: false\n",
            "    contextual_spacing: true\n",
            "  Heading1: {based_on: Normal}\n",
            "  InlineCode:\n",
            "    font: mono\n",
            "    size: 10pt\n",
            "    bold: true\n",
            "    italic: true\n",
            "    color: accent\n",
            "    fill: accent\n",
            "    char_spacing: 1pt\n",
            "    kern: 10pt\n",
            "    proofing: false\n",
        ));
        // Each property of the fixture is in the output.
        let normal = style(&xml, "Normal");
        for part in [
            "<w:keepNext/>",
            "<w:pBdr><w:top ",
            "<w:shd ",
            "<w:suppressAutoHyphens/>",
            "<w:snapToGrid ",
            "<w:spacing w:before",
            "<w:ind ",
            "<w:contextualSpacing/>",
            "<w:jc ",
            "<w:rFonts ",
            "<w:b/>",
            "<w:i/>",
            "<w:noProof/>",
            "<w:color ",
            "<w:spacing w:val",
            "<w:kern ",
            "<w:sz ",
        ] {
            assert_has(normal, part);
        }
        assert_has(style(&xml, "Heading1"), "<w:outlineLvl ");
        for part in [
            "<w:rFonts ",
            "<w:b/>",
            "<w:i/>",
            "<w:noProof/>",
            "<w:color ",
        ] {
            assert_has(style(&xml, "InlineCode"), part);
        }
        assert_has(
            style(&xml, "InlineCode"),
            r#"<w:spacing w:val="20"/><w:kern w:val="20"/><w:sz w:val="20"/><w:szCs w:val="20"/><w:shd "#,
        );
        assert_schema_order(&xml);
    }
}
