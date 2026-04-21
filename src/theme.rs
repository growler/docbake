use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::cli::OutputFormat;

// ============================================================================
// Embedded default theme
// ============================================================================

const DEFAULT_TEMPLATE_HTML: &str = include_str!("../default_theme/template.html");
const DEFAULT_TEMPLATE_MULTIPAGE: &str = include_str!("../default_theme/template-multipage.html");
const DEFAULT_TEMPLATE_TYP: &str = include_str!("../default_theme/template.typ");
const DEFAULT_STYLE_CSS: &str = include_str!("../default_theme/style.css");
const DEFAULT_THEME_PUML: &str = include_str!("../default_theme/theme.puml");
const DEFAULT_THEME_MMD: &str = include_str!("../default_theme/theme.mmd");
const DEFAULT_THEME_D2: &str = include_str!("../default_theme/theme.d2");
const DEFAULT_LOGO_SVG: &[u8] = include_bytes!("../default_theme/assets/logo.svg");

/// Files included in the default theme for export via `init-theme`.
const DEFAULT_THEME_FILES: &[(&str, &str)] = &[
    ("template.html", DEFAULT_TEMPLATE_HTML),
    ("template-multipage.html", DEFAULT_TEMPLATE_MULTIPAGE),
    ("template.typ", DEFAULT_TEMPLATE_TYP),
    ("style.css", DEFAULT_STYLE_CSS),
    ("theme.puml", DEFAULT_THEME_PUML),
    ("theme.mmd", DEFAULT_THEME_MMD),
    ("theme.d2", DEFAULT_THEME_D2),
];

// ============================================================================
// Convention file names
// ============================================================================

const TEMPLATE_HTML: &str = "template.html";
const TEMPLATE_MULTIPAGE_HTML: &str = "template-multipage.html";
const TEMPLATE_TYP: &str = "template.typ";
const STYLE_CSS: &str = "style.css";
const THEME_PUML: &str = "theme.puml";
const THEME_MMD: &str = "theme.mmd";
const THEME_D2: &str = "theme.d2";
const ASSETS_DIR: &str = "assets";

// ============================================================================
// Theme
// ============================================================================

/// A theme provides templates, CSS, diagram theme files, and static assets.
///
/// `Default` uses the embedded default theme compiled into the binary.
/// `Custom` loads everything from a directory on the filesystem.
#[derive(Clone, Debug)]
pub(crate) enum Theme {
    Default,
    Custom(PathBuf),
}

impl Theme {
    /// Return logo bytes from theme assets (`assets/logo.*`) for DOCX front page.
    pub fn logo_asset(&self) -> Result<Option<(Vec<u8>, String)>> {
        match self {
            Theme::Default => Ok(Some((DEFAULT_LOGO_SVG.to_vec(), "svg".to_string()))),
            Theme::Custom(dir) => {
                let assets = dir.join(ASSETS_DIR);
                let candidates = [
                    ("logo.svg", "svg"),
                    ("logo.png", "png"),
                    ("logo.jpg", "jpg"),
                    ("logo.jpeg", "jpeg"),
                ];
                for (name, ext) in candidates {
                    let path = assets.join(name);
                    if path.exists() {
                        let data = fs::read(&path)
                            .with_context(|| format!("failed to read {}", path.display()))?;
                        return Ok(Some((data, ext.to_string())));
                    }
                }
                Ok(None)
            }
        }
    }

    /// Return the Tera template content for the given output format.
    pub fn template_for(&self, format: OutputFormat) -> Result<String> {
        let name = match format {
            OutputFormat::Html => TEMPLATE_HTML,
            OutputFormat::HtmlMulti => TEMPLATE_MULTIPAGE_HTML,
            OutputFormat::Pdf | OutputFormat::Typst => TEMPLATE_TYP,
            OutputFormat::Markdown | OutputFormat::DocxGdoc => {
                bail!("{format:?} output does not use a template")
            }
        };
        match self {
            Theme::Default => Ok(match format {
                OutputFormat::Html => DEFAULT_TEMPLATE_HTML.to_string(),
                OutputFormat::HtmlMulti => DEFAULT_TEMPLATE_MULTIPAGE.to_string(),
                OutputFormat::Pdf | OutputFormat::Typst => DEFAULT_TEMPLATE_TYP.to_string(),
                OutputFormat::Markdown | OutputFormat::DocxGdoc => unreachable!(),
            }),
            Theme::Custom(dir) => {
                let path = dir.join(name);
                fs::read_to_string(&path)
                    .with_context(|| format!("failed to read template {}", path.display()))
            }
        }
    }

