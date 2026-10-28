//! Tests of the table output in `word/document.xml`.

use std::sync::LazyLock;

use regex::Regex;

use super::{assert_runs_unformatted, render_parts, PARAGRAPH, PARAGRAPH_STYLE};

/// A table. Tables are not nested.
static TABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<w:tbl>.*?</w:tbl>").unwrap());

/// A table row.
static ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<w:tr>.*?</w:tr>").unwrap());

/// A table cell.
static CELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<w:tc>.*?</w:tc>").unwrap());

/// The `w:firstRow` value of a table.
static FIRST_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<w:tblLook w:val="(\d{4})" w:firstRow="(\d)""#).unwrap());

/// A grid column width.
static GRID_COLUMN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<w:gridCol w:w="(\d+)" />"#).unwrap());

/// A cell width.
static CELL_WIDTH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<w:tcW w:w="(\d+)" w:type="dxa" />"#).unwrap());

const HEADER_ROW: &str = "<w:trPr><w:tblHeader /></w:trPr>";
const BODY_ROW: &str = "<w:trPr />";

/// Renders `markdown` and gives the tables of `word/document.xml`.
fn render_tables(markdown: &str) -> Vec<String> {
    let document = render_parts(markdown).unwrap()("word/document.xml");
    TABLE
        .find_iter(&document)
        .map(|m| m.as_str().to_string())
        .collect()
}

fn rows(table: &str) -> Vec<&str> {
    ROW.find_iter(table).map(|m| m.as_str()).collect()
}

fn cells(row: &str) -> Vec<&str> {
    CELL.find_iter(row).map(|m| m.as_str()).collect()
}

/// Gives the number of header rows of `table` and asserts that they are
/// the first rows.
#[track_caller]
fn header_row_count(table: &str) -> usize {
    let rows = rows(table);
    let count = rows.iter().take_while(|r| r.contains(HEADER_ROW)).count();
    for row in &rows[count..] {
        assert!(row.starts_with(&format!("<w:tr>{BODY_ROW}")), "{table}");
    }
    count
}

/// Gives the `w:tblLook` value and `w:firstRow` of `table`.
#[track_caller]
fn look(table: &str) -> (String, String) {
    let c = FIRST_ROW
        .captures(table)
        .unwrap_or_else(|| panic!("no w:tblLook in {table}"));
    (c[1].to_string(), c[2].to_string())
}

fn grid(table: &str) -> Vec<u32> {
    GRID_COLUMN
        .captures_iter(table)
        .map(|c| c[1].parse().unwrap())
        .collect()
}

/// Gives the width of `cell`.
#[track_caller]
fn cell_width(cell: &str) -> u32 {
    CELL_WIDTH
        .captures(cell)
        .unwrap_or_else(|| panic!("no w:tcW in {cell}"))[1]
        .parse()
        .unwrap()
}

/// The text width of the blank theme in twips.
const TEXT_WIDTH: u32 = 10205;

#[test]
fn pipe_table_has_style_header_row_and_equal_grid() {
    let tables = render_tables(
        "\
| Name | Value |
|------|-------|
| one  | 1     |
| two  | 2     |
",
    );
    assert_eq!(tables.len(), 1);
    let table = &tables[0];
    assert!(
        table.starts_with(concat!(
            r#"<w:tbl><w:tblPr><w:tblStyle w:val="DataTable" />"#,
            r#"<w:tblW w:w="0" w:type="auto" /><w:tblLayout w:type="fixed" />"#,
            r#"<w:tblLook w:val="0420" w:firstRow="1" w:lastRow="0" "#,
            r#"w:firstColumn="0" w:lastColumn="0" w:noHBand="0" w:noVBand="1" />"#,
            r#"</w:tblPr><w:tblGrid><w:gridCol w:w="5102" /><w:gridCol w:w="5103" />"#,
            r#"</w:tblGrid><w:tr>"#,
        )),
        "{table}"
    );
    assert!(!table.contains("w:tblBorders"), "{table}");
    assert_eq!(header_row_count(table), 1, "{table}");
    let rows = rows(table);
    assert_eq!(rows.len(), 3, "{table}");
    for row in rows {
        let widths: Vec<u32> = cells(row).into_iter().map(cell_width).collect();
        assert_eq!(widths, [5102, 5103], "{row}");
    }
}

#[test]
fn two_row_header_marks_both_rows() {
    let tables = render_tables(
        "\
+-----+-----+
| A   | B   |
+-----+-----+
| C   | D   |
+=====+=====+
| 1   | 2   |
+-----+-----+
",
    );
    assert_eq!(tables.len(), 1);
    assert_eq!(rows(&tables[0]).len(), 3, "{}", tables[0]);
    assert_eq!(header_row_count(&tables[0]), 2, "{}", tables[0]);
    assert_eq!(look(&tables[0]), ("0420".into(), "1".into()));
}

