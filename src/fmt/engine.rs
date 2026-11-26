use anyhow::Result;
use rushdown::parser::AstTransformer as _;
use rushdown::text::BasicReader;

use crate::fmt::renderer::MarkdownFmtRenderer;
use crate::fmt::rules;

pub(crate) fn format_prepared_markdown(
    prepared: &crate::graph::PreparedDocument,
    profile: &crate::conditional::RenderProfile,
) -> Result<String> {
    let base_renderer = MarkdownFmtRenderer::new(rules::PARAGRAPH_LINE_WIDTH, rules::ALIGN_TABLES);
    let graph_renderer = base_renderer.for_graph(profile.clone());
    let rendered = graph_renderer.render_prepared_document(prepared)?;
    let mut out = post_process(&rendered);
    if rules::ENSURE_FINAL_NEWLINE && !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

pub(crate) fn format_markdown(input: &str) -> Result<String> {
    let (front_matter, body) = crate::graph::build::split_front_matter(input);

    let parser = crate::build::markdown_parser();
    let mut reader = BasicReader::new(body);
    let (mut arena, doc_ref) = parser.parse(&mut reader);

    {
        let mut reader = BasicReader::new(body);
        let mut ctx = rushdown::parser::Context::new();
        crate::caption::CaptionAstTransformer::new().transform(
            &mut arena,
            doc_ref,
            &mut reader,
            &mut ctx,
        );
    }

    let renderer = MarkdownFmtRenderer::new(rules::PARAGRAPH_LINE_WIDTH, rules::ALIGN_TABLES);
    let rendered = renderer.render_document(&arena, doc_ref, body)?;
    let rendered = post_process(&rendered);

    let mut out = String::new();
    if let Some(front_matter) = front_matter {
        out.push_str(&front_matter);
    }
    out.push_str(&rendered);

    if rules::ENSURE_FINAL_NEWLINE && !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

fn post_process(text: &str) -> String {
    let mut out_lines = Vec::new();
    let mut blank_run = 0usize;
    let mut in_fence: Option<(char, usize)> = None;
    let mut preserve_blank_run = false;

    for line in text.lines() {
        if line == crate::fmt::renderer::PRESERVE_BLANK_RUN_MARKER {
            preserve_blank_run = true;
            blank_run = 0;
            continue;
        }

        if let Some((marker, len)) = in_fence {
            out_lines.push(line.to_string());
            if is_fence_close(line, marker, len) {
                in_fence = None;
            }
            continue;
        }

        if let Some((marker, len)) = parse_fence_line(line) {
            out_lines.push(line.to_string());
            in_fence = Some((marker, len));
            blank_run = 0;
            continue;
        }

        let line = if rules::TRIM_TRAILING_WHITESPACE {
            trim_trailing_whitespace(line)
        } else {
            line.to_string()
        };

        if line.trim().is_empty() {
            if !preserve_blank_run && blank_run >= rules::MAX_CONSECUTIVE_BLANK_LINES {
                continue;
            }
            blank_run += 1;
        } else {
            blank_run = 0;
            preserve_blank_run = false;
        }
        out_lines.push(line);
    }

    out_lines
        .join("\n")
        .replace(crate::fmt::renderer::HARD_BREAK_SPACES_MARKER, "  ")
}

fn parse_fence_line(line: &str) -> Option<(char, usize)> {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    if indent > 3 {
        return None;
    }
    let trimmed = &line[indent..];
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|c| *c == marker).count();
    (run >= 3).then_some((marker, run))
}

fn is_fence_close(line: &str, marker: char, open_len: usize) -> bool {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    if indent > 3 {
        return false;
    }
    let trimmed = &line[indent..];
    let run = trimmed.chars().take_while(|c| *c == marker).count();
    if run < open_len {
        return false;
    }
    trimmed[run..].chars().all(|c| c == ' ' || c == '\t')
}

fn trim_trailing_whitespace(line: &str) -> String {
    line.trim_end_matches([' ', '\t']).to_string()
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    #[test]
    fn wraps_plain_paragraphs_to_80_columns() -> Result<()> {
        let src = "word ".repeat(30);
        let formatted = super::format_markdown(&src)?;
        let longest = formatted
            .lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        assert!(longest <= 80);
        Ok(())
    }

    #[test]
    fn aligns_pipe_tables() -> Result<()> {
        let src = "| A | Long value |\n|---|---:|\n|1|2|\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("| A   | Long value |"));
        assert!(formatted.contains("| --- | ---------: |"));
        assert!(formatted.contains("| 1   |          2 |"));
        Ok(())
    }

    #[test]
    fn preserves_tables_without_edge_pipes() -> Result<()> {
        let src = "A | Long value\n---|---:\n1|2\n";
        let formatted = super::format_markdown(src)?;
        let lines: Vec<&str> = formatted.lines().take(3).collect();
        assert_eq!(lines.len(), 3);
        for line in lines {
            assert!(!line.trim_start().starts_with('|'));
            assert!(!line.trim_end().ends_with('|'));
        }
        Ok(())
    }

    #[test]
    fn keeps_text_visually_aligned_for_right_aligned_columns() -> Result<()> {
        let src = "Item | Cost\n:-------|----:\nSRV-001 | 100\nSRV-002 | 200\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains(":------ | ---:"));
        assert!(formatted.contains("SRV-001 |  100"));
        assert!(formatted.contains("SRV-002 |  200"));
        Ok(())
    }

    #[test]
    fn keeps_tight_task_lists_compact() -> Result<()> {
        let src = "- [x] Done\n- [ ] Pending\n- [ ] Next\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("- [x] Done\n- [ ] Pending\n- [ ] Next"));
        assert!(!formatted.contains("- [x] Done\n\n- [ ] Pending"));
        Ok(())
    }

    #[test]
    fn keeps_loose_task_lists_with_blank_lines() -> Result<()> {
        let src = "- [x] Done\n\n- [ ] Pending\n\n- [ ] Next\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("- [x] Done\n\n- [ ] Pending\n\n- [ ] Next"));
        Ok(())
    }

    #[test]
    fn preserves_unordered_list_marker_character() -> Result<()> {
        let src = "* Item 1\n* Item 2\n* Item 3\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("* Item 1\n* Item 2\n* Item 3"));
        assert!(!formatted.contains("- Item 1"));
        Ok(())
    }

    #[test]
    fn preserves_unordered_task_list_marker_character() -> Result<()> {
        let src = "+ [x] Done\n+ [ ] Pending\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("+ [x] Done\n+ [ ] Pending"));
        assert!(!formatted.contains("- [x] Done"));
        Ok(())
    }

    #[test]
    fn preserves_ordered_list_marker_character_while_renumbering() -> Result<()> {
        let src = "1) Item 1\n   1) Item 1.1\n   1) Item 1.2\n1) Item 2\n";
        let formatted = super::format_markdown(src)?;

        assert!(formatted.contains("1) Item 1"));
        assert!(formatted.contains("   1) Item 1.1"));
        assert!(formatted.contains("   2) Item 1.2"));
        assert!(formatted.contains("2) Item 2"));
        assert!(!formatted.contains("1. Item 1"));

        Ok(())
    }

    #[test]
    fn keeps_definition_list_structure() -> Result<()> {
        let src = "Term\n:  Definition\n\nTerm\n:  Definition\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("Term\n:  Definition"));
        assert!(!formatted.contains("Term : Definition"));
        Ok(())
    }

    #[test]
    fn keeps_sparse_definition_list_spacing() -> Result<()> {
        let src = "Term\n\n:  Definition\n\n    ```\n    code example\n    ```\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("Term\n\n:  Definition"));
        Ok(())
    }

    #[test]
    fn preserves_front_matter() -> Result<()> {
        let src = "---\ntitle:  Test  \n---\nSome paragraph with enough text to require line wrapping and trimming.\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.starts_with("---\ntitle:  Test  \n---\n"));
        Ok(())
    }

    #[test]
    fn preserves_tilde_fence_opening_line() -> Result<()> {
        let src = "~~~rust {.example}\nfn main() {}\n~~~\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("~~~rust {.example}"));
        assert!(!formatted.contains("```rust {.example}"));
        Ok(())
    }

    #[test]
    fn preserves_backslash_hard_break_marker() -> Result<()> {
        let src = "\\*not italic\\*\\\n\\# not a heading\\\n\\[not a link\\]\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("\\*not italic\\*\\\n\\# not a heading\\\n\\[not a link\\]"));
        assert!(!formatted.contains("\\*not italic\\*  \n"));
        assert!(!formatted.contains("\\# not a heading  \n"));
        Ok(())
    }

    #[test]
    fn preserves_two_space_hard_break_marker() -> Result<()> {
        let src = "line one  \nline two  \nline three\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("line one  \nline two  \nline three"));
        assert!(!formatted.contains("line one\\\nline two\\\nline three"));
        Ok(())
    }

    #[test]
    fn preserves_reference_and_shortcut_links() -> Result<()> {
        let src = "See [RFC 7763][rfc]. Also see [Builder].\n\n[rfc]: https://example.com\n[Builder]: builder/index.md\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("See [RFC 7763][rfc]. Also see [Builder]."));
        assert!(!formatted.contains("[RFC 7763](https://example.com)"));
        Ok(())
    }

    #[test]
    fn preserves_autolink_text_without_mailto_rewrite() -> Result<()> {
        let src = "<https://example.com>\n<someone@somewhere.net>\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("<https://example.com>"));
        assert!(formatted.contains("<someone@somewhere.net>"));
        assert!(!formatted.contains("mailto:"));
        Ok(())
    }

    #[test]
    fn preserves_angle_brackets_for_destinations_with_spaces() -> Result<()> {
        let src = concat!(
            "[Architecture](<../../assets/High Level Design.drawio>)\n\n",
            "![Architecture](<../../assets/High Level Design.drawio>)\n",
        );
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("](<../../assets/High Level Design.drawio>)"));
        Ok(())
    }

    #[test]
    fn preserves_fenced_div_opening_spacing() -> Result<()> {
        let src = "::: important\nText\n:::\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.starts_with("::: important\n"));
        Ok(())
    }

    #[test]
    fn preserves_fenced_div_closing_fence_length() -> Result<()> {
        let src = ":::: {.confluence}\nText\n\n:::::\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("\n:::::\n"));
        Ok(())
    }

    #[test]
    fn preserves_extra_blank_lines_between_fenced_div_blocks() -> Result<()> {
        let src = concat!(
            ":::alpha\n",
            "One\n",
            ":::\n",
            "\n",
            "\n",
            ":::beta\n",
            "Two\n",
            ":::\n",
        );
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains(":::\n\n\n:::beta"));
        Ok(())
    }

    #[test]
    fn inserts_blank_line_before_nested_fenced_div_when_missing() -> Result<()> {
        let src = concat!(
            ":::: {.confluence}\n",
            "This is text\n",
            ":::warning\n",
            "Warning\n",
            ":::\n",
            ":::::::::::::::::::::::::::::::::::::::::\n",
        );
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("This is text\n\n:::warning"));
        Ok(())
    }

    #[test]
    fn preserves_lowercase_caption_prefix() -> Result<()> {
        let src = "table: Example\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.starts_with("table: Example\n\n"));
        Ok(())
    }

    #[test]
    fn keeps_include_headings_compact() -> Result<()> {
        let src = ":::include\n## [One](one.md)\n## [Two](two.md)\n:::\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains(":::include\n## [One](one.md)\n## [Two](two.md)\n:::\n"));
        Ok(())
    }

    #[test]
    fn wraps_tight_list_items_without_inserting_blank_lines() -> Result<()> {
        let src = concat!(
            "- This is a very long tight list item that should still wrap according to the paragraph width ",
            "without turning into a loose list item or collapsing into one overlong line.\n",
            "- Second item\n"
        );
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains(
            "- This is a very long tight list item that should still wrap according to the\n"
        ));
        assert!(formatted.contains("  paragraph width without turning into a loose list item"));
        assert!(!formatted.contains("\n\n- Second item"));
        Ok(())
    }

    #[test]
    fn wraps_definition_lines_without_turning_them_into_code_blocks() -> Result<()> {
        let src = concat!(
            "`lang`\n",
            ":  The document language in RFC 5646 format, with a description that is long ",
            "enough to wrap to the next line by the line length rule of the formatter.\n"
        );
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains(":  The document language"));
        assert!(formatted.contains("\n   to wrap to the next line"));
        assert!(!formatted.contains("\n    to wrap to the next line"));
        Ok(())
    }

    #[test]
    fn preserves_explicit_codespan_padding_spaces() -> Result<()> {
        let src = "`` \\ ` * _ { } [ ] ( ) # + - . ! | ~ ``\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("`` \\ ` * _ { } [ ] ( ) # + - . ! | ~ ``"));
        assert!(!formatted.contains("``\\ ` * _ { } [ ] ( ) # + - . ! | ~``"));
        Ok(())
    }

    #[test]
    fn preserves_tilde_fence_inside_list_item() -> Result<()> {
        let src = "* Item 1\n\n* Item 2\n\n  ~~~\n  code\n  ~~~\n\n* Item 3\n";
        let formatted = super::format_markdown(src)?;
        assert_eq!(formatted, src);
        Ok(())
    }

    #[test]
    fn preserves_tilde_fence_on_definition_marker_line() -> Result<()> {
        let src = concat!(
            "`project/index.md`\n\n",
            ":  ~~~markdown\n",
            "   # System purpose\n\n",
            "     :::include\n",
            "   # [HLD](hld/index.md)\n",
            "   :::\n",
            "   ~~~\n",
        );
        let formatted = super::format_markdown(src)?;
        assert_eq!(formatted, src);
        Ok(())
    }

    #[test]
    fn preserves_loose_list_block_indent() -> Result<()> {
        let src = "- Point\n\n    ~~~\n  code\n    ~~~\n";
        let formatted = super::format_markdown(src)?;
        assert_eq!(formatted, src);
        Ok(())
    }

    #[test]
    fn preserves_definition_list_marker_spacing_and_wrap_indent() -> Result<()> {
        let src = concat!(
            "Orange\n",
            ":   A fruit of evergreen trees of the genus Citrus. The fruit has an orange skin and\n",
            "    a round shape.\n",
        );
        let formatted = super::format_markdown(src)?;
        assert_eq!(formatted, src);
        Ok(())
    }

    #[test]
    fn preserves_block_style_footnote_definition() -> Result<()> {
        let src = concat!(
            "Text with a footnote[^fn2].\n\n",
            "[^fn2]:\n",
            "    This is a footnote with more than one paragraph.\n\n",
            "    Indent each paragraph with four spaces.\n",
        );
        let formatted = super::format_markdown(src)?;
        assert_eq!(formatted, src);
        Ok(())
    }

    #[test]
    fn ignores_footnote_like_text_inside_fenced_code() -> Result<()> {
        let src = "```markdown\n[^fake]: not a footnote definition\n```\n";
        let formatted = super::format_markdown(src)?;
        assert_eq!(formatted, src);
        Ok(())
    }
}