    /// Return the CSS content from `style.css`, if available.
    pub fn style_css(&self) -> Result<Option<String>> {
        match self {
            Theme::Default => Ok(Some(DEFAULT_STYLE_CSS.to_string())),
            Theme::Custom(dir) => {
                let path = dir.join(STYLE_CSS);
                if path.exists() {
                    Ok(Some(fs::read_to_string(&path).with_context(|| {
                        format!("failed to read {}", path.display())
                    })?))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Return the PlantUML theme content from `theme.puml`, if available.
    pub fn plantuml_theme(&self) -> Result<Option<String>> {
        self.read_optional_theme_file(THEME_PUML, DEFAULT_THEME_PUML)
    }

    /// Return the Mermaid theme content from `theme.mmd`, if available.
    pub fn mermaid_theme(&self) -> Result<Option<String>> {
        self.read_optional_theme_file(THEME_MMD, DEFAULT_THEME_MMD)
    }

    /// Return the D2 theme content from `theme.d2`, if available.
    pub fn d2_theme(&self) -> Result<Option<String>> {
        self.read_optional_theme_file(THEME_D2, DEFAULT_THEME_D2)
    }

    /// Copy the theme's `assets/` directory contents to `output_dir/assets/`.
    pub fn copy_assets_to(&self, output_dir: &Path) -> Result<()> {
        let target_dir = output_dir.join(ASSETS_DIR);
        match self {
            Theme::Default => {
                fs::create_dir_all(&target_dir)
                    .with_context(|| format!("failed to create {}", target_dir.display()))?;
                fs::write(target_dir.join("logo.svg"), DEFAULT_LOGO_SVG)
                    .context("failed to write default logo.svg")?;
            }
            Theme::Custom(dir) => {
                let src = dir.join(ASSETS_DIR);
                if src.exists() && src.is_dir() {
                    copy_dir_recursive(&src, &target_dir)?;
                }
            }
        }
        Ok(())
    }

    /// Export the entire default theme to a directory (for `init-theme`).
    pub fn export_to(target_dir: &Path) -> Result<()> {
        if target_dir.exists() {
            bail!("target directory already exists: {}", target_dir.display());
        }
        fs::create_dir_all(target_dir)
            .with_context(|| format!("failed to create {}", target_dir.display()))?;

        for (name, content) in DEFAULT_THEME_FILES {
            let path = target_dir.join(name);
            fs::write(&path, content)
                .with_context(|| format!("failed to write {}", path.display()))?;
        }

        let assets_dir = target_dir.join(ASSETS_DIR);
        fs::create_dir_all(&assets_dir)
            .with_context(|| format!("failed to create {}", assets_dir.display()))?;
        fs::write(assets_dir.join("logo.svg"), DEFAULT_LOGO_SVG)
            .context("failed to write default logo.svg")?;

        Ok(())
    }

    /// Read an optional theme file.  For `Default`, returns the embedded
    /// content (trimmed; `None` if the embedded content is empty or
    /// comment-only).  For `Custom`, reads from the filesystem if the file
    /// exists.
    fn read_optional_theme_file(
        &self,
        name: &str,
        default_content: &str,
    ) -> Result<Option<String>> {
        match self {
            Theme::Default => {
                let trimmed = strip_comment_lines(default_content);
                if trimmed.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(trimmed))
                }
            }
            Theme::Custom(dir) => {
                let path = dir.join(name);
                if path.exists() {
                    let content = fs::read_to_string(&path)
                        .with_context(|| format!("failed to read {}", path.display()))?;
                    let trimmed = strip_comment_lines(&content);
                    if trimmed.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(trimmed))
                    }
                } else {
                    Ok(None)
                }
            }
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Recursively copy a directory tree.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    for entry in
        fs::read_dir(src).with_context(|| format!("failed to read directory {}", src.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    src_path.display(),
                    dst_path.display()
                )
            })?;
        }
    }
    Ok(())
}

/// Strip lines that are only comments (PlantUML `'`, Mermaid `%%` prefix,
/// D2 `#`) and return the remaining non-empty content.
fn strip_comment_lines(content: &str) -> String {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with('\'')
                && !trimmed.starts_with('#')
                && !trimmed.starts_with("%%")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
