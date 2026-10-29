//! Tests of the list output in `word/numbering.xml` and
//! `word/document.xml`.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use super::{assert_runs_unformatted, between, render_parts, PARAGRAPH, PARAGRAPH_STYLE, RUN_TEXT};

/// The numbering reference of a paragraph.
static NUM_PR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<w:numPr><w:numId w:val="(\d+)" /><w:ilvl w:val="(\d+)" /></w:numPr>"#).unwrap()
});

/// A run.
static RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<w:r>.*?</w:r>").unwrap());

/// A `w:num` with its `w:abstractNumId` and its level overrides.
static NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<w:num w:numId="(\d+)"><w:abstractNumId w:val="(\d+)" />(.*?)</w:num>"#).unwrap()
});

/// A list item paragraph.
#[derive(Debug)]
struct Item {
    text: String,
    /// The text of the first run.
    first_run: String,
    style: String,
    num_id: usize,
    ilvl: usize,
}

/// The `w:abstractNumId` and the level overrides of a `w:num`.
#[derive(Debug)]
struct Num {
    abstract_num_id: usize,
    overrides: String,
}

/// Renders `markdown` and gives the paragraphs with a numbering reference
/// and the `w:num` elements by `w:numId`.
fn render_lists(markdown: &str) -> (Vec<Item>, BTreeMap<usize, Num>) {
    let part = render_parts(markdown).unwrap();
    let document = part("word/document.xml");
    let items = PARAGRAPH
        .find_iter(&document)
        .filter_map(|p| {
            let p = p.as_str();
            let num_pr = NUM_PR.captures(p)?;
            Some(Item {
                text: RUN_TEXT
                    .captures_iter(p)
                    .map(|c| c[1].to_string())
                    .collect(),
                first_run: RUN.find(p).map_or(String::new(), |r| {
                    RUN_TEXT
                        .captures_iter(r.as_str())
                        .map(|c| c[1].to_string())
                        .collect()
                }),
                style: PARAGRAPH_STYLE
                    .captures(p)
                    .map_or(String::new(), |c| c[1].to_string()),
                num_id: num_pr[1].parse().unwrap(),
                ilvl: num_pr[2].parse().unwrap(),
            })
        })
        .collect();
    let nums = NUM
        .captures_iter(&part("word/numbering.xml"))
        .map(|c| {
            let num = Num {
                abstract_num_id: c[2].parse().unwrap(),
                overrides: c[3].to_string(),
            };
            (c[1].parse().unwrap(), num)
        })
        .collect();
    (items, nums)
}

