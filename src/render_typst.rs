use std::cell::Cell;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use rushdown::as_extension_data;
use rushdown::as_kind_data;
use rushdown::as_type_data;
use rushdown::ast::{Arena, KindData, LinkKind, NodeRef, TableCellAlignment, Task, TextQualifier};
use rushdown_definition_list::{DefinitionList, Term, TermDefinition};
use rushdown_fenced_div::FencedDiv;
use rushdown_footnote::FootnoteReference;

use crate::conditional::RenderProfile;
use crate::graph::PreparedDocument;
use crate::DebugDumper;

pub struct TypstRenderer {
    /// Output directory where the `.typ` file is written.  Sidecar PDF
    /// files for Draw.io diagrams are placed here so that Typst can
    /// reference them via relative paths.
    output_root: PathBuf,
    /// Counter for generating unique sidecar file names.
    drawio_counter: Cell<u32>,
    /// Artifact cache for rendered diagrams.
    cache: crate::cache::Cache,
    /// PlantUML theme content (inserted after `@start*` line).
    plantuml_theme: Option<String>,
    /// Mermaid theme content (prepended before diagram source).
    mermaid_theme: Option<String>,
    /// D2 theme content (prepended before diagram source).
    d2_theme: Option<String>,
    profile: RenderProfile,
}

#[derive(Clone, Copy)]
struct RenderScope {
    doc_id: usize,
}

#[derive(Clone, Copy)]
struct RenderContext<'a> {
    prepared: &'a PreparedDocument,
    scope: RenderScope,
    arena: &'a Arena,
    source: &'a str,
    heading_shift: usize,
    known_labels: &'a HashSet<String>,
    logger: &'a DebugDumper,
}

impl TypstRenderer {
    pub fn new(
        output_root: PathBuf,
        cache: crate::cache::Cache,
        plantuml_theme: Option<String>,
        mermaid_theme: Option<String>,
        d2_theme: Option<String>,
        profile: RenderProfile,
    ) -> Self {
        Self {
            output_root,
            drawio_counter: Cell::new(0),
            cache,
            plantuml_theme,
            mermaid_theme,
            d2_theme,
            profile,
        }
    }

    pub fn render_document(
        &self,
        prepared: &PreparedDocument,
        logger: &DebugDumper,
    ) -> Result<String> {
        let mut out = String::new();
        let known_labels = collect_labels(prepared, &self.profile)?;
        self.render_graph_document(
            &mut out,
            prepared,
            prepared.graph().root_id,
            0,
            &known_labels,
            logger,
        )?;
        Ok(out)
    }

    fn render_graph_document(
        &self,
        out: &mut String,
        prepared: &PreparedDocument,
        doc_id: usize,
        heading_shift: usize,
        known_labels: &HashSet<String>,
        logger: &DebugDumper,
    ) -> Result<()> {
        let doc = prepared.graph().document(doc_id);
        for child in doc.arena[doc.doc_ref].children(&doc.arena) {
            if !crate::conditional::node_is_visible(&doc.arena, child, &doc.source, &self.profile)?
            {
                continue;
            }
            if let Some(entries) = prepared.graph().include_entries(doc_id, child) {
                for entry in entries {
                    self.render_graph_document(
                        out,
                        prepared,
                        entry.doc_id,
                        heading_shift + entry.heading_shift,
                        known_labels,
                        logger,
                    )?;
                }
                continue;
            }
            let context = RenderContext {
                prepared,
                scope: RenderScope { doc_id },
                arena: &doc.arena,
                source: &doc.source,
                heading_shift,
                known_labels,
                logger,
            };
            self.render_block(out, &context, child)?;
        }
        Ok(())
    }

