// The zipper in this file is a copy of `src/zipper/mod.rs` of docx-rs 0.4.20
// (https://github.com/bokuweb/docx-rs), Copyright (c) 2020 bokuweb, MIT
// License.

//! DOCX package writer.
//!
//! [`Package`] holds the parts that `docx-rs` makes, with `word/styles.xml`
//! and `word/fontTable.xml` from the style parameters of the theme, and
//! `docProps/core.xml` with the document title. It adds the table
//! properties that `docx-rs` cannot write to `word/document.xml`. It writes
//! the parts to a ZIP file in the entry order of the `docx-rs` zipper. The
//! XML parts are Deflated. The images in `word/media/` and the directories
//! are Stored. All entries have the time 1980-01-01 00:00, thus the same
//! input gives the same file.

use std::io::{Cursor, Seek, Write};

use anyhow::{Context, Result};
use docx_rs::{Docx, XMLDocx};
use zip::write::FileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use super::doc_props::core_xml;
use super::font_table::font_table_xml;
use super::params::DocxParams;
use super::stylesheet::styles_xml;
use super::tables::{splice_tables, TableLayout};

/// The parts of a DOCX file, ready to be packed.
pub(crate) struct Package {
    xml: XMLDocx,
}

impl Package {
    /// Builds the parts of `docx` and replaces the style sheet and the font
    /// table with the parts made from `params`, and the core properties
    /// with the parts made from `title`. Adds the table properties from
    /// `tables` to the document part. `lang` must be a valid language tag.
    pub(super) fn new(
        docx: Docx,
        params: &DocxParams,
        lang: &str,
        title: &str,
        tables: &[TableLayout],
    ) -> Result<Self> {
        let mut xml = docx.build();
        xml.styles = styles_xml(params, lang);
        xml.font_table = font_table_xml(params);
        xml.doc_props.core = core_xml(title);
        xml.document = splice_tables(std::mem::take(&mut xml.document), tables)
            .context("failed to add table properties")?;
        Ok(Self { xml })
    }

    /// Writes the DOCX file to `out` and gives `out` back.
    pub(crate) fn write_to<W: Write + Seek>(self, out: W) -> Result<W> {
        zip(out, self.xml).context("failed to pack DOCX")
    }

    /// Gives the DOCX file as bytes.
    pub(crate) fn into_bytes(self) -> Result<Vec<u8>> {
        Ok(self.write_to(Cursor::new(Vec::new()))?.into_inner())
    }
}

#[cfg(test)]
impl Package {
    /// Gives the parts before they are packed.
    pub(super) fn xml(&self) -> &XMLDocx {
        &self.xml
    }
}