#[test]
fn table_without_header_has_first_row_off() {
    let tables = render_tables(
        "\
+-----+-----+
| 1   | 2   |
+-----+-----+
| 3   | 4   |
+-----+-----+

-----  -----
one    1

two    2
-----  -----
",
    );
    assert_eq!(tables.len(), 2);
    for table in &tables {
        assert_eq!(rows(table).len(), 2, "{table}");
        assert_eq!(header_row_count(table), 0, "{table}");
        assert!(!table.contains("w:tblHeader"), "{table}");
        assert_eq!(look(table), ("0400".into(), "0".into()));
    }
}

#[test]
fn grid_and_multiline_widths_follow_source() {
    let tables = render_tables(
        "\
+----+--------+
| A  | B      |
+====+========+
| 1  | 2      |
+----+--------+

------------------
Name     Value
------   ------------
one      1
------------------
",
    );
    assert_eq!(tables.len(), 2);
    for table in &tables {
        assert_eq!(grid(table), [3401, 6804], "{table}");
        assert_eq!(grid(table).iter().sum::<u32>(), TEXT_WIDTH);
        for row in rows(table) {
            let widths: Vec<u32> = cells(row).into_iter().map(cell_width).collect();
            assert_eq!(widths, [3401, 6804], "{row}");
        }
    }
}

#[test]
fn multiline_widths_in_containers_follow_source() {
    let tables = render_tables(
        "\
1.  Ordered item

    -------------------
    Name   Value
    -----  -----------
    one    1
    -------------------

> -------------------
> Name   Value
> -----  -----------
> one    1
> -------------------

Term
:   Definition.

    -------------------
    Name   Value
    -----  -----------
    one    1
    -------------------
",
    );
    assert_eq!(tables.len(), 3);
    for table in &tables {
        assert_eq!(grid(table), [3189, 7016], "{table}");
    }
}

