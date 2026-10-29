//! Internal diagram extension for rushdown.
//!
//! Supports:
//!
//! - **Mermaid** diagrams — client-side JS rendering (HTML), SVG via
//!   `mermaid-cli` (`mmdc`) with built-in `mermaid-rs-renderer` fallback
//!   for PDF output.
//! - **PlantUML** diagrams — server-side SVG via the external `plantuml` CLI.
//! - **Typst** diagrams — direct embedding in PDF output, SVG via `typst
//!   compile` for HTML output.
//! - **Draw.io** diagrams — SVG for HTML output, PDF for Typst/PDF output
//!   (avoids the `<foreignObject>` problem in Typst's SVG renderer).
//!   Rendering is deferred to the render stage so each renderer can choose
//!   the appropriate output format.
//!
//! Artifact caching is supported via the [`crate::cache::Cache`] struct
//! threaded through option structs.  When the cache is enabled, rendered
//! output is stored as `<blake3-hex>.<renderer>` and reused on subsequent
//! builds if the source content has not changed.

use std::any::TypeId;
use std::cell::RefCell;
use std::fmt;
use std::fmt::Write;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;

use anyhow::Context as _;

use rushdown::as_extension_data;
use rushdown::as_extension_data_mut;
use rushdown::as_kind_data;
use rushdown::as_type_data;
use rushdown::as_type_data_mut;
use rushdown::ast::walk;
use rushdown::ast::{
    pp_indent, Arena, KindData, NodeKind, NodeRef, NodeType, PrettyPrint, WalkStatus,
};
use rushdown::context::{BoolValue, ContextKey, ContextKeyRegistry};
use rushdown::matches_kind;
use rushdown::parser::{
    AnyAstTransformer, AstTransformer, Parser, ParserExtension, ParserExtensionFn, ParserOptions,
};
use rushdown::renderer::html::{Renderer, RendererExtension, RendererExtensionFn};
use rushdown::renderer::{
    self, BoxRenderNode, NodeRenderer, NodeRendererRegistry, PostRender, Render, RenderNode,
    RendererOptions, TextWrite,
};
use rushdown::text::{self, Reader as _};
use rushdown::Result;
use rushdown::{parser as rd_parser, renderer::html as rd_html};

mod backends;
mod html;
mod parser;
mod theme;
mod transform;
mod types;

#[allow(unused_imports)]
pub use html::{
    diagram_html_renderer_extension, DiagramHtmlRendererOptions, DiagramThemes, MermaidHtmlMode,
};
pub use parser::{diagram_parser_extension, DiagramParserOptions};
pub use types::{Diagram, DiagramType};

pub(crate) use backends::{
    cached_render_bytes, cached_render_svg, collect_diagram_source, drawio_cache_key,
    render_d2_png, render_d2_svg, render_drawio, render_mermaid_png, render_mermaid_svg_auto,
    render_plantuml_png, render_plantuml_svg, render_typst_png, render_typst_svg, DrawioFormat,
};
pub(crate) use theme::{apply_d2_theme, apply_mermaid_theme, apply_plantuml_theme};
pub(crate) use transform::{paragraph_has_sole_image, transform_diagram_images};

#[cfg(test)]
use backends::{plantuml_error, plantuml_java_tool_options, render_mermaid_svg};
#[cfg(test)]
use transform::{diagram_type_for_path, drawio_page_from_dest};

#[cfg(test)]
mod tests;
