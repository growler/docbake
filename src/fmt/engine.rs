use anyhow::Result;
use rushdown::parser::AstTransformer as _;
use rushdown::text::BasicReader;

use crate::fmt::renderer::MarkdownFmtRenderer;
use crate::fmt::rules;

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

    for line in text.lines() {
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
            if blank_run >= rules::MAX_CONSECUTIVE_BLANK_LINES {
                continue;
            }
            blank_run += 1;
        } else {
            blank_run = 0;
        }
        out_lines.push(line);
    }

    out_lines.join("\n")
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
    fn wraps_plain_paragraphs_to_100_columns() -> Result<()> {
        let src = "word ".repeat(30);
        let formatted = super::format_markdown(&src)?;
        let longest = formatted
            .lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        assert!(longest <= 100);
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
        let src = "Позиция | Цена\n:-------|----:\nSRV-001 | 100\nSRV-002 | 200\n";
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
        let src = "* Элемент 1\n* Элемент 2\n* Элемент 3\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("* Элемент 1\n* Элемент 2\n* Элемент 3"));
        assert!(!formatted.contains("- Элемент 1"));
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
    fn keeps_definition_list_structure() -> Result<()> {
        let src = "Термин\n:  Определение\n\nТермин\n:  Определение\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("Термин\n:  Определение"));
        assert!(!formatted.contains("Термин : Определение"));
        Ok(())
    }

    #[test]
    fn keeps_sparse_definition_list_spacing() -> Result<()> {
        let src = "Термин\n\n:  Определение\n\n    ```\n    code example\n    ```\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("Термин\n\n:  Определение"));
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
    fn normalizes_hard_break_to_backslash_marker() -> Result<()> {
        let src = "\\*не курсив\\*\\\n\\# не заголовок\\\n\\[не ссылка\\]\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("\\*не курсив\\*\\\n\\# не заголовок\\\n\\[не ссылка\\]"));
        assert!(!formatted.contains("\\*не курсив\\*  \n"));
        assert!(!formatted.contains("\\# не заголовок  \n"));
        Ok(())
    }

    #[test]
    fn converts_two_space_hard_break_to_backslash_marker() -> Result<()> {
        let src = "line one  \nline two  \nline three\n";
        let formatted = super::format_markdown(src)?;
        assert!(formatted.contains("line one\\\nline two\\\nline three"));
        assert!(!formatted.contains("line one  \n"));
        assert!(!formatted.contains("line two  \n"));
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
}
