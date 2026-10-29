use super::*;

// ============================================================================
// AST node types
// ============================================================================

/// The type of diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramType {
    Mermaid,
    PlantUml,
    Typst,
    DrawIo,
    D2,
}

/// A diagram node in the AST, replacing a fenced code block with a
/// diagram-language info string (`mermaid`, `plantuml`), or an image
/// reference to a `.puml`/`.mmd`/`.drawio` file.
#[derive(Debug)]
pub struct Diagram {
    diagram_type: DiagramType,
    value: text::Lines,
    /// Diagram source read from an external file (`.puml`, `.mmd`).
    /// When set, takes precedence over `value` during rendering.
    file_source: Option<String>,
    /// Resolved filesystem path to a `.drawio` file.  Set by
    /// [`transform_diagram_images`] for deferred rendering — the actual
    /// conversion to SVG or PDF happens at render time so that each
    /// renderer can choose the appropriate output format.
    drawio_path: Option<PathBuf>,
    /// Optional 1-based page index within a multi-page `.drawio` file
    /// (extracted from a `#N` URL fragment, e.g. `diagram.drawio#2`).
    drawio_page: Option<u32>,
    /// The image destination as the document gives it, for a diagram from
    /// an image reference to a diagram file.
    destination: Option<String>,
}

impl Diagram {
    pub fn new(diagram_type: DiagramType) -> Self {
        Self {
            diagram_type,
            value: text::Lines::default(),
            file_source: None,
            drawio_path: None,
            drawio_page: None,
            destination: None,
        }
    }

    /// Creates a new [`Diagram`] whose source was read from an external file.
    /// `destination` is the image destination as the document gives it.
    pub fn from_file(diagram_type: DiagramType, content: String, destination: String) -> Self {
        Self {
            diagram_type,
            value: text::Lines::default(),
            file_source: Some(content),
            drawio_path: None,
            drawio_page: None,
            destination: Some(destination),
        }
    }

    /// Creates a new [`Diagram`] that references a `.drawio` file for
    /// deferred rendering.  The actual SVG/PDF conversion is performed at
    /// render time so that each renderer can choose the appropriate format.
    /// `destination` is the image destination as the document gives it.
    pub fn from_drawio(path: PathBuf, page: Option<u32>, destination: String) -> Self {
        Self {
            diagram_type: DiagramType::DrawIo,
            value: text::Lines::default(),
            file_source: None,
            drawio_path: Some(path),
            drawio_page: page,
            destination: Some(destination),
        }
    }

    /// Returns the type of the diagram.
    pub fn diagram_type(&self) -> DiagramType {
        self.diagram_type
    }

    /// Returns the lines of the diagram source (from a fenced code block).
    pub fn value(&self) -> &text::Lines {
        &self.value
    }

    /// Returns the file-sourced content, if any.
    pub fn file_source(&self) -> Option<&str> {
        self.file_source.as_deref()
    }

    /// Returns the resolved path to a `.drawio` file, if this diagram was
    /// created via [`Diagram::from_drawio`].
    pub fn drawio_path(&self) -> Option<&Path> {
        self.drawio_path.as_deref()
    }

    /// Returns the optional 1-based page index for a multi-page `.drawio`
    /// file.
    pub fn drawio_page(&self) -> Option<u32> {
        self.drawio_page
    }

    /// Returns the image destination as the document gives it, for a
    /// diagram from an image reference to a diagram file.
    pub fn destination(&self) -> Option<&str> {
        self.destination.as_deref()
    }

    /// Sets the diagram source lines (for code-block-based diagrams).
    pub fn set_value(&mut self, value: impl Into<text::Lines>) {
        self.value = value.into();
    }
}

impl NodeKind for Diagram {
    fn typ(&self) -> NodeType {
        NodeType::LeafBlock
    }

    fn kind_name(&self) -> &'static str {
        "Diagram"
    }
}

impl PrettyPrint for Diagram {
    fn pretty_print(&self, w: &mut dyn Write, source: &str, level: usize) -> fmt::Result {
        writeln!(
            w,
            "{}DiagramType: {:?}",
            pp_indent(level),
            self.diagram_type()
        )?;
        if let Some(file_src) = &self.file_source {
            write!(w, "{}FileSource: ", pp_indent(level))?;
            writeln!(w, "[ ")?;
            for line in file_src.lines() {
                writeln!(w, "{}{}", pp_indent(level + 1), line)?;
            }
            writeln!(w, "{}]", pp_indent(level))
        } else {
            write!(w, "{}Value: ", pp_indent(level))?;
            writeln!(w, "[ ")?;
            for line in self.value.iter(source) {
                write!(w, "{}{}", pp_indent(level + 1), line)?;
            }
            writeln!(w)?;
            writeln!(w, "{}]", pp_indent(level))
        }
    }
}

impl From<Diagram> for KindData {
    fn from(e: Diagram) -> Self {
        KindData::Extension(Box::new(e))
    }
}