/// Writes the parts to a ZIP file.
fn zip<W>(w: W, xml: XMLDocx) -> zip::result::ZipResult<W>
where
    W: Write + Seek,
{
    let mut zip = ZipWriter::new(w);

    let stored = FileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o755);
    let options = stored.compression_method(CompressionMethod::Deflated);

    zip.add_directory("word/", stored)?;
    zip.add_directory("word/_rels", stored)?;
    zip.add_directory("_rels/", stored)?;
    zip.add_directory("docProps/", stored)?;

    zip.start_file("[Content_Types].xml", options)?;
    zip.write_all(&xml.content_type)?;
    zip.start_file("_rels/.rels", options)?;
    zip.write_all(&xml.rels)?;
    zip.start_file("docProps/app.xml", options)?;
    zip.write_all(&xml.doc_props.app)?;
    zip.start_file("docProps/core.xml", options)?;
    zip.write_all(&xml.doc_props.core)?;
    zip.start_file("docProps/custom.xml", options)?;
    zip.write_all(&xml.doc_props.custom)?;
    zip.start_file("word/_rels/document.xml.rels", options)?;
    zip.write_all(&xml.document_rels)?;
    zip.start_file("word/document.xml", options)?;
    zip.write_all(&xml.document)?;
    zip.start_file("word/styles.xml", options)?;
    zip.write_all(&xml.styles)?;
    zip.start_file("word/settings.xml", options)?;
    zip.write_all(&xml.settings)?;
    zip.start_file("word/fontTable.xml", options)?;
    zip.write_all(&xml.font_table)?;
    zip.start_file("word/comments.xml", options)?;
    zip.write_all(&xml.comments)?;
    zip.start_file("word/numbering.xml", options)?;
    zip.write_all(&xml.numberings)?;
    zip.start_file("word/commentsExtended.xml", options)?;
    zip.write_all(&xml.comments_extended)?;
    zip.start_file("word/footnotes.xml", options)?;
    zip.write_all(&xml.footnotes)?;

    for (i, h) in xml.headers.iter().enumerate() {
        zip.start_file(format!("word/header{}.xml", i + 1), options)?;
        zip.write_all(h)?;

        if let Some(rels) = xml.header_rels.get(i) {
            zip.start_file(format!("word/_rels/header{}.xml.rels", i + 1), options)?;
            zip.write_all(rels)?;
        }
    }

    for (i, h) in xml.footers.iter().enumerate() {
        zip.start_file(format!("word/footer{}.xml", i + 1), options)?;
        zip.write_all(h)?;

        if let Some(rels) = xml.footer_rels.get(i) {
            zip.start_file(format!("word/_rels/footer{}.xml.rels", i + 1), options)?;
            zip.write_all(rels)?;
        }
    }

    if !xml.media.is_empty() {
        zip.add_directory("word/media/", stored)?;
        for m in xml.media {
            // docx-rs embeds all images as PNG.
            zip.start_file(format!("word/media/{}.png", m.0), stored)?;
            zip.write_all(&m.1)?;
        }
    }

    // docx-rs supports only taskpanes.
    if let Some(taskpanes) = xml.taskpanes {
        zip.add_directory("word/webextensions/", stored)?;
        zip.start_file("word/webextensions/taskpanes.xml", options)?;
        zip.write_all(&taskpanes)?;

        zip.add_directory("word/webextensions/_rels", stored)?;
        zip.start_file("word/webextensions/_rels/taskpanes.xml.rels", options)?;
        zip.write_all(&xml.taskpanes_rels)?;

        for (i, ext) in xml.web_extensions.iter().enumerate() {
            zip.start_file(
                format!("word/webextensions/webextension{}.xml", i + 1),
                options,
            )?;
            zip.write_all(ext)?;
        }
    }

    if !xml.custom_items.is_empty() {
        zip.add_directory("customXml/_rels", stored)?;
    }

    for (i, item) in xml.custom_items.into_iter().enumerate() {
        let n = i + 1;
        zip.start_file(format!("customXml/_rels/item{}.xml.rels", n), options)?;
        zip.write_all(&xml.custom_item_rels[i])?;
        zip.start_file(format!("customXml/item{}.xml", n), options)?;
        zip.write_all(&item)?;
        zip.start_file(format!("customXml/itemProps{}.xml", n), options)?;
        zip.write_all(&xml.custom_item_props[i])?;
    }

    zip.finish()
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use docx_rs::XMLDocProps;
    use zip::ZipArchive;

    use super::*;
    use crate::theme::Theme;

    /// An entry of a packed file: name, compression, directory flag, time
    /// (year, month, day, hour, minute, second), permission bits and data.
    type Entry = (String, CompressionMethod, bool, [u16; 6], u32, Vec<u8>);

    fn part(name: &str) -> Vec<u8> {
        format!("<{name}/>").into_bytes()
    }

    /// Parts with a distinct content each, with all optional parts set.
    fn sample_xml() -> XMLDocx {
        XMLDocx {
            content_type: part("content_type"),
            rels: part("rels"),
            doc_props: XMLDocProps {
                app: part("app"),
                core: part("core"),
                custom: part("custom"),
            },
            styles: part("styles"),
            document: part("document"),
            comments: part("comments"),
            document_rels: part("document_rels"),
            settings: part("settings"),
            font_table: part("font_table"),
            numberings: part("numberings"),
            media: vec![
                ("rIdImage1".to_string(), b"png1".to_vec()),
                ("rIdImage2".to_string(), b"png2".to_vec()),
            ],
            headers: vec![part("header1"), part("header2")],
            header_rels: vec![part("header_rels1")],
            footers: vec![part("footer1")],
            footer_rels: vec![part("footer_rels1")],
            comments_extended: part("comments_extended"),
            taskpanes: Some(part("taskpanes")),
            taskpanes_rels: part("taskpanes_rels"),
            web_extensions: vec![part("web_extension1")],
            custom_items: vec![part("item1"), part("item2")],
            custom_item_rels: vec![part("item_rels1"), part("item_rels2")],
            custom_item_props: vec![part("item_props1"), part("item_props2")],
            footnotes: part("footnotes"),
        }
    }

    fn entries(bytes: Vec<u8>) -> Vec<Entry> {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..archive.len())
            .map(|i| {
                let mut file = archive.by_index(i).unwrap();
                let t = file.last_modified();
                let time = [
                    t.year(),
                    t.month().into(),
                    t.day().into(),
                    t.hour().into(),
                    t.minute().into(),
                    t.second().into(),
                ];
                let mode = file.unix_mode().unwrap_or_default() & 0o777;
                let mut data = Vec::new();
                file.read_to_end(&mut data).unwrap();
                (
                    file.name().to_string(),
                    file.compression(),
                    file.is_dir(),
                    time,
                    mode,
                    data,
                )
            })
            .collect()
    }

    fn pack(xml: XMLDocx) -> Vec<Entry> {
        let out = zip(Cursor::new(Vec::new()), xml).unwrap();
        entries(out.into_inner())
    }

    #[test]
    fn entries_match_docx_rs_zipper() {
        let mut expected = Cursor::new(Vec::new());
        sample_xml().pack(&mut expected).unwrap();
        let expected = entries(expected.into_inner());
        let actual = pack(sample_xml());

        let names = |e: &[Entry]| e.iter().map(|e| e.0.clone()).collect::<Vec<_>>();
        assert_eq!(names(&actual), names(&expected));
        assert!(names(&actual).contains(&"word/media/rIdImage2.png".to_string()));
        assert!(names(&actual).contains(&"customXml/itemProps2.xml".to_string()));
        for (a, e) in actual.iter().zip(&expected) {
            assert_eq!(a.2, e.2, "{}", a.0);
            assert_eq!(a.5, e.5, "{}", a.0);
        }
    }

    #[test]
    fn compression_time_and_permissions() {
        for (name, compression, is_dir, time, mode, _) in pack(sample_xml()) {
            let stored = is_dir || name.starts_with("word/media/");
            let method = if stored {
                CompressionMethod::Stored
            } else {
                CompressionMethod::Deflated
            };
            assert_eq!(compression, method, "{name}");
            assert_eq!(time, [1980, 1, 1, 0, 0, 0], "{name}");
            assert_eq!(mode, 0o755, "{name}");
        }
    }

    #[test]
    fn generated_parts_replace_docx_rs_parts() {
        let params = Theme::Embedded("blank").docx_params().unwrap();
        let bytes = Package::new(Docx::new(), &params, "ru-RU", "Title", &[])
            .unwrap()
            .into_bytes()
            .unwrap();
        let entries = entries(bytes);
        let data = |name: &str| &entries.iter().find(|e| e.0 == name).unwrap().5;
        assert_eq!(data("word/styles.xml"), &styles_xml(&params, "ru-RU"));
        assert_eq!(data("word/fontTable.xml"), &font_table_xml(&params));
        assert_eq!(data("docProps/core.xml"), &core_xml("Title"));
    }
}