#[test]
fn cells_use_table_styles_and_markdown_alignment() {
    let document = render_parts(
        "\
| Left | Center | Right | None |
|:-----|:------:|------:|------|
| l    | c      | r     | n    |
",
    )
    .unwrap()("word/document.xml");
    let table = TABLE.find(&document).unwrap().as_str();
    let rows = rows(table);
    assert_eq!(rows.len(), 2, "{table}");
    for (row, style) in rows.iter().zip(["TableHeader", "TableCell"]) {
        let paragraphs: Vec<&str> = PARAGRAPH.find_iter(row).map(|m| m.as_str()).collect();
        assert_eq!(paragraphs.len(), 4, "{row}");
        let alignments = [Some("left"), Some("center"), Some("right"), None];
        for (p, alignment) in paragraphs.iter().zip(alignments) {
            assert_eq!(&PARAGRAPH_STYLE.captures(p).unwrap()[1], style, "{p}");
            match alignment {
                Some(a) => assert!(p.contains(&format!(r#"<w:jc w:val="{a}" />"#)), "{p}"),
                None => assert!(!p.contains("<w:jc "), "{p}"),
            }
        }
    }
    assert_runs_unformatted(&document, "TableHeader", &[]);
    assert_runs_unformatted(&document, "TableCell", &[]);
}

#[test]
fn tables_in_containers_keep_record_order() {
    let tables = render_tables(
        "\
> | H |
> |---|
> | b |

> +---+
> | x |
> +---+

- Item

  +---+
  | y |
  +---+

- | H |
  |---|
  | b |

:::note
+---+
| z |
+---+
:::

Term
: | H |
  |---|
  | b |

+---+
| w |
+---+

Table: Caption

| H |
|---|
| c |
",
    );
    let first_rows: Vec<String> = tables.iter().map(|t| look(t).1).collect();
    assert_eq!(first_rows, ["1", "0", "0", "1", "0", "1", "0", "1"]);
    for table in &tables {
        let expected = usize::from(look(table).1 == "1");
        assert_eq!(header_row_count(table), expected, "{table}");
    }
}

#[test]
fn column_span_gives_grid_span_and_width() {
    let tables = render_tables(
        "\
+--------------+---------------+--------------+
| Name         | Contact                      |
+--------------+---------------+--------------+
| Name         | Email         | Phone        |
+==============+===============+==============+
| A            | a@example.com | 1            |
+--------------+---------------+--------------+
| All data is current                         |
+--------------+---------------+--------------+
",
    );
    assert_eq!(tables.len(), 1);
    let table = &tables[0];
    let grid = grid(table);
    assert_eq!(grid.len(), 3, "{table}");
    assert_eq!(grid.iter().sum::<u32>(), TEXT_WIDTH);
    assert_eq!(header_row_count(table), 2, "{table}");
    let rows = rows(table);
    assert_eq!(rows.len(), 4, "{table}");

    let first = cells(rows[0]);
    assert_eq!(first.len(), 2, "{}", rows[0]);
    assert_eq!(cell_width(first[0]), grid[0]);
    assert!(!first[0].contains("w:gridSpan"), "{}", first[0]);
    assert_eq!(cell_width(first[1]), grid[1] + grid[2]);
    assert!(
        first[1].contains(r#"<w:gridSpan w:val="2" />"#),
        "{}",
        first[1]
    );

    assert_eq!(cells(rows[1]).len(), 3, "{}", rows[1]);
    assert_eq!(cells(rows[2]).len(), 3, "{}", rows[2]);

    let last = cells(rows[3]);
    assert_eq!(last.len(), 1, "{}", rows[3]);
    assert_eq!(cell_width(last[0]), TEXT_WIDTH);
    assert!(
        last[0].contains(r#"<w:gridSpan w:val="3" />"#),
        "{}",
        last[0]
    );
    assert!(!table.contains("w:vMerge"), "{table}");
}

#[test]
fn row_span_gives_vertical_merge() {
    let tables = render_tables(
        "\
+------+------+
| Name | Mail |
+======+======+
| A    | a@x  |
+      +------+
|      | 1    |
+------+------+
| B    | b    |
+------+      +
| C    |      |
+------+------+
",
    );
    assert_eq!(tables.len(), 1);
    let table = &tables[0];
    let rows = rows(table);
    assert_eq!(rows.len(), 5, "{table}");
    let restart = r#"<w:vMerge w:val="restart" />"#;
    let continued = r#"<w:vMerge w:val="continue" />"#;
    let merges = |row: &str| -> Vec<&'static str> {
        cells(row)
            .into_iter()
            .map(|c| {
                if c.contains(restart) {
                    "restart"
                } else if c.contains(continued) {
                    "continue"
                } else {
                    "-"
                }
            })
            .collect()
    };
    assert_eq!(merges(rows[0]), ["-", "-"]);
    assert_eq!(merges(rows[1]), ["restart", "-"]);
    assert_eq!(merges(rows[2]), ["continue", "-"]);
    assert_eq!(merges(rows[3]), ["-", "restart"]);
    assert_eq!(merges(rows[4]), ["-", "continue"]);
    let grid = grid(table);
    for row in &rows {
        let widths: Vec<u32> = cells(row).into_iter().map(cell_width).collect();
        assert_eq!(widths, grid, "{row}");
        for cell in cells(row) {
            assert!(PARAGRAPH.is_match(cell), "{cell}");
        }
    }
}

#[test]
fn row_and_column_span_continues_with_grid_span() {
    let tables = render_tables(
        "\
+-----+-----+-----+
| H1  | H2  | H3  |
+=====+=====+=====+
| a         | b   |
+     +     +-----+
|           | c   |
+-----+-----+-----+
",
    );
    assert_eq!(tables.len(), 1);
    let table = &tables[0];
    let grid = grid(table);
    let rows = rows(table);
    assert_eq!(rows.len(), 3, "{table}");
    for (row, merge) in rows[1..].iter().zip(["restart", "continue"]) {
        let cells = cells(row);
        assert_eq!(cells.len(), 2, "{row}");
        assert!(
            cells[0].contains(&format!(r#"<w:vMerge w:val="{merge}" />"#)),
            "{row}"
        );
        assert!(cells[0].contains(r#"<w:gridSpan w:val="2" />"#), "{row}");
        assert_eq!(cell_width(cells[0]), grid[0] + grid[1]);
        assert_eq!(cell_width(cells[1]), grid[2]);
    }
}

#[test]
fn document_with_tables_is_well_formed() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // Skip if xmllint is not on PATH.
    if which::which("xmllint").is_err() {
        eprintln!("skipping document_with_tables_is_well_formed: xmllint not found on PATH");
        return;
    }
    let document = render_parts(
        "\
| A | B |
|---|--:|
| 1 | 2 |

+-----+-----+
| H1  | H2  |
+=====+=====+
| a   | b   |
+     +-----+
|     | c   |
+-----+-----+
",
    )
    .unwrap()("word/document.xml");
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
        .write_all(document.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
