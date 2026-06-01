//! Post-parse AST transform that normalises hard line breaks inside table cells.
//!
//! Two cell-content idioms get folded into the same `HARD_LINE_BREAK` shape
//! so every renderer (HTML, Typst, Confluence, DOCX, fmt) sees the same
//! thing:
//!
//! 1. **Trailing backslash on a non-final cell line.** For non-last columns
//!    of multiline / grid tables, the cell builder emits a content segment
//!    plus a one-byte separator pointing at the line's `\n`. Rushdown's
//!    inline parser cannot turn a `\` at the end of such a segment into a
//!    `HARD_LINE_BREAK` because the `\` and the `\n` live in different
//!    `Segment`s, so it sees only a `SOFT_LINE_BREAK` on the empty
//!    separator and a plain `Text` ending in `\`. This transform finds
//!    that pair, strips the trailing `\` from the text, and promotes the
//!    following soft break to a hard break. (The last-column case already
//!    works natively, because the segment includes the line's `\n` and
//!    rushdown applies the standard CommonMark `\<nl>` rule directly.)
//!
//! 2. **Inline `<br>` / `<br/>` / `<br />`.** The inline raw-HTML parser
//!    produces a `RawHtml` node; HTML output passes that through verbatim,
//!    but Typst and Confluence drop unknown inline HTML. Replacing the
//!    node with an empty `Text` carrying `HARD_LINE_BREAK` makes every
//!    renderer treat it uniformly as a hard break. The replacement
//!    happens only inside `TableCell`s — outside of tables, raw HTML
//!    keeps its existing semantics.
//!
//! Registered with a small positive priority so it runs after rushdown's
//! built-in `TableAstTransformer` (priority 0). The transform is global
//! over the document: it inspects every `TableCell` regardless of which
//! parser produced it (GFM pipe table, multiline, or grid), since the
//! cell-content semantics are the same.

use rushdown::as_kind_data;
use rushdown::ast::{walk, Arena, KindData, NodeRef, Text, TextQualifier, WalkStatus};
use rushdown::matches_kind;
use rushdown::parser::{
    AnyAstTransformer, AstTransformer, Context, NoParserOptions, Parser, ParserExtension,
    ParserExtensionFn,
};
use rushdown::text::{BasicReader, Index, Reader, Value};

#[derive(Debug)]
struct CellHardBreakTransformer;

impl AstTransformer for CellHardBreakTransformer {
    fn transform(
        &self,
        arena: &mut Arena,
        doc_ref: NodeRef,
        reader: &mut BasicReader,
        _context: &mut Context,
    ) {
        let source: &str = reader.source();
        let mut cells: Vec<NodeRef> = Vec::new();
        walk(arena, doc_ref, &mut |arena: &Arena,
                                   node_ref: NodeRef,
                                   entering: bool|
         -> Result<
            WalkStatus,
            core::convert::Infallible,
        > {
            if entering && matches_kind!(arena, node_ref, TableCell) {
                cells.push(node_ref);
            }
            Ok(WalkStatus::Continue)
        })
        .expect("walk failed");

        for cell in cells {
            process_cell(arena, cell, source);
        }
    }
}

