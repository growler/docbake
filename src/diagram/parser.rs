use super::*;

// ============================================================================
// Parser extension
// ============================================================================

/// Options for the diagram parser.
#[derive(Debug, Clone)]
pub struct DiagramParserOptions {
    pub mermaid_enabled: bool,
    pub plantuml_enabled: bool,
    pub typst_enabled: bool,
    pub d2_enabled: bool,
}

impl Default for DiagramParserOptions {
    fn default() -> Self {
        Self {
            mermaid_enabled: true,
            plantuml_enabled: true,
            typst_enabled: true,
            d2_enabled: true,
        }
    }
}

impl ParserOptions for DiagramParserOptions {}

#[derive(Debug)]
struct DiagramAstTransformer {
    options: DiagramParserOptions,
}

impl DiagramAstTransformer {
    fn with_options(options: DiagramParserOptions) -> Self {
        Self { options }
    }
}

/// Returns `true` if the node has the `.as-code` class in its attributes.
fn has_as_code_class(arena: &Arena, node_ref: NodeRef, source: &str) -> bool {
    arena[node_ref]
        .attributes()
        .get("class")
        .is_some_and(|v| v.str(source).split_whitespace().any(|c| c == "as-code"))
}

impl AstTransformer for DiagramAstTransformer {
    fn transform(
        &self,
        arena: &mut Arena,
        doc_ref: NodeRef,
        reader: &mut text::BasicReader,
        _ctx: &mut rd_parser::Context,
    ) {
        // Phase 1: collect code blocks that should become diagrams (read-only walk).
        let mut target_codes: Vec<NodeRef> = Vec::new();
        walk(arena, doc_ref, &mut |arena: &Arena,
                                   node_ref: NodeRef,
                                   entering: bool|
         -> Result<WalkStatus> {
            if entering && matches_kind!(arena[node_ref], CodeBlock) {
                // The `.as-code` class suppresses diagram transformation,
                // keeping the block as ordinary syntax-highlighted code.
                if has_as_code_class(arena, node_ref, reader.source()) {
                    return Ok(WalkStatus::Continue);
                }
                let code_block = as_kind_data!(arena[node_ref], CodeBlock);
                if let Some(lang) = code_block.language_str(reader.source()) {
                    let dominated = match lang {
                        "mermaid" => self.options.mermaid_enabled,
                        "plantuml" => self.options.plantuml_enabled,
                        "typst" => self.options.typst_enabled,
                        "d2" => self.options.d2_enabled,
                        _ => false,
                    };
                    if dominated {
                        target_codes.push(node_ref);
                    }
                }
            }
            Ok(WalkStatus::Continue)
        })
        .ok();

        // Phase 2: replace collected code blocks with Diagram nodes.
        for code_ref in target_codes {
            let code_block = as_kind_data!(arena[code_ref], CodeBlock);
            let lines = code_block.value().clone();
            let diagram_type = match code_block.language_str(reader.source()) {
                Some("mermaid") => DiagramType::Mermaid,
                Some("plantuml") => DiagramType::PlantUml,
                Some("typst") => DiagramType::Typst,
                Some("d2") => DiagramType::D2,
                _ => continue,
            };
            let pos = arena[code_ref].pos();
            let diagram = arena.new_node(Diagram::new(diagram_type));
            if let Some(pos) = pos {
                arena[diagram].set_pos(pos);
            }
            as_extension_data_mut!(arena, diagram, Diagram).set_value(lines);
            let hbl = as_type_data!(arena, code_ref, Block).has_blank_previous_line();
            as_type_data_mut!(arena, diagram, Block).set_blank_previous_line(hbl);
            // Propagate user attributes (e.g. {width=50%}) from the original
            // code block to the replacement Diagram node so that renderers can
            // apply them to the output element.
            let attrs: Vec<_> = arena[code_ref]
                .attributes()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (key, value) in attrs {
                arena[diagram].attributes_mut().insert(key, value);
            }
            arena[code_ref]
                .parent()
                .unwrap()
                .replace_child(arena, code_ref, diagram);
        }
    }
}

impl From<DiagramAstTransformer> for AnyAstTransformer {
    fn from(t: DiagramAstTransformer) -> Self {
        AnyAstTransformer::Extension(Box::new(t))
    }
}

/// Returns a parser extension that detects diagram code blocks and transforms
/// them into [`Diagram`] AST nodes.
pub fn diagram_parser_extension(options: DiagramParserOptions) -> impl ParserExtension {
    ParserExtensionFn::new(|p: &mut Parser| {
        p.add_ast_transformer(DiagramAstTransformer::with_options, options, 100);
    })
}
