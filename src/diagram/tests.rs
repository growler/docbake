use super::*;
use rushdown::matches_extension_kind;
use rushdown::parser;
use rushdown::text::BasicReader;

fn test_parser() -> parser::Parser {
    parser::Parser::with_extensions(
        parser::Options {
            attributes: true,
            ..Default::default()
        },
        parser::gfm(parser::GfmOptions::default())
            .and(diagram_parser_extension(DiagramParserOptions::default())),
    )
}

fn parse(source: &str) -> (Arena, NodeRef) {
    let parser = test_parser();
    let mut reader = BasicReader::new(source);
    parser.parse(&mut reader)
}

#[test]
fn mermaid_code_block_becomes_diagram_node() {
    let source = "```mermaid\ngraph LR\n  A --> B\n```\n";
    let (arena, doc_ref) = parse(source);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert_eq!(children.len(), 1, "expected one Diagram node");
    assert!(
        matches_extension_kind!(&arena, children[0], Diagram),
        "expected Diagram extension node"
    );

    let diagram = as_extension_data!(&arena, children[0], Diagram);
    assert_eq!(diagram.diagram_type(), DiagramType::Mermaid);
}

#[test]
fn plantuml_code_block_becomes_diagram_node() {
    let source = "```plantuml\n@startuml\nAlice -> Bob\n@enduml\n```\n";
    let (arena, doc_ref) = parse(source);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert_eq!(children.len(), 1, "expected one Diagram node");
    assert!(matches_extension_kind!(&arena, children[0], Diagram));

    let diagram = as_extension_data!(&arena, children[0], Diagram);
    assert_eq!(diagram.diagram_type(), DiagramType::PlantUml);
}

#[test]
fn non_diagram_code_block_unchanged() {
    let source = "```rust\nfn main() {}\n```\n";
    let (arena, doc_ref) = parse(source);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert_eq!(children.len(), 1);
    assert!(
        matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
        "rust code block should remain a CodeBlock"
    );
}

#[test]
fn disabled_mermaid_leaves_code_block() {
    let opts = DiagramParserOptions {
        mermaid_enabled: false,
        plantuml_enabled: true,
        typst_enabled: true,
        d2_enabled: true,
    };
    let parser = parser::Parser::with_extensions(
        parser::Options::default(),
        parser::gfm(parser::GfmOptions::default()).and(diagram_parser_extension(opts)),
    );
    let source = "```mermaid\ngraph LR\n```\n";
    let mut reader = BasicReader::new(source);
    let (arena, doc_ref) = parser.parse(&mut reader);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert!(
        matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
        "mermaid code block should remain when mermaid is disabled"
    );
}

#[test]
fn mermaid_builtin_rendering() {
    let svg = render_mermaid_svg("flowchart LR; A-->B-->C").unwrap();
    assert!(
        svg.contains("<svg"),
        "expected SVG output from mermaid-rs-renderer"
    );
}

#[test]
fn typst_code_block_becomes_diagram_node() {
    let source = "```typst\n#rect(width: 50pt, height: 30pt)\n```\n";
    let (arena, doc_ref) = parse(source);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert_eq!(children.len(), 1, "expected one Diagram node");
    assert!(
        matches_extension_kind!(&arena, children[0], Diagram),
        "expected Diagram extension node"
    );

    let diagram = as_extension_data!(&arena, children[0], Diagram);
    assert_eq!(diagram.diagram_type(), DiagramType::Typst);
}

#[test]
fn disabled_typst_leaves_code_block() {
    let opts = DiagramParserOptions {
        mermaid_enabled: true,
        plantuml_enabled: true,
        typst_enabled: false,
        d2_enabled: true,
    };
    let parser = parser::Parser::with_extensions(
        parser::Options::default(),
        parser::gfm(parser::GfmOptions::default()).and(diagram_parser_extension(opts)),
    );
    let source = "```typst\n#rect(width: 50pt, height: 30pt)\n```\n";
    let mut reader = BasicReader::new(source);
    let (arena, doc_ref) = parser.parse(&mut reader);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert!(
        matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
        "typst code block should remain when typst is disabled"
    );
}

#[test]
fn typst_svg_rendering() {
    // Skip if typst is not on PATH.
    if which::which("typst").is_err() {
        eprintln!("skipping typst_svg_rendering: typst not found on PATH");
        return;
    }
    let svg = render_typst_svg(None, "#rect(width: 50pt, height: 30pt, fill: blue)").unwrap();
    assert!(
        svg.contains("<svg"),
        "expected SVG output from typst compile"
    );
}

#[test]
fn diagram_type_for_path_detects_drawio() {
    assert_eq!(
        diagram_type_for_path("arch.drawio"),
        Some(DiagramType::DrawIo)
    );
    assert_eq!(
        diagram_type_for_path("arch.DRAWIO"),
        Some(DiagramType::DrawIo)
    );
}

#[test]
fn diagram_type_for_path_strips_fragment() {
    assert_eq!(
        diagram_type_for_path("arch.drawio#1"),
        Some(DiagramType::DrawIo)
    );
    assert_eq!(
        diagram_type_for_path("arch.drawio#42"),
        Some(DiagramType::DrawIo)
    );
    // Non-drawio with fragment should still work.
    assert_eq!(
        diagram_type_for_path("seq.puml#something"),
        Some(DiagramType::PlantUml)
    );
}