fn process_cell(arena: &mut Arena, cell: NodeRef, source: &str) {
    // Pass 1: replace inline `<br>` / `<br/>` / `<br />` with empty Text(HARD).
    let children: Vec<NodeRef> = arena[cell].children(arena).collect();
    for &child in &children {
        if arena.get(child).is_none() {
            continue;
        }
        let is_br = matches!(arena[child].kind_data(), KindData::RawHtml(raw) if is_br_html(&raw.str(source)));
        if is_br {
            let replacement = arena.new_node(Text::with_qualifiers(
                Value::String(String::new()),
                TextQualifier::HARD_LINE_BREAK,
            ));
            cell.replace_child(arena, child, replacement);
        }
    }

    // Pass 2: fold trailing-backslash patterns. Re-snapshot because pass 1
    // swapped child refs.
    let children: Vec<NodeRef> = arena[cell].children(arena).collect();
    let mut i = 0;
    while i < children.len() {
        let cur = children[i];
        if arena.get(cur).is_none() {
            i += 1;
            continue;
        }
        // Only fold plain Text leaves at the cell's top level.
        if !matches_kind!(arena, cur, Text) {
            i += 1;
            continue;
        }

        let (cur_str_owned, cur_index, cur_has_soft) = {
            let t = as_kind_data!(arena, cur, Text);
            (
                t.str(source).to_owned(),
                t.index().map(|idx| (idx.start(), idx.stop())),
                t.has_qualifiers(TextQualifier::SOFT_LINE_BREAK),
            )
        };

        let marker_len = unescaped_trailing_backslash_len(cur_str_owned.as_bytes());
        if marker_len == 0 {
            i += 1;
            continue;
        }

        let next_opt = children.get(i + 1).copied();
        let next_is_empty_soft = next_opt
            .and_then(|n| arena.get(n).map(|node| (n, node)))
            .map(|(_n, node)| match node.kind_data() {
                KindData::Text(t) => {
                    t.has_qualifiers(TextQualifier::SOFT_LINE_BREAK)
                        && t.str(source).chars().all(char::is_whitespace)
                }
                _ => false,
            })
            .unwrap_or(false);

        let new_len = cur_str_owned.len() - marker_len;
        let new_value = match cur_index {
            Some((start, _stop)) => Value::Index(Index::new(start, start + new_len)),
            None => Value::String(cur_str_owned[..new_len].to_owned()),
        };

        if cur_has_soft || next_is_empty_soft {
            // Replace the current Text with one carrying HARD_LINE_BREAK,
            // dropping SOFT if present.
            let replacement = arena.new_node(Text::with_qualifiers(
                new_value,
                TextQualifier::HARD_LINE_BREAK,
            ));
            cell.replace_child(arena, cur, replacement);
            if next_is_empty_soft {
                if let Some(n) = next_opt {
                    n.remove(arena);
                }
                i += 2;
            } else {
                i += 1;
            }
        } else {
            // No soft-break partner — this is a trailing `\` on the cell's
            // very last text node. Strip the `\` and add a hard break.
            let replacement = arena.new_node(Text::with_qualifiers(
                new_value,
                TextQualifier::HARD_LINE_BREAK,
            ));
            cell.replace_child(arena, cur, replacement);
            i += 1;
        }
    }
}

/// Returns the number of trailing bytes that constitute an unescaped
/// backslash line-break marker, mirroring rushdown's
/// `line[len-2] == '\\' && line[len-3] != '\\'` rule. An odd-length run of
/// trailing `\` => 1 (one byte to trim); even-length run => 0.
fn unescaped_trailing_backslash_len(bytes: &[u8]) -> usize {
    let mut count = 0usize;
    for &b in bytes.iter().rev() {
        if b == b'\\' {
            count += 1;
        } else {
            break;
        }
    }
    if count % 2 == 1 {
        1
    } else {
        0
    }
}

/// Returns `true` if `s` is one of `<br>`, `<br/>`, `<br />` (case-insensitive,
/// arbitrary whitespace before the optional self-closing slash).
fn is_br_html(s: &str) -> bool {
    let s = s.trim();
    if !s.starts_with('<') || !s.ends_with('>') {
        return false;
    }
    let inner = &s[1..s.len() - 1];
    let inner = inner.trim_end_matches('/').trim_end();
    inner.eq_ignore_ascii_case("br")
}

/// Returns a [`ParserExtension`] that registers the cell hard-break
/// transformer. Add this alongside the table parser extensions.
pub(crate) fn table_cell_break_transformer_extension() -> impl ParserExtension {
    ParserExtensionFn::new(|p: &mut Parser| {
        // Priority 10: after rushdown's built-in `TableAstTransformer`
        // (priority 0) so any cell mutations it does are already in place.
        p.add_ast_transformer(make_transformer, NoParserOptions, 10);
    })
}

