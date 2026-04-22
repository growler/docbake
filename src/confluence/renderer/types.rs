use rushdown::ast::Arena;

use crate::graph::PreparedDocument;

/// A single attachment to be uploaded alongside the page.
#[derive(Debug)]
pub(crate) struct ConfluenceAttachment {
    /// Filename in Confluence (e.g. `"diagram-0.png"` or `"photo.jpg"`).
    pub filename: String,
    /// Raw file bytes.
    pub data: Vec<u8>,
    /// `"blake3:<hex>"` content hash stored in the attachment comment
    /// field for change detection.
    pub content_hash: String,
}

/// Result of rendering a document for Confluence.
pub(crate) struct ConfluenceDocument {
    /// XHTML storage-format body (without `<html>` / `<body>` wrapper).
    pub body: String,
    /// Attachments collected during rendering.
    pub attachments: Vec<ConfluenceAttachment>,
}

#[derive(Clone, Copy)]
pub(super) struct Scope {
    pub(super) doc_id: usize,
}

#[derive(Clone, Copy)]
pub(super) struct RenderContext<'a> {
    pub(super) prepared: &'a PreparedDocument,
    pub(super) scope: Scope,
    pub(super) arena: &'a Arena,
    pub(super) source: &'a str,
    pub(super) heading_shift: usize,
    pub(super) footnotes: &'a [FootnoteEntry],
}

pub(super) struct FootnoteEntry {
    pub(super) label: String,
    pub(super) doc_id: usize,
    pub(super) index: usize,
}
