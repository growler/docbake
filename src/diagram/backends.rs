use super::*;

// ============================================================================
// Rendering backends
// ============================================================================

/// Collect diagram source text from the [`Diagram`] AST node into a single
/// [`String`].  Prefers `file_source` (from `.puml`/`.mmd` images) over
/// `value` (from fenced code blocks).
pub(crate) fn collect_diagram_source(diagram: &Diagram, source: &str) -> String {
    if let Some(fs) = diagram.file_source() {
        return fs.to_string();
    }
    let mut buf = String::new();
    for line in diagram.value().iter(source) {
        buf.push_str(&line);
    }
    buf
}

// ============================================================================
// Cache helpers
// ============================================================================

/// Render a diagram to SVG (as `String`), using the cache when available.
///
/// `content` is the raw bytes used to derive the cache key (typically the
/// diagram source text).  `renderer` is the cache tag (e.g. `"mermaid.svg"`).
/// `render_fn` is the fallback that performs the actual rendering on a cache
/// miss.
pub(crate) fn cached_render_svg(
    cache: &crate::cache::Cache,
    content: &[u8],
    renderer: &str,
    render_fn: impl FnOnce() -> anyhow::Result<String>,
) -> anyhow::Result<String> {
    if let Some(data) = cache.lookup(content, renderer) {
        return Ok(String::from_utf8(data)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()));
    }
    let svg = render_fn()?;
    // Best-effort cache store — do not fail the build on cache write errors.
    let _ = cache.store(content, renderer, svg.as_bytes());
    Ok(svg)
}

/// Render a diagram to raw bytes, using the cache when available.
///
/// Same as [`cached_render_svg`] but for renderers that produce opaque byte
/// output (e.g. PlantUML SVG as `Vec<u8>`, Draw.io PDF).
pub(crate) fn cached_render_bytes(
    cache: &crate::cache::Cache,
    content: &[u8],
    renderer: &str,
    render_fn: impl FnOnce() -> anyhow::Result<Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    if let Some(data) = cache.lookup(content, renderer) {
        return Ok(data);
    }
    let data = render_fn()?;
    let _ = cache.store(content, renderer, &data);
    Ok(data)
}

/// Build a cache key for a `.drawio` file.
///
/// The key is the file contents concatenated with the page number (if any),
/// so the cache correctly invalidates when the file changes or a different
/// page is requested.
pub(crate) fn drawio_cache_key(path: &Path, page: Option<u32>) -> Vec<u8> {
    let mut key = fs::read(path).unwrap_or_default();
    if let Some(p) = page {
        key.extend_from_slice(b"\0page:");
        key.extend_from_slice(p.to_string().as_bytes());
    }
    key
}

/// Render a Mermaid diagram to SVG using the `mermaid-rs-renderer` crate.
pub(super) fn render_mermaid_svg(src: &str) -> anyhow::Result<String> {
    mermaid_rs_renderer::render(src)
}