fn make_transformer() -> AnyAstTransformer {
    AnyAstTransformer::Extension(Box::new(CellHardBreakTransformer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rushdown::ast::NodeRef;
    use rushdown::parser;

    #[test]
    fn unescaped_trailing_backslash_len_simple_cases() {
        assert_eq!(unescaped_trailing_backslash_len(b"foo\\"), 1);
        assert_eq!(unescaped_trailing_backslash_len(b"foo\\\\"), 0);
        assert_eq!(unescaped_trailing_backslash_len(b"foo\\\\\\"), 1);
        assert_eq!(unescaped_trailing_backslash_len(b"foo"), 0);
        assert_eq!(unescaped_trailing_backslash_len(b""), 0);
        assert_eq!(unescaped_trailing_backslash_len(b"\\"), 1);
        assert_eq!(unescaped_trailing_backslash_len(b"\\\\"), 0);
    }

    #[test]
    fn is_br_html_recognises_common_shapes() {
        assert!(is_br_html("<br>"));
        assert!(is_br_html("<br/>"));
        assert!(is_br_html("<br />"));
        assert!(is_br_html("<BR>"));
        assert!(is_br_html("<Br/>"));
        assert!(is_br_html("<br  />"));
        assert!(!is_br_html("<b>"));
        assert!(!is_br_html("<break>"));
        assert!(!is_br_html("br"));
        assert!(!is_br_html("<br attr>"));
    }

    // ── Integration tests ───────────────────────────────────────────────────
    //
    // Parse a real table with the full extension chain wired (gfm + multiline
    // + grid + this transformer) and walk the AST to assert the expected
    // HARD_LINE_BREAK shape lands on cell text nodes.

    fn make_parser() -> parser::Parser {
        parser::Parser::with_extensions(
            parser::Options {
                attributes: false,
                auto_heading_ids: false,
                ..Default::default()
            },
            parser::gfm(parser::GfmOptions::default())
                .and(crate::grid_table::grid_table_parser_extension())
                .and(crate::multiline_table::multiline_table_parser_extension())
                .and(table_cell_break_transformer_extension()),
        )
    }

    /// Returns the first `TableCell` of the first row of the first body in
    /// the document at `doc_ref`, walking past header sections.
    fn first_body_cell(arena: &Arena, doc_ref: NodeRef) -> NodeRef {
        let mut cell = None;
        walk(arena, doc_ref, &mut |arena: &Arena,
                                   n: NodeRef,
                                   entering: bool|
         -> Result<
            WalkStatus,
            core::convert::Infallible,
        > {
            if entering && matches!(arena[n].kind_data(), KindData::TableBody(_)) && cell.is_none()
            {
                // Walk into the body to find the first TableCell.
                walk(arena, n, &mut |arena: &Arena,
                                     m: NodeRef,
                                     entering2: bool|
                 -> Result<
                    WalkStatus,
                    core::convert::Infallible,
                > {
                    if entering2 && matches_kind!(arena, m, TableCell) && cell.is_none() {
                        cell = Some(m);
                    }
                    Ok(WalkStatus::Continue)
                })
                .unwrap();
            }
            Ok(WalkStatus::Continue)
        })
        .unwrap();
        cell.expect("no TableCell found in document")
    }

    fn nth_cell_of_row(arena: &Arena, row: NodeRef, n: usize) -> NodeRef {
        arena[row]
            .children(arena)
            .filter(|c| matches_kind!(arena, *c, TableCell))
            .nth(n)
            .expect("not enough cells in row")
    }

    /// Collect (text, hard_break, soft_break) tuples for direct Text children.
    fn cell_text_summary(arena: &Arena, cell: NodeRef, source: &str) -> Vec<(String, bool, bool)> {
        arena[cell]
            .children(arena)
            .filter_map(|c| match arena[c].kind_data() {
                KindData::Text(t) => Some((
                    t.str(source).to_owned(),
                    t.has_qualifiers(TextQualifier::HARD_LINE_BREAK),
                    t.has_qualifiers(TextQualifier::SOFT_LINE_BREAK),
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn multiline_non_last_column_trailing_backslash_becomes_hard_break() {
        // First column ends with `\` on its first physical line; pandoc
        // semantics now treat that as a hard break inside the cell.
        let input = "\
-----------  -----------
 First       Second
 line\\       cell text
 continued   here
-----------  -----------
";
        let p = make_parser();
        let mut reader = rushdown::text::BasicReader::new(input);
        let (arena, doc) = p.parse(&mut reader);
        let cell = first_body_cell(&arena, doc);
        let summary = cell_text_summary(&arena, cell, input);
        // Expect: "First line" with HARD_LINE_BREAK, then "continued".
        // The trailing `\` is stripped from the first text node.
        assert!(
            summary
                .iter()
                .any(|(s, hard, _)| s.contains("line") && !s.ends_with('\\') && *hard),
            "expected a HARD_LINE_BREAK Text with the trailing `\\` stripped; got {summary:?}"
        );
        // And no spurious SOFT_LINE_BREAK partner remains for that pair.
        let hard_idx = summary.iter().position(|(_, h, _)| *h).unwrap();
        if let Some((next_s, _, next_soft)) = summary.get(hard_idx + 1) {
            assert!(
                !(next_s.trim().is_empty() && *next_soft),
                "empty soft-break partner should have been removed; got {summary:?}"
            );
        }
    }

    #[test]
    fn multiline_last_column_trailing_backslash_already_hard_natively() {
        // Last column with trailing `\` — rushdown handles this natively
        // (segment includes the line's `\n`), so the transformer should
        // not double-fold; the assert is just that the resulting cell has
        // a HARD_LINE_BREAK and the trailing `\` is not in the text.
        let input = "\
-----------  -----------
 First       cell line\\
              continued
 Second       row
-----------  -----------
";
        let p = make_parser();
        let mut reader = rushdown::text::BasicReader::new(input);
        let (arena, doc) = p.parse(&mut reader);
        let cell = first_body_cell(&arena, doc);
        let summary = cell_text_summary(&arena, cell, input);
        // We're looking at the first body cell (column 1). It only has
        // "First" — the assertion of interest is on column 2, the last
        // column.
        // Walk the row to find the second cell.
        let row = arena[cell].parent().expect("cell has parent row");
        let last_cell = nth_cell_of_row(&arena, row, 1);
        let last_summary = cell_text_summary(&arena, last_cell, input);
        // Some text node (the one closing the first cell line) carries
        // HARD_LINE_BREAK and does *not* end with `\` — the trailing
        // backslash got consumed by rushdown's native hard-break path.
        assert!(
            last_summary
                .iter()
                .any(|(s, hard, _)| *hard && !s.ends_with('\\')),
            "expected last-column cell to have HARD_LINE_BREAK on text without trailing `\\`; got {last_summary:?}"
        );
        // Silence unused if first cell summary turns out empty.
        let _ = summary;
    }

    #[test]
    fn multiline_double_backslash_stays_literal() {
        // `\\` at line end is an even-count run → not a hard-break marker.
        let input = "\
-----------  -----------
 First       Second
 line\\\\       cell text
 continued   here
-----------  -----------
";
        let p = make_parser();
        let mut reader = rushdown::text::BasicReader::new(input);
        let (arena, doc) = p.parse(&mut reader);
        let cell = first_body_cell(&arena, doc);
        let summary = cell_text_summary(&arena, cell, input);
        // No HARD_LINE_BREAK should appear in this cell — the trailing `\\`
        // is literal text and the line still joins via SOFT.
        assert!(
            summary.iter().all(|(_, hard, _)| !*hard),
            "expected no HARD_LINE_BREAK for `\\\\` line end; got {summary:?}"
        );
    }

    #[test]
    fn br_html_in_multiline_cell_becomes_hard_break() {
        let input = "\
-----------  -----------
 First       Second
 a<br>b      cell text
-----------  -----------
";
        let p = make_parser();
        let mut reader = rushdown::text::BasicReader::new(input);
        let (arena, doc) = p.parse(&mut reader);
        let cell = first_body_cell(&arena, doc);
        // Expect: Text("a"), then an empty Text(HARD_LINE_BREAK), then Text("b").
        let children: Vec<_> = arena[cell].children(&arena).collect();
        let mut found_hard = false;
        for c in &children {
            if let KindData::Text(t) = arena[*c].kind_data() {
                if t.has_qualifiers(TextQualifier::HARD_LINE_BREAK) && t.str(input).is_empty() {
                    found_hard = true;
                }
            }
            // Should no longer be any RawHtml children for the <br>.
            assert!(
                !matches!(arena[*c].kind_data(), KindData::RawHtml(_)),
                "<br> RawHtml should have been replaced"
            );
        }
        assert!(
            found_hard,
            "expected an empty HARD_LINE_BREAK Text node from <br>; children: {children:?}"
        );
    }

    #[test]
    fn br_html_variants_all_recognised_in_cell() {
        // <br/>, <br />, <BR>.
        for variant in ["<br/>", "<br />", "<BR>"] {
            let input = format!(
                "\
-----------  -----------
 First       Second
 a{variant}b      cell text
-----------  -----------
"
            );
            let p = make_parser();
            let mut reader = rushdown::text::BasicReader::new(&input);
            let (arena, doc) = p.parse(&mut reader);
            let cell = first_body_cell(&arena, doc);
            let children: Vec<_> = arena[cell].children(&arena).collect();
            assert!(
                children.iter().any(|c| matches!(arena[*c].kind_data(),
                    KindData::Text(t) if t.has_qualifiers(TextQualifier::HARD_LINE_BREAK)
                        && t.str(&input).is_empty())),
                "expected hard-break Text for variant {variant}; got children {children:?}"
            );
            assert!(
                !children
                    .iter()
                    .any(|c| matches!(arena[*c].kind_data(), KindData::RawHtml(_))),
                "{variant} RawHtml should be gone"
            );
        }
    }

    #[test]
    fn grid_cell_trailing_backslash_becomes_hard_break() {
        let input = "\
+----------+----------+
| Header 1 | Header 2 |
+==========+==========+
| line\\    | other    |
| more     | text     |
+----------+----------+
";
        let p = make_parser();
        let mut reader = rushdown::text::BasicReader::new(input);
        let (arena, doc) = p.parse(&mut reader);
        let cell = first_body_cell(&arena, doc);
        let summary = cell_text_summary(&arena, cell, input);
        assert!(
            summary
                .iter()
                .any(|(s, hard, _)| s.contains("line") && !s.ends_with('\\') && *hard),
            "expected HARD_LINE_BREAK on grid cell first-line text without trailing `\\`; got {summary:?}"
        );
    }
}
