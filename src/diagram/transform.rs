use super::*;

// ============================================================================
// Image-to-diagram AST transformation
// ============================================================================

/// Diagram file extensions and their corresponding types.
const DIAGRAM_EXTENSIONS: &[(&str, DiagramType)] = &[
    ("puml", DiagramType::PlantUml),
    ("plantuml", DiagramType::PlantUml),
    ("mmd", DiagramType::Mermaid),
    ("mermaid", DiagramType::Mermaid),
    ("typ", DiagramType::Typst),
    ("drawio", DiagramType::DrawIo),
    ("d2", DiagramType::D2),
];

/// Detect whether an image destination path refers to a diagram file.
///
/// The `dest` string may contain a fragment (e.g. `diagram.drawio#1`);
/// the fragment is stripped before checking the extension.
pub(super) fn diagram_type_for_path(dest: &str) -> Option<DiagramType> {
    let path_part = dest.split('#').next().unwrap_or(dest);
    let ext = path_part.rsplit('.').next()?;
    DIAGRAM_EXTENSIONS
        .iter()
        .find(|(e, _)| e.eq_ignore_ascii_case(ext))
        .map(|(_, dt)| *dt)
}

/// Extract the page index from a `.drawio` image destination fragment.
///
/// Given `diagram.drawio#2`, returns `Some(2)`.  Returns `None` if
/// there is no fragment or the fragment is not a valid page number.
pub(super) fn drawio_page_from_dest(dest: &str) -> Option<u32> {
    let (_, fragment) = dest.split_once('#')?;
    fragment.parse::<u32>().ok()
}

/// If `paragraph_ref` contains exactly one child and it is an `Image` node,
/// return that image's `NodeRef`.
pub(crate) fn paragraph_has_sole_image(arena: &Arena, paragraph_ref: NodeRef) -> Option<NodeRef> {
    let mut children = arena[paragraph_ref].children(arena);
    let first = children.next()?;
    if children.next().is_some() {
        return None; // more than one child
    }
    if matches!(arena[first].kind_data(), KindData::Image(_)) {
        Some(first)
    } else {
        None
    }
}

/// Walk the AST and replace `Paragraph > Image(.puml/.mmd/.drawio)` with a
/// `Diagram` node containing the file content (or pre-rendered SVG for
/// Draw.io diagrams).
///
/// An image is eligible when:
/// - Its destination ends with a known diagram extension (`.puml`, `.mmd`,
///   `.drawio`, etc.)
/// - It is the only meaningful child of its parent `Paragraph`
///
/// The parent `Paragraph` is replaced by a `Diagram` `LeafBlock`.
pub(crate) fn transform_diagram_images(
    arena: &mut Arena,
    doc_ref: NodeRef,
    source: &str,
    source_dir: &Path,
    base_dir: &Path,
) -> anyhow::Result<()> {
    // Phase 1: collect eligible (paragraph, image, type, resolved_path, drawio_page) tuples.
    // The image NodeRef is stored so we can copy its attributes (e.g. {width=50%})
    // onto the replacement Diagram node in phase 2.
    let mut candidates: Vec<(NodeRef, NodeRef, DiagramType, PathBuf, Option<u32>)> = Vec::new();

    for child in arena[doc_ref].children(arena).collect::<Vec<_>>() {
        if !matches!(arena[child].kind_data(), KindData::Paragraph(_)) {
            continue;
        }
        let Some(image_ref) = paragraph_has_sole_image(arena, child) else {
            continue;
        };
        let image = as_kind_data!(arena[image_ref], Image);
        let dest = image.destination_str(source);
        let Some(diagram_type) = diagram_type_for_path(dest) else {
            continue;
        };

        // Strip fragment before path resolution (e.g. `diagram.drawio#1` → `diagram.drawio`).
        let path_part = dest.split('#').next().unwrap_or(dest);
        let drawio_page = if diagram_type == DiagramType::DrawIo {
            drawio_page_from_dest(dest)
        } else {
            None
        };

        // Resolve the file path relative to the source directory.
        let file_path = if path_part.starts_with('/') {
            base_dir.join(path_part.trim_start_matches('/'))
        } else {
            source_dir.join(path_part)
        };
        let file_path = file_path.canonicalize().with_context(|| {
            format!(
                "failed to resolve diagram image `{dest}` relative to {}",
                source_dir.display()
            )
        })?;
        if !file_path.starts_with(base_dir) {
            anyhow::bail!(
                "diagram image `{dest}` resolves to {} which is outside the base directory {}",
                file_path.display(),
                base_dir.display()
            );
        }
        candidates.push((child, image_ref, diagram_type, file_path, drawio_page));
    }

    // Phase 2: replace each paragraph with a Diagram node.
    for (paragraph_ref, image_ref, diagram_type, file_path, drawio_page) in candidates {
        let dest = as_kind_data!(arena[image_ref], Image)
            .destination_str(source)
            .to_owned();
        let diagram_node = if diagram_type == DiagramType::DrawIo {
            // Draw.io diagrams store the resolved path for deferred
            // rendering — the actual SVG/PDF conversion happens at render
            // time so that each renderer can choose the appropriate format.
            arena.new_node(Diagram::from_drawio(file_path, drawio_page, dest))
        } else {
            let content = fs::read_to_string(&file_path)
                .with_context(|| format!("failed to read diagram file {}", file_path.display()))?;
            arena.new_node(Diagram::from_file(diagram_type, content, dest))
        };
        if let Some(pos) = arena[paragraph_ref].pos() {
            arena[diagram_node].set_pos(pos);
        }
        let hbl = as_type_data!(arena, paragraph_ref, Block).has_blank_previous_line();
        as_type_data_mut!(arena, diagram_node, Block).set_blank_previous_line(hbl);
        // Propagate user attributes (e.g. {width=50%}) from the original
        // Image node to the replacement Diagram node so that renderers can
        // apply them to the output element.
        let attrs: Vec<_> = arena[image_ref]
            .attributes()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (key, value) in attrs {
            arena[diagram_node].attributes_mut().insert(key, value);
        }
        doc_ref.replace_child(arena, paragraph_ref, diagram_node);
    }

    Ok(())
}
