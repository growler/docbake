use std::fmt::Write as _;

use anyhow::Result;
use rushdown::as_extension_data;
use rushdown::ast::{Arena, KindData, LinkKind, NodeRef, TextQualifier};
use rushdown_footnote::FootnoteReference;

use super::source_recovery::{
    backtick_fence, code_span_has_explicit_padding, emphasis_marker_from_source, escape_md_title,
    format_inline_destination, hard_line_break_marker, original_reference_link, render_node_attrs,
};
use super::MarkdownFmtRenderer;

impl MarkdownFmtRenderer {
    pub(super) fn render_inline_children(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        for child in arena[node_ref].children(arena) {
            self.render_inline(out, arena, child, source)?;
        }
        Ok(())
    }

    fn render_inline(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        match arena[node_ref].kind_data() {
            KindData::Text(text) => {
                out.push_str(text.str(source));
                if text.has_qualifiers(TextQualifier::HARD_LINE_BREAK) {
                    out.push_str(hard_line_break_marker(arena, node_ref, source));
                } else if text.has_qualifiers(TextQualifier::SOFT_LINE_BREAK) {
                    out.push(' ');
                }
            }
            KindData::CodeSpan(cs) => {
                let text = cs.value_str(source).to_string();
                let fence = backtick_fence(&text);
                let preserve_padding = code_span_has_explicit_padding(arena, node_ref, source);
                if preserve_padding || text.starts_with('`') || text.ends_with('`') {
                    write!(out, "{fence} {text} {fence}")?;
                } else {
                    write!(out, "{fence}{text}{fence}")?;
                }
            }
            KindData::Emphasis(emphasis) if emphasis.level() == 1 => {
                let marker = emphasis_marker_from_source(arena, node_ref, source);
                out.push(marker);
                self.render_inline_children(out, arena, node_ref, source)?;
                out.push(marker);
            }
            KindData::Emphasis(_) => {
                let marker = emphasis_marker_from_source(arena, node_ref, source);
                out.push_str(&marker.to_string().repeat(2));
                self.render_inline_children(out, arena, node_ref, source)?;
                out.push_str(&marker.to_string().repeat(2));
            }
            KindData::Strikethrough(_) => {
                out.push_str("~~");
                self.render_inline_children(out, arena, node_ref, source)?;
                out.push_str("~~");
            }
            KindData::Link(link) => match link.link_kind() {
                LinkKind::Auto(auto) => out.push_str(auto.text_str(source)),
                LinkKind::Reference(_) => {
                    if let Some(raw) = original_reference_link(arena, node_ref, source) {
                        out.push_str(&raw);
                    } else {
                        out.push('[');
                        self.render_inline_children(out, arena, node_ref, source)?;
                        out.push(']');
                    }
                }
                _ => {
                    out.push('[');
                    self.render_inline_children(out, arena, node_ref, source)?;
                    out.push(']');
                    out.push('(');
                    out.push_str(&format_inline_destination(link.destination_str(source)));
                    if let Some(title) = link.title() {
                        write!(out, " \"{}\"", escape_md_title(&title.str(source)))?;
                    }
                    out.push(')');
                }
            },
            KindData::Image(image) => {
                out.push_str("![");
                self.render_image_alt_text(out, arena, node_ref, source)?;
                out.push(']');
                out.push('(');
                out.push_str(&format_inline_destination(image.destination_str(source)));
                if let Some(title) = image.title() {
                    write!(out, " \"{}\"", escape_md_title(&title.str(source)))?;
                }
                out.push(')');
                if let Some(attrs) = render_node_attrs(arena, node_ref, source) {
                    out.push_str(&attrs);
                }
            }
            KindData::RawHtml(html) => out.push_str(&html.str(source)),
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FootnoteReference) =>
            {
                let footnote = as_extension_data!(arena, node_ref, FootnoteReference);
                write!(out, "[^{}]", footnote.label().str(source))?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::bracketed_span::BracketedSpan
                ) =>
            {
                out.push('[');
                self.render_inline_children(out, arena, node_ref, source)?;
                out.push(']');
                if let Some(attrs) = render_node_attrs(arena, node_ref, source) {
                    out.push_str(&attrs);
                }
            }
            KindData::Extension(_) => {}
            _ => {}
        }
        Ok(())
    }

    fn render_image_alt_text(
        &self,
        out: &mut String,
        arena: &Arena,
        node_ref: NodeRef,
        source: &str,
    ) -> Result<()> {
        for child in arena[node_ref].children(arena) {
            match arena[child].kind_data() {
                KindData::Text(text) => out.push_str(text.str(source)),
                KindData::CodeSpan(cs) => out.push_str(&cs.value_str(source)),
                _ => self.render_inline(out, arena, child, source)?,
            }
        }
        Ok(())
    }
}