/// Render a Mermaid diagram to SVG by shelling out to `mmdc` (mermaid-cli).
///
/// If `command` is `None`, the binary is located via `which::which("mmdc")`.
fn render_mermaid_svg_cli(
    command: Option<&str>,
    src: &str,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("mmdc")?,
    };

    let mut child = Command::new(path)
        .args(["-i", "-", "-o", "-", "-e", "svg", "-q"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().ok_or("failed to open mmdc stdin")?;
        stdin.write_all(src.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Render a Mermaid diagram to SVG, trying `mmdc` first and falling back
/// to the built-in `mermaid-rs-renderer`.
pub(crate) fn render_mermaid_svg_auto(command: Option<&str>, src: &str) -> anyhow::Result<String> {
    let mmdc_available = match command {
        Some(_) => true,
        None => which::which("mmdc").is_ok(),
    };
    if mmdc_available {
        if let Ok(svg) = render_mermaid_svg_cli(command, src) {
            return Ok(svg);
        }
    }
    render_mermaid_svg(src)
}

/// Render a PlantUML diagram to SVG by shelling out to the `plantuml` binary.
///
/// If `command` is `None`, the binary is located via `which::which("plantuml")`.
pub(crate) fn render_plantuml_svg(
    command: Option<&str>,
    src: &[u8],
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("plantuml")?,
    };

    let mut child = Command::new(path)
        .args(["-tsvg", "-p", "-Djava.awt.headless=true"])
        .env("JAVA_OPTS", "-Djava.awt.headless=true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or("failed to open plantuml stdin")?;
        stdin.write_all(src)?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Typst document preamble for SVG rendering.
///
/// Sets page dimensions to `auto` with zero margins so the resulting SVG
/// is tightly cropped around the diagram content.
const TYPST_SVG_PREAMBLE: &str = "#set page(width: auto, height: auto, margin: 0pt)\n";

/// Render a Typst diagram to SVG by shelling out to `typst compile`.
///
/// The user's source is wrapped in a minimal document preamble
/// ([`TYPST_SVG_PREAMBLE`]) so that the produced SVG is tightly cropped.
///
/// If `command` is `None`, the binary is located via `which::which("typst")`.
pub(crate) fn render_typst_svg(
    command: Option<&str>,
    src: &str,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("typst")?,
    };

    let input = format!("{TYPST_SVG_PREAMBLE}{src}");

    let mut child = Command::new(path)
        .args(["compile", "--format", "svg", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().ok_or("failed to open typst stdin")?;
        stdin.write_all(input.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Output format for the Draw.io CLI renderer.
#[allow(dead_code)]
pub(crate) enum DrawioFormat {
    Svg,
    Pdf,
    Png,
}

/// Render a Draw.io diagram by shelling out to the `drawio` binary.
///
/// If `command` is `None`, the binary is located via `which::which("drawio")`.
/// `page` is an optional 1-based page index (passed as `-p <page>`).
///
/// The `drawio` CLI does not support stdout output, so a temporary file is
/// used to capture the result. Returns the raw output bytes regardless of
/// format; callers that need a `String` (e.g. for SVG) can convert via
/// `String::from_utf8`.
pub(crate) fn render_drawio(
    command: Option<&str>,
    input: &Path,
    page: Option<u32>,
    format: DrawioFormat,
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("drawio")?,
    };

    let (suffix, fmt_flag) = match format {
        DrawioFormat::Svg => (".svg", "svg"),
        DrawioFormat::Pdf => (".pdf", "pdf"),
        DrawioFormat::Png => (".png", "png"),
    };

    let tmp = tempfile::Builder::new().suffix(suffix).tempfile()?;
    let tmp_path = tmp.path().to_path_buf();

    let mut cmd = Command::new(path);
    cmd.arg("-x");
    if matches!(format, DrawioFormat::Svg) {
        cmd.args(["--embed-svg-fonts", "false"]);
    }
    cmd.args(["-f", fmt_flag, "--crop", "-o"]);
    cmd.arg(&tmp_path);
    if let Some(p) = page {
        cmd.args(["-p", &p.to_string()]);
    }
    cmd.arg(input);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd
        .output()
        .map_err(|e| format!("failed to run drawio: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }

    let data =
        fs::read(&tmp_path).map_err(|e| format!("failed to read drawio {fmt_flag} output: {e}"))?;
    Ok(data)
}

/// Render a D2 diagram to SVG by shelling out to the `d2` binary.
///
/// If `command` is `None`, the binary is located via `which::which("d2")`.
/// Uses `--no-xml-tag` to strip the `<?xml ...?>` header for cleaner
/// embedding in HTML and Typst.
pub(crate) fn render_d2_svg(
    command: Option<&str>,
    src: &str,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("d2")?,
    };

    let mut child = Command::new(path)
        .args(["--no-xml-tag", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().ok_or("failed to open d2 stdin")?;
        stdin.write_all(src.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

// ============================================================================
// PNG rendering functions (available for future use)
// ============================================================================

/// Render a PlantUML diagram to PNG by shelling out to the `plantuml` binary.
#[allow(dead_code)]
pub(crate) fn render_plantuml_png(
    command: Option<&str>,
    src: &[u8],
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("plantuml")?,
    };

    let mut child = Command::new(path)
        .args(["-tpng", "-p", "-Djava.awt.headless=true"])
        .env("JAVA_OPTS", "-Djava.awt.headless=true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or("failed to open plantuml stdin")?;
        stdin.write_all(src)?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Render a Mermaid diagram to PNG by shelling out to `mmdc`.
///
/// Unlike SVG rendering, PNG does not have a built-in fallback.
/// If `mmdc` is not available this function returns an error.
#[allow(dead_code)]
pub(crate) fn render_mermaid_png(
    command: Option<&str>,
    src: &str,
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("mmdc")?,
    };

    let tmp = tempfile::Builder::new().suffix(".png").tempfile()?;
    let tmp_path = tmp.path().to_path_buf();

    let mut child = Command::new(path)
        .args(["-i", "-", "-o"])
        .arg(&tmp_path)
        .args(["-e", "png", "-q"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().ok_or("failed to open mmdc stdin")?;
        stdin.write_all(src.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        let data =
            fs::read(&tmp_path).map_err(|e| format!("failed to read mmdc PNG output: {e}"))?;
        Ok(data)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Render a D2 diagram to PNG by shelling out to the `d2` binary.
#[allow(dead_code)]
pub(crate) fn render_d2_png(
    command: Option<&str>,
    src: &str,
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("d2")?,
    };

    let tmp = tempfile::Builder::new().suffix(".png").tempfile()?;
    let tmp_path = tmp.path().to_path_buf();

    let mut child = Command::new(path)
        .args(["-", &tmp_path.to_string_lossy()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().ok_or("failed to open d2 stdin")?;
        stdin.write_all(src.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        let data = fs::read(&tmp_path).map_err(|e| format!("failed to read d2 PNG output: {e}"))?;
        Ok(data)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Render a Typst diagram to PNG by shelling out to `typst compile`.
#[allow(dead_code)]
pub(crate) fn render_typst_png(
    command: Option<&str>,
    src: &str,
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let path = match command {
        Some(cmd) => std::path::PathBuf::from(cmd),
        None => which::which("typst")?,
    };

    let input = format!("{TYPST_SVG_PREAMBLE}{src}");

    let tmp = tempfile::Builder::new().suffix(".png").tempfile()?;
    let tmp_path = tmp.path().to_path_buf();

    let output = Command::new(path)
        .args(["compile", "--format", "png", "-"])
        .arg(&tmp_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .ok_or(std::io::Error::other("failed to open typst stdin"))?
                .write_all(input.as_bytes())?;
            child.wait_with_output()
        })?;

    if output.status.success() {
        let data =
            fs::read(&tmp_path).map_err(|e| format!("failed to read typst PNG output: {e}"))?;
        Ok(data)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}