#[test]
fn drawio_page_extraction() {
    assert_eq!(drawio_page_from_dest("arch.drawio#1"), Some(1));
    assert_eq!(drawio_page_from_dest("arch.drawio#42"), Some(42));
    assert_eq!(drawio_page_from_dest("arch.drawio"), None);
    assert_eq!(drawio_page_from_dest("arch.drawio#"), None);
    assert_eq!(drawio_page_from_dest("arch.drawio#abc"), None);
}

#[test]
fn drawio_svg_rendering() {
    // Skip if drawio is not on PATH.
    if which::which("drawio").is_err() {
        eprintln!("skipping drawio_svg_rendering: drawio not found on PATH");
        return;
    }
    // Use the project's test asset.
    let asset = std::path::Path::new("doc/assets/Docbake High Level.drawio");
    if !asset.exists() {
        eprintln!("skipping drawio_svg_rendering: test asset not found");
        return;
    }
    let bytes = render_drawio(None, asset, Some(1), DrawioFormat::Svg).unwrap();
    let svg = String::from_utf8(bytes).expect("SVG output should be valid UTF-8");
    assert!(svg.contains("<svg"), "expected SVG output from drawio CLI");
}

#[test]
fn d2_code_block_becomes_diagram_node() {
    let source = "```d2\nx -> y: hello\n```\n";
    let (arena, doc_ref) = parse(source);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert_eq!(children.len(), 1, "expected one Diagram node");
    assert!(
        matches_extension_kind!(&arena, children[0], Diagram),
        "expected Diagram extension node"
    );

    let diagram = as_extension_data!(&arena, children[0], Diagram);
    assert_eq!(diagram.diagram_type(), DiagramType::D2);
}

#[test]
fn disabled_d2_leaves_code_block() {
    let opts = DiagramParserOptions {
        mermaid_enabled: true,
        plantuml_enabled: true,
        typst_enabled: true,
        d2_enabled: false,
    };
    let parser = parser::Parser::with_extensions(
        parser::Options::default(),
        parser::gfm(parser::GfmOptions::default()).and(diagram_parser_extension(opts)),
    );
    let source = "```d2\nx -> y: hello\n```\n";
    let mut reader = BasicReader::new(source);
    let (arena, doc_ref) = parser.parse(&mut reader);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert!(
        matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
        "d2 code block should remain when d2 is disabled"
    );
}

#[test]
fn d2_svg_rendering() {
    // Skip if d2 is not on PATH.
    if which::which("d2").is_err() {
        eprintln!("skipping d2_svg_rendering: d2 not found on PATH");
        return;
    }
    let svg = render_d2_svg(None, "x -> y: hello world").unwrap();
    assert!(svg.contains("<svg"), "expected SVG output from d2 CLI");
}

#[test]
fn diagram_type_for_path_detects_d2() {
    assert_eq!(diagram_type_for_path("network.d2"), Some(DiagramType::D2));
    assert_eq!(diagram_type_for_path("network.D2"), Some(DiagramType::D2));
}

/// Parser that includes both the code-block attribute extension (priority
/// 50) and the diagram extension (priority 100), matching production order.
fn parse_with_attrs(source: &str) -> (Arena, NodeRef) {
    let parser = parser::Parser::with_extensions(
        parser::Options {
            attributes: true,
            ..Default::default()
        },
        parser::gfm(parser::GfmOptions::default())
            .and(crate::code_block_attribute::code_block_attribute_parser_extension())
            .and(diagram_parser_extension(DiagramParserOptions::default())),
    );
    let mut reader = BasicReader::new(source);
    parser.parse(&mut reader)
}

#[test]
fn as_code_suppresses_mermaid_diagram() {
    let source = "```mermaid {.as-code}\ngraph LR\n  A --> B\n```\n";
    let (arena, doc_ref) = parse_with_attrs(source);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert_eq!(children.len(), 1);
    assert!(
        matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
        "mermaid block with .as-code should remain a CodeBlock, got {:?}",
        arena[children[0]].kind_data().kind_name()
    );
}

#[test]
fn as_code_suppresses_d2_diagram() {
    let source = "```d2 {.as-code}\nx -> y: hello\n```\n";
    let (arena, doc_ref) = parse_with_attrs(source);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert_eq!(children.len(), 1);
    assert!(
        matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
        "d2 block with .as-code should remain a CodeBlock"
    );
}

#[test]
fn as_code_with_multiple_classes() {
    let source = "```mermaid {.as-code .example}\ngraph LR\n```\n";
    let (arena, doc_ref) = parse_with_attrs(source);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert!(
        matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
        "mermaid block with .as-code among multiple classes should remain a CodeBlock"
    );
}

#[test]
fn mermaid_without_as_code_still_becomes_diagram() {
    let source = "```mermaid {#my-diagram}\ngraph LR\n  A --> B\n```\n";
    let (arena, doc_ref) = parse_with_attrs(source);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert_eq!(children.len(), 1);
    assert!(
        matches_extension_kind!(&arena, children[0], Diagram),
        "mermaid block without .as-code should become a Diagram"
    );
}

#[test]
fn as_code_suppresses_typst_diagram() {
    let source = "```typst {.as-code}\n#rect(width: 50pt)\n```\n";
    let (arena, doc_ref) = parse_with_attrs(source);

    let children: Vec<_> = arena[doc_ref].children(&arena).collect();
    assert_eq!(children.len(), 1);
    assert!(
        matches!(arena[children[0]].kind_data(), KindData::CodeBlock(_)),
        "typst block with .as-code should remain a CodeBlock"
    );
}
