use std::fmt::Write as _;

use anyhow::Result;
use rushdown::as_extension_data;
use rushdown_footnote::FootnoteReference;

use crate::graph::PreparedDocument;

use super::types::{FootnoteEntry, RenderContext, Scope};
use super::ConfluenceRenderer;

impl<'a> ConfluenceRenderer<'a> {
    pub(super) fn collect_footnotes(
        &self,
        prepared: &PreparedDocument,
    ) -> Result<Vec<FootnoteEntry>> {
        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut index = 1usize;
        let rendered = prepared
            .graph()
            .rendered_nodes_preorder_visible(&self.profile)?;
        for (doc_id, node_ref, _) in rendered {
            let doc = prepared.graph().document(doc_id);
            if !rushdown::matches_extension_kind!(&doc.arena, node_ref, FootnoteReference) {
                continue;
            }
            let footnote = as_extension_data!(&doc.arena, node_ref, FootnoteReference);
            let label = footnote.label().str(&doc.source).to_string();
            let key = (doc_id, label.clone());
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);
            entries.push(FootnoteEntry {
                label,
                doc_id,
                index,
            });
            index += 1;
        }
        Ok(entries)
    }

    pub(super) fn render_footnotes_section(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        footnotes: &[FootnoteEntry],
    ) -> Result<()> {
        out.push_str("<hr />\n<ol>\n");
        for entry in footnotes {
            let doc = prepared.graph().document(entry.doc_id);
            write!(
                out,
                "<li><ac:structured-macro ac:name=\"anchor\">\
                 <ac:parameter ac:name=\"\">fn:{}</ac:parameter>\
                 </ac:structured-macro>",
                entry.index
            )?;
            if let Some(def) = doc.footnote_definition(&entry.label) {
                let context = RenderContext {
                    prepared,
                    scope: Scope {
                        doc_id: entry.doc_id,
                    },
                    arena: &doc.arena,
                    source: &doc.source,
                    heading_shift: 0,
                    footnotes,
                };
                for child in doc.arena[def.node_ref].children(&doc.arena) {
                    self.render_block(out, &context, child)?;
                }
            }
            out.push_str("</li>\n");
        }
        out.push_str("</ol>\n");
        Ok(())
    }
}