    fn render_block(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        let logger = context.logger;
        match arena[node_ref].kind_data() {
            KindData::Paragraph(_) => {
                self.render_inline_children(out, context, node_ref)?;
                out.push_str("\n\n");
            }
            KindData::Heading(heading) => {
                let level = (heading.level() as usize + context.heading_shift).min(6);
                out.push_str(&"=".repeat(level));
                out.push(' ');
                self.render_inline_children(out, context, node_ref)?;
                if let Some(id) = arena[node_ref].attributes().get("id") {
                    write!(out, " <{}>", id.str(source))?;
                }
                out.push_str("\n\n");
            }
            KindData::ThematicBreak(_) => out.push_str("#thematic-break()\n\n"),
            KindData::CodeBlock(code) => {
                // Lines from rushdown include trailing newlines already.
                let mut value = String::new();
                for line in code.value().iter(source) {
                    value.push_str(line.as_ref());
                }
                // Strip a single trailing newline so the raw block does not
                // end with an extra blank line.
                if value.ends_with('\n') {
                    value.pop();
                }
                out.push_str("#code(\"");
                out.push_str(&escape_typst_string(&value));
                if let Some(lang) = code.language_str(source) {
                    write!(out, "\", lang: \"{}\"", escape_typst_string(lang))?;
                } else {
                    out.push('"');
                }
                if let Some(start) = arena[node_ref]
                    .attributes()
                    .get("start")
                    .and_then(|s| s.str(source).parse::<u64>().ok())
                {
                    write!(out, ", start-line: {}", start)?;
                }
                out.push(')');
                if let Some(id) = arena[node_ref].attributes().get("id") {
                    write!(out, " <{}>", id.str(source))?;
                }
                out.push_str("\n\n");
            }
            KindData::Blockquote(_) => {
                out.push_str("#quote(block: true)[");
                for child in arena[node_ref].children(arena) {
                    self.render_block(out, context, child)?;
                }
                out.push_str("]\n\n");
            }
            KindData::List(list) => {
                let tight = list.is_tight();
                let is_task_list = !list.is_ordered()
                    && arena[node_ref].children(arena).any(|item| {
                        matches!(
                            arena[item].kind_data(),
                            KindData::ListItem(li) if li.is_task()
                        )
                    });
                if list.is_ordered() {
                    write!(
                        out,
                        "#enum(start: {}, tight: {tight}, ",
                        list.start().max(1)
                    )?;
                } else if is_task_list {
                    write!(out, "#list(marker: none, tight: {tight}, ")?;
                } else {
                    write!(out, "#list(tight: {tight}, ")?;
                }
                let mut first = true;
                for item in arena[node_ref].children(arena) {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    out.push('[');
                    if is_task_list {
                        if let KindData::ListItem(li) = arena[item].kind_data() {
                            match li.task() {
                                Some(Task::Completed) => out.push_str("#task-checkbox(true) "),
                                Some(Task::Active) => out.push_str("#task-checkbox(false) "),
                                // Non-task item in a mixed list — restore the
                                // bullet that marker: none suppressed.
                                Some(_) | None => out.push_str("#task-bullet() "),
                            }
                        }
                    }
                    let item_children: Vec<_> = arena[item].children(arena).collect();
                    for (ci, &child) in item_children.iter().enumerate() {
                        self.render_block(out, context, child)?;
                        // In tight lists, collapse the paragraph break between
                        // a paragraph and a following nested list so they don't
                        // get extra vertical spacing inside the item.
                        if tight
                            && ci + 1 < item_children.len()
                            && matches!(arena[child].kind_data(), KindData::Paragraph(_))
                            && matches!(arena[item_children[ci + 1]].kind_data(), KindData::List(_))
                        {
                            // The paragraph just wrote "\n\n"; trim to "\n".
                            if out.ends_with("\n\n") {
                                out.pop();
                            }
                        }
                    }
                    out.push(']');
                }
                out.push_str(")\n\n");
            }
            KindData::Table(_) => {
                self.render_table(out, context, node_ref)?;
            }
            KindData::HtmlBlock(_) | KindData::RawHtml(_) => {
                logger.log("skipping raw HTML for Typst output");
            }
            KindData::LinkReferenceDefinition(_) => {}
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, DefinitionList) =>
            {
                self.render_definition_list(out, context, node_ref)?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FencedDiv) =>
            {
                if !crate::conditional::fenced_div_is_visible(
                    arena,
                    node_ref,
                    source,
                    &self.profile,
                )? {
                    return Ok(());
                }
                let classes = arena[node_ref]
                    .attributes()
                    .get("class")
                    .map(|value| value.str(source))
                    .unwrap_or_default();
                if let Some(adm_type) = crate::admonition_type(&classes) {
                    self.render_admonition(out, context, node_ref, adm_type)?;
                } else {
                    for child in arena[node_ref].children(arena) {
                        self.render_block(out, context, child)?;
                    }
                }
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, crate::caption::Caption) =>
            {
                self.render_caption(out, context, node_ref)?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::caption::CaptionText
                ) =>
            {
                // CaptionText is rendered as part of Caption; if encountered
                // standalone, just render inline children.
                self.render_inline_children(out, context, node_ref)?;
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, crate::diagram::Diagram) =>
            {
                let diagram = as_extension_data!(arena, node_ref, crate::diagram::Diagram);
                let src = crate::diagram::collect_diagram_source(diagram, source);

                // Read width/height attributes so they can be forwarded to
                // #image() calls (or #block() wrappers for embedded Typst
                // diagrams), mirroring how regular images handle these.
                let attrs = arena[node_ref].attributes();
                let width_attr = attrs.get("width").map(|v| v.str(source).to_string());
                let height_attr = attrs.get("height").map(|v| v.str(source).to_string());
                // Build a suffix like ", width: 50%, height: 200px" for
                // #image() argument lists.
                let mut size_args = String::new();
                if let Some(w) = &width_attr {
                    write!(size_args, ", width: {w}")?;
                }
                if let Some(h) = &height_attr {
                    write!(size_args, ", height: {h}")?;
                }

                if diagram.diagram_type() == crate::diagram::DiagramType::Typst {
                    // Typst diagrams are embedded directly into the Typst
                    // document — no SVG conversion needed.  Wrapped in
                    // #{ [ ... ] } to create an isolated scope (avoiding
                    // name collisions between multiple diagram imports)
                    // while keeping the inner content in markup mode
                    // where `#` prefixes work as expected.
                    if size_args.is_empty() {
                        out.push_str("#{\n[\n");
                    } else {
                        write!(out, "#block({}) {{\n[\n", &size_args[2..])?;
                    }
                    out.push_str(&src);
                    if !src.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("]\n}\n\n");
                } else if diagram.diagram_type() == crate::diagram::DiagramType::DrawIo {
                    // Draw.io diagrams are rendered to PDF and written as
                    // sidecar files — Typst 0.14+ can use PDFs in #image().
                    // This avoids the foreignObject problem that makes
                    // Draw.io SVGs incompatible with Typst's SVG renderer.
                    if let Some(path) = diagram.drawio_path() {
                        let cache_key =
                            crate::diagram::drawio_cache_key(path, diagram.drawio_page());
                        match crate::diagram::cached_render_bytes(
                            &self.cache,
                            &cache_key,
                            "drawio.pdf",
                            || {
                                crate::diagram::render_drawio(
                                    None,
                                    path,
                                    diagram.drawio_page(),
                                    crate::diagram::DrawioFormat::Pdf,
                                )
                                .map_err(|e| anyhow::anyhow!("{e}"))
                            },
                        ) {
                            Ok(pdf_bytes) => {
                                let n = self.drawio_counter.get();
                                self.drawio_counter.set(n + 1);
                                let filename = format!("drawio-{n}.pdf");
                                let sidecar = self.output_root.join(&filename);
                                if let Err(e) = std::fs::write(&sidecar, &pdf_bytes) {
                                    logger.log(format!(
                                        "warning: failed to write Draw.io sidecar {}: {e}",
                                        sidecar.display()
                                    ));
                                } else {
                                    write!(
                                        out,
                                        "#image(\"{}\"{size_args})\n\n",
                                        escape_typst_string(&filename)
                                    )?;
                                }
                            }
                            Err(e) => {
                                logger.log(format!(
                                    "warning: failed to render Draw.io diagram for Typst: {e}"
                                ));
                                out.push_str("#raw(\"");
                                out.push_str(&escape_typst_string(&path.display().to_string()));
                                out.push_str("\", block: true)\n\n");
                            }
                        }
                    }
                } else if diagram.diagram_type() == crate::diagram::DiagramType::D2 {
                    // D2 diagrams are rendered to SVG via the `d2` CLI.
                    let src = crate::diagram::apply_d2_theme(&src, self.d2_theme.as_deref());
                    let svg_result = crate::diagram::cached_render_svg(
                        &self.cache,
                        src.as_bytes(),
                        "d2.svg",
                        || {
                            crate::diagram::render_d2_svg(None, &src)
                                .map_err(|e| anyhow::anyhow!("{e}"))
                        },
                    );
                    match svg_result {
                        Ok(svg) => {
                            out.push_str("#image(\n  bytes(\"");
                            out.push_str(&escape_typst_string(&svg));
                            write!(out, "\"),\n  format: \"svg\"{size_args},\n)\n\n")?;
                        }
                        Err(e) => {
                            logger.log(format!(
                                "warning: failed to render D2 diagram for Typst: {e}"
                            ));
                            out.push_str("#raw(\"");
                            out.push_str(&escape_typst_string(&src));
                            out.push_str("\", block: true)\n\n");
                        }
                    }
                } else {
                    let renderer_tag = match diagram.diagram_type() {
                        crate::diagram::DiagramType::Mermaid => "mermaid.svg",
                        crate::diagram::DiagramType::PlantUml => "plantuml.svg",
                        _ => unreachable!(),
                    };

                    // Apply theme content to the diagram source.  The cache
                    // key incorporates the themed source so that theme
                    // changes invalidate cached SVGs.
                    let render_src = match diagram.diagram_type() {
                        crate::diagram::DiagramType::PlantUml => {
                            crate::diagram::apply_plantuml_theme(
                                &src,
                                self.plantuml_theme.as_deref(),
                            )
                        }
                        crate::diagram::DiagramType::Mermaid => {
                            crate::diagram::apply_mermaid_theme(&src, self.mermaid_theme.as_deref())
                        }
                        _ => unreachable!(),
                    };
                    let svg_result = crate::diagram::cached_render_bytes(
                        &self.cache,
                        render_src.as_bytes(),
                        renderer_tag,
                        || match diagram.diagram_type() {
                            crate::diagram::DiagramType::Mermaid => {
                                crate::diagram::render_mermaid_svg_auto(None, &render_src)
                                    .map(|s| s.into_bytes())
                            }
                            crate::diagram::DiagramType::PlantUml => {
                                crate::diagram::render_plantuml_svg(None, render_src.as_bytes())
                                    .map_err(|e| anyhow::anyhow!("{e}"))
                            }
                            _ => unreachable!(),
                        },
                    );
                    match svg_result {
                        Ok(svg_bytes) => {
                            let svg = String::from_utf8_lossy(&svg_bytes);
                            // Embed the SVG inline via bytes() — image.decode is
                            // deprecated since Typst 0.13.
                            out.push_str("#image(\n  bytes(\"");
                            out.push_str(&escape_typst_string(&svg));
                            write!(out, "\"),\n  format: \"svg\"{size_args},\n)\n\n")?;
                        }
                        Err(e) => {
                            logger.log(format!(
                                "warning: failed to render {:?} diagram for Typst: {e}",
                                diagram.diagram_type()
                            ));
                            // Fallback: emit the raw diagram source as a code block.
                            out.push_str("#raw(\"");
                            out.push_str(&escape_typst_string(&src));
                            out.push_str("\", block: true)\n\n");
                        }
                    }
                }
            }
            KindData::Extension(_) => {}
            _ => {}
        }
        Ok(())
    }

    /// Default (fallback) admonition titles when no heading is present.
    fn admonition_default_title(adm_type: &str) -> &'static str {
        match adm_type {
            "note" => "Note",
            "warning" => "Warning",
            "important" => "Important",
            "caution" => "Caution",
            "tip" => "Tip",
            "related" => "Related",
            _ => "Note",
        }
    }

    fn render_admonition(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
        adm_type: &str,
    ) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        // Collect children; check if the first is a heading (used as title).
        let children: Vec<_> = arena[node_ref].children(arena).collect();
        let (title, skip_first) = if let Some(&first) = children.first() {
            if matches!(arena[first].kind_data(), KindData::Heading(_)) {
                // Render heading inline content as the title string.
                let mut title_buf = String::new();
                self.render_inline_children(&mut title_buf, context, first)?;
                (title_buf, true)
            } else {
                (Self::admonition_default_title(adm_type).to_string(), false)
            }
        } else {
            (Self::admonition_default_title(adm_type).to_string(), false)
        };

        write!(
            out,
            "#admonition(\"{}\", \"{}\")[",
            escape_typst_string(adm_type),
            escape_typst_string(&title),
        )?;

        let body_children = if skip_first {
            &children[1..]
        } else {
            &children[..]
        };
        for &child in body_children {
            self.render_block(out, context, child)?;
        }
        out.push(']');
        // Emit Typst label if the fenced div has an id attribute.
        if let Some(id) = arena[node_ref].attributes().get("id") {
            write!(out, " <{}>", id.str(source))?;
        }
        out.push_str("\n\n");
        Ok(())
    }

    fn render_definition_list(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        let tight = as_extension_data!(arena, node_ref, DefinitionList).is_tight();
        write!(out, "#terms(tight: {tight}")?;

        let children = arena[node_ref].children(arena).collect::<Vec<_>>();
        let mut index = 0usize;
        while index < children.len() {
            let mut terms = Vec::new();
            while index < children.len()
                && rushdown::matches_extension_kind!(arena, children[index], Term)
            {
                terms.push(children[index]);
                index += 1;
            }

            let mut definitions = Vec::new();
            while index < children.len()
                && rushdown::matches_extension_kind!(arena, children[index], TermDefinition)
            {
                definitions.push(children[index]);
                index += 1;
            }

            if terms.is_empty() {
                // Skip unrecognised child nodes (neither Term nor TermDefinition).
                index += 1;
                continue;
            }

            for term_ref in terms {
                out.push_str(", terms.item([");
                self.render_term(out, context, term_ref)?;
                out.push_str("], [");
                for definition_ref in &definitions {
                    for child in arena[*definition_ref].children(arena) {
                        self.render_block(out, context, child)?;
                    }
                }
                out.push_str("])");
            }
        }

        out.push_str(")\n\n");
        Ok(())
    }

    fn render_term(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        if arena[node_ref].children(arena).next().is_some() {
            return self.render_inline_children(out, context, node_ref);
        }

        let block = as_type_data!(arena, node_ref, Block);
        for (index, line) in block.source().iter().enumerate() {
            if index > 0 {
                out.push_str(" \\\n");
            }
            out.push_str(&escape_typst_markup(line.str(source).as_ref()));
        }
        Ok(())
    }

    fn render_table(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        let mut header_rows: Vec<NodeRef> = Vec::new();
        let mut body_rows: Vec<NodeRef> = Vec::new();
        for child in arena[node_ref].children(arena) {
            match arena[child].kind_data() {
                KindData::TableHeader(_) => {
                    for row in arena[child].children(arena) {
                        header_rows.push(row);
                    }
                }
                KindData::TableBody(_) => {
                    for row in arena[child].children(arena) {
                        body_rows.push(row);
                    }
                }
                _ => {}
            }
        }

        let has_header = !header_rows.is_empty();
        // Column count = sum of colspans in the first row. The first row has
        // no rowspan continuation from above, so its cells fully span every
        // column once.
        let column_count = match header_rows.first().or_else(|| body_rows.first()) {
            Some(&row) => arena[row]
                .children(arena)
                .map(|cell| cell_colspan(arena, source, cell))
                .sum::<usize>(),
            None => 0,
        };
        if column_count == 0 {
            return Ok(());
        }

        // Per-column alignment: walk every row with rowspan tracking so the
        // alignment of a single-column cell lands in its actual column, even
        // when earlier columns are occupied by a rowspan from a previous row.
        // First single-column cell encountered locks each column's alignment;
        // unlocked columns stay at the default "left".
        let mut aligns: Vec<&'static str> = vec!["left"; column_count];
        let mut align_locked: Vec<bool> = vec![false; column_count];
        let mut occupied_until: Vec<i32> = vec![-1; column_count];
        for (row_idx, &row) in header_rows.iter().chain(body_rows.iter()).enumerate() {
            let row_i = row_idx as i32;
            let mut col_idx = 0usize;
            for cell in arena[row].children(arena) {
                while col_idx < column_count && occupied_until[col_idx] >= row_i {
                    col_idx += 1;
                }
                if col_idx >= column_count {
                    break;
                }
                let cs = cell_colspan(arena, source, cell);
                let rs = cell_rowspan(arena, source, cell);
                if cs == 1 && !align_locked[col_idx] {
                    match as_kind_data!(arena, cell, TableCell).alignment() {
                        TableCellAlignment::Center => {
                            aligns[col_idx] = "center";
                            align_locked[col_idx] = true;
                        }
                        TableCellAlignment::Right => {
                            aligns[col_idx] = "right";
                            align_locked[col_idx] = true;
                        }
                        TableCellAlignment::Left => {
                            aligns[col_idx] = "left";
                            align_locked[col_idx] = true;
                        }
                        _ => {}
                    }
                }
                if rs > 1 {
                    let end = row_i + rs as i32 - 1;
                    let span_end = (col_idx + cs).min(column_count);
                    for slot in occupied_until.iter_mut().take(span_end).skip(col_idx) {
                        *slot = end;
                    }
                }
                col_idx += cs;
            }
        }

        // Emit via the theme wrapper so header styling can be suppressed for
        // headerless tables (the global `where(y: 0)` selector can't tell a
        // body row apart from a header row on its own).
        out.push_str("#data-table(\n");
        writeln!(out, "  has-header: {has_header},")?;
        writeln!(
            out,
            "  columns: ({}),",
            vec!["auto"; column_count].join(", ")
        )?;
        writeln!(out, "  align: ({}),", aligns.join(", "))?;
        if has_header {
            out.push_str("  table.header(\n");
            for &row in &header_rows {
                for cell in arena[row].children(arena) {
                    self.render_table_cell(out, context, cell, 4)?;
                }
            }
            out.push_str("  ),\n");
        }
        for &row in &body_rows {
            for cell in arena[row].children(arena) {
                self.render_table_cell(out, context, cell, 2)?;
            }
        }
        out.push_str(")\n\n");
        Ok(())
    }

    fn render_table_cell(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        cell: NodeRef,
        indent: usize,
    ) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        let colspan = cell_colspan(arena, source, cell);
        let rowspan = cell_rowspan(arena, source, cell);
        for _ in 0..indent {
            out.push(' ');
        }
        if colspan > 1 || rowspan > 1 {
            out.push_str("table.cell(");
            let mut first = true;
            if colspan > 1 {
                write!(out, "colspan: {colspan}")?;
                first = false;
            }
            if rowspan > 1 {
                if !first {
                    out.push_str(", ");
                }
                write!(out, "rowspan: {rowspan}")?;
            }
            out.push_str(")[");
            self.render_inline_children(out, context, cell)?;
            out.push_str("],\n");
        } else {
            out.push('[');
            self.render_inline_children(out, context, cell)?;
            out.push_str("],\n");
        }
        Ok(())
    }

    fn render_caption(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        let source = context.source;
        // Find the CaptionText child and the captioned block child.
        let mut block_child = None;
        let mut caption_text_child = None;
        for child in arena[node_ref].children(arena) {
            if rushdown::matches_extension_kind!(arena, child, crate::caption::CaptionText) {
                caption_text_child = Some(child);
            } else {
                block_child = Some(child);
            }
        }

        // Render: #figure(caption: [caption text])[block content] <label>
        // For listings, pass `kind: raw` so Typst numbers them separately.
        let caption_data = rushdown::as_extension_data!(arena, node_ref, crate::caption::Caption);
        if caption_data.kind() == crate::caption::CaptionKind::Listing {
            out.push_str("#figure(kind: raw, caption: [");
        } else {
            out.push_str("#figure(caption: [");
        }
        if let Some(ct) = caption_text_child {
            self.render_inline_children(out, context, ct)?;
        }
        out.push_str("])[\n");
        if let Some(block) = block_child {
            self.render_block(out, context, block)?;
        }
        out.push(']');
        if let Some(id) = arena[node_ref].attributes().get("id") {
            write!(out, " <{}>", id.str(source))?;
        }
        out.push_str("\n\n");
        Ok(())
    }

    fn render_inline_children(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let arena = context.arena;
        for child in arena[node_ref].children(arena) {
            self.render_inline(out, context, child)?;
        }
        Ok(())
    }

    fn render_inline(
        &self,
        out: &mut String,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        let prepared = context.prepared;
        let scope = context.scope;
        let arena = context.arena;
        let source = context.source;
        let known_labels = context.known_labels;
        let logger = context.logger;
        match arena[node_ref].kind_data() {
            KindData::Text(text) => {
                out.push_str(&escape_typst_markup(text.str(source)));
                if text.has_qualifiers(TextQualifier::HARD_LINE_BREAK) {
                    out.push_str(" \\\n");
                } else if text.has_qualifiers(TextQualifier::SOFT_LINE_BREAK) {
                    out.push('\n');
                }
            }
            KindData::CodeSpan(cs) => {
                write!(
                    out,
                    "#inline-code(\"{}\")",
                    escape_typst_string(&cs.str(source))
                )?;
            }
            KindData::Emphasis(_) => self.wrap_inline(out, "emph", context, node_ref)?,
            KindData::Strong(_) => self.wrap_inline(out, "strong", context, node_ref)?,
            KindData::Strikethrough(_) => self.wrap_inline(out, "strike", context, node_ref)?,
            KindData::Link(link) => {
                let dest = prepared
                    .graph()
                    .link_target(scope.doc_id, node_ref)
                    .ok_or_else(|| {
                        anyhow!(
                            "missing resolved link target for `{}` in {}",
                            link.destination_str(source),
                            prepared.graph().document(scope.doc_id).path.display()
                        )
                    })?
                    .to_string();
                match link.link_kind() {
                    LinkKind::Inline | LinkKind::Reference(_) => {
                        if dest.starts_with('#') {
                            let label = dest.trim_start_matches('#');
                            if !known_labels.contains(label) {
                                self.render_inline_children(out, context, node_ref)?;
                                return Ok(());
                            }
                            write!(out, "#link(<{}>)[", label)?;
                        } else {
                            write!(out, "#link(\"{}\")[", escape_typst_string(&dest))?;
                        }
                    }
                    LinkKind::Auto(auto) => {
                        write!(out, "#link(\"{}\")[", escape_typst_string(&dest))?;
                        out.push_str(&escape_typst_markup(auto.text_str(source)));
                        out.push(']');
                        return Ok(());
                    }
                    _ => {
                        write!(out, "#link(\"{}\")[", escape_typst_string(&dest))?;
                    }
                }
                self.render_inline_children(out, context, node_ref)?;
                out.push(']');
            }
            KindData::Image(image) => {
                let dest = prepared
                    .graph()
                    .image_target(scope.doc_id, node_ref)
                    .ok_or_else(|| {
                        anyhow!(
                            "missing resolved image target for `{}` in {}",
                            image.destination_str(source),
                            prepared.graph().document(scope.doc_id).path.display()
                        )
                    })?
                    .to_string();

                let attrs = arena[node_ref].attributes();
                let classes = attrs
                    .get("class")
                    .map(|v| v.str(source))
                    .unwrap_or_default();
                let has_inline_class = classes.split_whitespace().any(|c| c == "inline-image");
                let is_inline = has_inline_class || crate::html::is_inline_image(arena, node_ref);
                let width_attr = attrs.get("width").map(|v| v.str(source).to_string());
                let height_attr = attrs.get("height").map(|v| v.str(source).to_string());

                // Build the image(...) argument list (no leading #).
                let mut image_args = format!("\"{}\"", escape_typst_string(&dest));
                if let Some(w) = &width_attr {
                    write!(image_args, ", width: {w}")?;
                }
                if let Some(h) = &height_attr {
                    write!(image_args, ", height: {h}")?;
                }

                if is_inline {
                    // Delegate sizing to the template's inline-image()
                    // function so the theme controls inline image appearance.
                    write!(out, "#inline-image({image_args})")?;
                } else {
                    write!(out, "#image({image_args})")?;
                }

                if let Some(id) = attrs.get("id") {
                    write!(out, " <{}>", id.str(source))?;
                }
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(arena, node_ref, FootnoteReference) =>
            {
                let footnote = as_extension_data!(arena, node_ref, FootnoteReference);
                let label = footnote.label().str(source).to_string();
                let definition = prepared
                    .graph()
                    .document(scope.doc_id)
                    .footnote_definition(&label)
                    .ok_or_else(|| anyhow!("missing footnote definition for `{label}`"))?;
                let fragment = self.render_fragment(
                    prepared,
                    scope.doc_id,
                    definition.node_ref,
                    known_labels,
                    logger,
                )?;
                out.push_str("#footnote[");
                out.push_str(&fragment);
                out.push(']');
            }
            KindData::Extension(_)
                if rushdown::matches_extension_kind!(
                    arena,
                    node_ref,
                    crate::bracketed_span::BracketedSpan
                ) =>
            {
                self.render_inline_children(out, context, node_ref)?;
                if let Some(id) = arena[node_ref].attributes().get("id") {
                    write!(out, " <{}>", id.str(source))?;
                }
            }
            KindData::Extension(_) => {}
            _ => {}
        }
        Ok(())
    }

    fn wrap_inline(
        &self,
        out: &mut String,
        name: &str,
        context: &RenderContext<'_>,
        node_ref: NodeRef,
    ) -> Result<()> {
        write!(out, "#{name}[")?;
        self.render_inline_children(out, context, node_ref)?;
        out.push(']');
        Ok(())
    }

    fn render_fragment(
        &self,
        prepared: &PreparedDocument,
        doc_id: usize,
        definition_ref: NodeRef,
        known_labels: &HashSet<String>,
        logger: &DebugDumper,
    ) -> Result<String> {
        let doc = prepared.graph().document(doc_id);
        let mut out = String::new();
        let context = RenderContext {
            prepared,
            scope: RenderScope { doc_id },
            arena: &doc.arena,
            source: &doc.source,
            heading_shift: 0,
            known_labels,
            logger,
        };
        for child in doc.arena[definition_ref].children(&doc.arena) {
            self.render_block(&mut out, &context, child)?;
        }
        Ok(out)
    }
}