/// The level override that sets the start number of `ilvl` to `start`.
fn start_override(ilvl: usize, start: usize) -> String {
    format!(r#"<w:lvlOverride w:ilvl="{ilvl}"><w:startOverride w:val="{start}" /></w:lvlOverride>"#)
}

#[track_caller]
fn assert_list_paragraphs(items: &[Item]) {
    for item in items {
        assert_eq!(item.style, "ListParagraph", "{item:?}");
    }
}

#[test]
fn numbering_has_list_definitions_with_six_levels() {
    let numbering = render_parts("- Item\n").unwrap()("word/numbering.xml");
    assert_eq!(
        numbering
            .matches(r#"<w:abstractNum w:abstractNumId="1">"#)
            .count(),
        1
    );
    assert_eq!(numbering.matches(r#"<w:num w:numId="1">"#).count(), 1);
    assert_eq!(numbering.matches("<w:abstractNum ").count(), 4);

    let level = |abstract_num_id: usize, ilvl: usize| {
        let definition = between(
            &numbering,
            &format!(r#"<w:abstractNum w:abstractNumId="{abstract_num_id}">"#),
            "</w:abstractNum>",
        );
        assert_eq!(definition.matches("<w:lvl ").count(), 6, "{definition}");
        between(
            definition,
            &format!(r#"<w:lvl w:ilvl="{ilvl}">"#),
            "</w:lvl>",
        )
        .to_string()
    };
    for (abstract_num_id, ilvl, format, text) in [
        (2, 0, "bullet", "•"),
        (2, 1, "bullet", "◦"),
        (2, 2, "bullet", "▪"),
        (2, 3, "bullet", "•"),
        (2, 4, "bullet", "◦"),
        (2, 5, "bullet", "▪"),
        (3, 0, "decimal", "%1."),
        (3, 1, "lowerLetter", "%2."),
        (3, 2, "lowerRoman", "%3."),
        (3, 3, "decimal", "%4."),
        (3, 4, "lowerLetter", "%5."),
        (3, 5, "lowerRoman", "%6."),
    ] {
        let level = level(abstract_num_id, ilvl);
        assert!(
            level.contains(&format!(
                r#"<w:numFmt w:val="{format}" /><w:lvlText w:val="{text}" /><w:lvlJc w:val="left" />"#
            )),
            "{level}"
        );
    }
    for abstract_num_id in 2..=4 {
        for ilvl in 0..6 {
            let level = level(abstract_num_id, ilvl);
            assert!(level.contains(r#"<w:start w:val="1" />"#), "{level}");
            assert!(
                level.contains(&format!(
                    r#"<w:ind w:left="{}" w:right="0" w:hanging="360" />"#,
                    720 * (ilvl + 1)
                )),
                "{level}"
            );
            if abstract_num_id == 4 {
                assert!(
                    level.contains(r#"<w:numFmt w:val="none" /><w:lvlText w:val="" />"#),
                    "{level}"
                );
            }
        }
    }
}

#[test]
fn nested_lists_have_levels_and_own_nums() {
    let (items, nums) = render_lists("- a\n  - b\n    1. c\n- d\n");
    assert_list_paragraphs(&items);
    let texts: Vec<_> = items.iter().map(|i| (i.text.as_str(), i.ilvl)).collect();
    assert_eq!(texts, [("a", 0), ("b", 1), ("c", 2), ("d", 0)]);
    let [a, b, c, d] = [0, 1, 2, 3].map(|i| items[i].num_id);
    assert_eq!(a, d);
    assert!(a != b && b != c && a != c, "{items:?}");
    assert_eq!(nums[&a].abstract_num_id, 2);
    assert_eq!(nums[&b].abstract_num_id, 2);
    assert_eq!(nums[&c].abstract_num_id, 3);
    assert_eq!(nums[&a].overrides, "");
    assert_eq!(nums[&b].overrides, "");
    assert_eq!(nums[&c].overrides, start_override(2, 1));

    let (items, nums) = render_lists(
        "\
- 1
  - 2
    - 3
      - 4
        - 5
          - 6
            1. 7
",
    );
    let levels: Vec<_> = items.iter().map(|i| (i.text.as_str(), i.ilvl)).collect();
    assert_eq!(
        levels,
        [
            ("1", 0),
            ("2", 1),
            ("3", 2),
            ("4", 3),
            ("5", 4),
            ("6", 5),
            ("7", 5)
        ]
    );
    let deepest = items[6].num_id;
    assert_ne!(deepest, items[5].num_id);
    assert_eq!(nums[&deepest].abstract_num_id, 3);
    assert_eq!(nums[&deepest].overrides, start_override(5, 1));
}

#[test]
fn each_ordered_list_restarts() {
    let (items, nums) =
        render_lists("1. a\n2. b\n\ntext\n\n1. c\n\ntext\n\n3. d\n\ntext\n\n- e\n\ntext\n\n0. f\n");
    assert_list_paragraphs(&items);
    let texts: Vec<_> = items.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(texts, ["a", "b", "c", "d", "e", "f"]);
    let [a, b, c, d, e, f] = [0, 1, 2, 3, 4, 5].map(|i| items[i].num_id);
    assert_eq!(a, b);
    let ids = [a, c, d, e, f];
    for (i, id) in ids.iter().enumerate() {
        assert!(!ids[i + 1..].contains(id), "{items:?}");
    }
    for (id, start) in [(a, 1), (c, 1), (d, 3), (f, 1)] {
        assert_eq!(nums[&id].abstract_num_id, 3);
        assert_eq!(nums[&id].overrides, start_override(0, start));
    }
    assert_eq!(nums[&e].abstract_num_id, 2);
    assert_eq!(nums[&e].overrides, "");
}

#[test]
fn task_items_start_with_box_characters() {
    let markdown = "- [ ] open\n- [x] done\n- plain\n\n1. [ ] ordered\n2. second\n";
    let (items, nums) = render_lists(markdown);
    assert_list_paragraphs(&items);
    let texts: Vec<_> = items.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(
        texts,
        ["☐ open", "☒ done", "• plain", "☐ ordered", "second"]
    );
    let first_runs: Vec<_> = items.iter().map(|i| i.first_run.as_str()).collect();
    assert_eq!(first_runs, ["☐ ", "☒ ", "• ", "☐ ", "second"]);
    let task = items[0].num_id;
    assert!(items[..3].iter().all(|i| i.num_id == task), "{items:?}");
    assert_eq!(nums[&task].abstract_num_id, 4);
    assert_eq!(nums[&task].overrides, "");
    let ordered = items[3].num_id;
    assert_eq!(items[4].num_id, ordered);
    assert_eq!(nums[&ordered].abstract_num_id, 3);

    let document = render_parts(markdown).unwrap()("word/document.xml");
    assert!(!document.contains("[ ]"), "{document}");
    assert!(!document.contains("[x]"), "{document}");
    assert_runs_unformatted(&document, "ListParagraph", &[]);
}

#[test]
fn next_paragraphs_of_item_have_no_marker() {
    let (items, nums) = render_lists(
        "\
1. one

   more

   again
2. two
   - nested

     nested more
",
    );
    assert_list_paragraphs(&items);
    let texts: Vec<_> = items.iter().map(|i| (i.text.as_str(), i.ilvl)).collect();
    assert_eq!(
        texts,
        [
            ("one", 0),
            ("more", 0),
            ("again", 0),
            ("two", 0),
            ("nested", 1),
            ("nested more", 1),
        ]
    );
    let ids: Vec<_> = items.iter().map(|i| i.num_id).collect();
    let [list, next, nested, nested_next] = [ids[0], ids[1], ids[4], ids[5]];
    assert_eq!(ids, [list, next, next, list, nested, nested_next]);
    assert_eq!(nums[&list].abstract_num_id, 3);
    assert_eq!(nums[&nested].abstract_num_id, 2);
    for id in [next, nested_next] {
        assert_eq!(nums[&id].abstract_num_id, 4);
        assert_eq!(nums[&id].overrides, "");
    }
    assert_ne!(next, nested_next);

    let (items, _) = render_lists("- [ ] one\n\n  more\n- [x] two\n");
    assert_list_paragraphs(&items);
    let texts: Vec<_> = items.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(texts, ["☐ one", "more", "☒ two"]);
    assert!(
        items.iter().all(|i| i.num_id == items[0].num_id),
        "{items:?}"
    );
}

#[test]
fn empty_items_have_numbered_paragraphs() {
    let (items, nums) = render_lists("1. a\n2.\n3. c\n\ntext\n\n- [ ] task\n-\n");
    assert_list_paragraphs(&items);
    let texts: Vec<_> = items.iter().map(|i| (i.text.as_str(), i.ilvl)).collect();
    assert_eq!(
        texts,
        [("a", 0), ("", 0), ("c", 0), ("☐ task", 0), ("• ", 0)]
    );
    let ordered = items[0].num_id;
    assert_eq!([items[1].num_id, items[2].num_id], [ordered, ordered]);
    assert_eq!(nums[&ordered].abstract_num_id, 3);
    let task = items[3].num_id;
    assert_eq!(items[4].num_id, task);
    assert_eq!(nums[&task].abstract_num_id, 4);
}

#[test]
fn item_that_starts_with_nested_list_has_numbered_paragraph() {
    let (items, nums) = render_lists("1. - nested first\n2. b\n");
    assert_list_paragraphs(&items);
    let texts: Vec<_> = items.iter().map(|i| (i.text.as_str(), i.ilvl)).collect();
    assert_eq!(texts, [("", 0), ("nested first", 1), ("b", 0)]);
    let ordered = items[0].num_id;
    assert_eq!(items[2].num_id, ordered);
    assert_eq!(nums[&ordered].abstract_num_id, 3);
    assert_eq!(nums[&items[1].num_id].abstract_num_id, 2);
}

#[test]
fn item_that_starts_with_code_block_has_numbered_paragraph() {
    let markdown = "\
1. ```
   code
   ```

   text
";
    let document = render_parts(markdown).unwrap()("word/document.xml");
    let paragraphs: Vec<_> = PARAGRAPH
        .find_iter(&document)
        .map(|p| {
            let p = p.as_str();
            let style = PARAGRAPH_STYLE
                .captures(p)
                .map_or(String::new(), |c| c[1].to_string());
            let text: String = RUN_TEXT
                .captures_iter(p)
                .map(|c| c[1].to_string())
                .collect();
            (style, text)
        })
        .collect();
    let expected = [
        ("ListParagraph", ""),
        ("CodeBlock", "code"),
        ("ListParagraph", "text"),
    ]
    .map(|(style, text)| (style.to_string(), text.to_string()));
    assert!(
        paragraphs.windows(3).any(|w| w == expected),
        "{paragraphs:?}"
    );

    let (items, nums) = render_lists(markdown);
    let texts: Vec<_> = items.iter().map(|i| (i.text.as_str(), i.ilvl)).collect();
    assert_eq!(texts, [("", 0), ("text", 0)]);
    let [list, next] = [items[0].num_id, items[1].num_id];
    assert_eq!(nums[&list].abstract_num_id, 3);
    assert_eq!(nums[&list].overrides, start_override(0, 1));
    assert_eq!(nums[&next].abstract_num_id, 4);
    assert_eq!(nums[&next].overrides, "");
}
