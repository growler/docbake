use std::fmt::Write as _;

use anyhow::{anyhow, Result};
use rushdown::as_extension_data;
use rushdown::ast::{Arena, KindData, LinkKind, NodeRef, TextQualifier};
use rushdown_footnote::FootnoteReference;

use crate::html::is_inline_image;

use super::types::RenderContext;
use super::xml::{cdata_escape, xml_attr_escape, xml_escape};
use super::ConfluenceRenderer;

impl<'a> ConfluenceRenderer<'a> {
    pub(super) fn render_inline_children(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        for child in arena[node_ref].children(arena) {
            self.render_inline(out, context, child)?;
        }
        Ok(())
    }

    fn render_inline(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let prepared = context.prepared;
        let scope = context.scope;
        let arena = context.arena;
        let source = context.source;
        let footnotes = context.footnotes;
        self.render_anchor_if_node_has_id(out, arena, source, node_ref)?;
        match arena[node_ref].kind_data() {
            KindData::Text(text) => {
                out.push_str(&xml_escape(text.str(source)));
                if text.has_qualifiers(TextQualifier::HARD_LINE_BREAK) {
                    out.push_str("<br />\n");
                } else if text.has_qualifiers(TextQualifier::SOFT_LINE_BREAK) {
                    out.push('\n');
                }
            }
            KindData::CodeSpan(cs) => {
                write!(out, "<code>{}</code>", xml_escape(&cs.str(source)))?;
            }
            KindData::Emphasis(_) => {
                out.push_str("<em>");
                self.render_inline_children(out, context, node_ref)?;
                out.push_str("</em>");
            }
            KindData::Strong(_) => {
                out.push_str("<strong>");
                self.render_inline_children(out, context, node_ref)?;
                out.push_str("</strong>");
            }
            KindData::Strikethrough(_) => {
                // Confluence doesn't support <del>, use inline style.
                out.push_str("<span style=\"text-decoration: line-through;\">");
                self.render_inline_children(out, context, node_ref)?;
                out.push_str("</span>");
            }
            KindData::Link(link) => {
                let dest = prepared
                    .graph()
                    .link_target(scope.doc_id, node_ref)
                    .ok_or_else(|| {
                        anyhow!(
                            "missing resolved link target for `{}` in {}",
                            link.destination_str(source),
                            prepared.graph().document(scope.doc_id).path.display()
                        )
                    })?
                    .to_string();

                match link.link_kind() {
                    LinkKind::Auto(auto) => {
                        if dest.starts_with('#') {
                            self.render_internal_link(
                                out,
                                &dest,
                                &xml_escape(auto.text_str(source)),
                            )?;
                        } else {
                            write!(
                                out,
                                "<a href=\"{}\">{}</a>",
                                xml_attr_escape(&dest),
                                xml_escape(auto.text_str(source))
                            )?;
                        }
                    }
                    _ => {
                        if dest.starts_with('#') {
                            // Internal anchor link -> ac:link with anchor.
                            let mut link_text = String::new();
                            self.render_inline_children(&mut link_text, context, node_ref)?;
                            self.render_internal_link(out, &dest, &link_text)?;
                        } else {
                            // External link.
                            write!(out, "<a href=\"{}\">", xml_attr_escape(&dest))?;
                            self.render_inline_children(out, context, node_ref)?;
                            out.push_str("</a>");
                        }
                    }
                }
            }
            KindData::Image(image) => {
                let dest = prepared
                    .graph()
                    .image_target(scope.doc_id, node_ref)
                    .ok_or_else(|| {
                        anyhow!(
                            "missing resolved image target for `{}` in {}",
                            image.destination_str(source),
                            prepared.graph().document(scope.doc_id).path.display()
                        )
                    })?
                    .to_string();
                let alt = collect_image_alt_text(arena, node_ref, source);
                let attrs = arena[node_ref].attributes();
                let width = attrs.get("width").map(|v| v.str(source).to_string());
                let height = attrs.get("height").map(|v| v.str(source).to_string());
                let inline = is_inline_image(arena, node_ref);

                if dest.contains("://") {
                    // External image URL.
                    let is_svg = dest.ends_with(".svg") || dest.contains(".svg?");
                    out.push_str("<ac:image");
                    // SVG data unavailable for external URLs.
                    self.write_image_size_attrs(
                        out,
                        width.as_deref(),
                        height.as_deref(),
                        is_svg,
                        inline,
                        None,
                    );
                    if !alt.is_empty() {
                        write!(out, " ac:alt=\"{}\"", xml_attr_escape(&alt))?;
                    }
                    write!(
                        out,
                        "><ri:url ri:value=\"{}\" /></ac:image>",
                        xml_attr_escape(&dest)
                    )?;
                } else {
                    // Local image -> attachment.
                    let filename = self.resolve_image_attachment(&dest, prepared)?;
                    let is_svg = filename.ends_with(".svg");
                    // For local SVGs, look up the attachment data so we can
                    // extract the intrinsic width from the SVG root element.
                    let svg_data: Option<Vec<u8>> = if is_svg {
                        self.attachments
                            .borrow()
                            .iter()
                            .find(|a| a.filename == filename)
                            .map(|a| a.data.clone())
                    } else {
                        None
                    };
                    out.push_str("<ac:image");
                    self.write_image_size_attrs(
                        out,
                        width.as_deref(),
                        height.as_deref(),
                        is_svg,
                        inline,
                        svg_data.as_deref(),
                    );
                    if !alt.is_empty() {
                        write!(out, " ac:alt=\"{}\"", xml_attr_escape(&alt))?;
                    }
                    write!(
                        out,
                        "><ri:attachment ri:filename=\"{}\" /></ac:image>",
                        xml_attr_escape(&filename)
                    )?;
                }
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FootnoteReference) =>
            {
                let footnote = as_extension_data!(arena, node_ref, FootnoteReference);
                let label = footnote.label().str(source).to_string();
                if let Some(entry) = footnotes
                    .iter()
                    .find(|e| e.label == label && e.doc_id == scope.doc_id)
                {
                    let idx = entry.index;
                    write!(
                        out,
                        "<sup><ac:link ac:anchor=\"fn:{idx}\"><ac:plain-text-link-body>\
                         <![CDATA[{idx}]]></ac:plain-text-link-body></ac:link></sup>"
                    )?;
                }
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::bracketed_span::BracketedSpan
                ) =>
            {
                out.push_str("<span>");
                self.render_inline_children(out, context, node_ref)?;
                out.push_str("</span>");
            }
            KindData::Extension(_) => {}
            _ => {}
        }
        Ok(())
    }

    fn render_internal_link(&self, out: &mut String, dest: &str, link_body: &str) -> Result<()> {
        let anchor = dest.trim_start_matches('#');
        write!(
            out,
            "<ac:link ac:anchor=\"{}\"><ac:plain-text-link-body>\
             <![CDATA[{}]]></ac:plain-text-link-body></ac:link>",
            xml_attr_escape(anchor),
            cdata_escape(link_body)
        )?;
        Ok(())
    }
}

/// Extract the alt text from an Image node by collecting text from its
/// inline children (the `![alt text](url)` content).
fn collect_image_alt_text(arena: &Arena, node_ref: NodeRef, source: &str) -> String {
    let mut alt = String::new();
    for child in arena[node_ref].children(arena) {
        match arena[child].kind_data() {
            KindData::Text(text) => alt.push_str(text.str(source)),
            KindData::CodeSpan(cs) => alt.push_str(&cs.str(source)),
            _ => {}
        }
    }
    alt
}