/// Escape a string for use inside Typst string literals (double-quoted `"..."`).
fn escape_typst_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Escape text for use in Typst markup (content) mode.
///
/// In markup mode the following characters have special meaning and must be
/// prefixed with a backslash:
///   \  #  *  _  `  $  <  >  @  ~
///
/// Quotes (" and ') become smart-quotes in Typst. We leave them unescaped
/// because smart-quoting is generally desirable in document output.
///
/// Line-start-sensitive characters (=, +, -, /) are only special at the
/// beginning of a line. Since the renderer controls line boundaries (heading
/// markup, list markup, etc.) these do not need escaping in inline text.
fn escape_typst_markup(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' | '#' | '*' | '_' | '`' | '$' | '<' | '>' | '@' | '~' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

fn cell_colspan(arena: &Arena, source: &str, cell: NodeRef) -> usize {
    arena[cell]
        .attributes()
        .get("colspan")
        .and_then(|v| v.str(source).parse::<usize>().ok())
        .unwrap_or(1)
}

fn cell_rowspan(arena: &Arena, source: &str, cell: NodeRef) -> usize {
    arena[cell]
        .attributes()
        .get("rowspan")
        .and_then(|v| v.str(source).parse::<usize>().ok())
        .unwrap_or(1)
}

fn collect_labels(prepared: &PreparedDocument, profile: &RenderProfile) -> Result<HashSet<String>> {
    let mut labels = HashSet::new();
    for (doc_id, node_ref, _) in prepared.graph().rendered_nodes_preorder_visible(profile)? {
        let doc = prepared.graph().document(doc_id);
        if let Some(id) = doc.arena[node_ref].attributes().get("id") {
            labels.insert(id.str(&doc.source).to_string());
        }
    }
    Ok(labels)
}
