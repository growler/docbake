use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use include_dir::{include_dir, Dir, DirEntry};

use crate::cli::OutputFormat;

// ============================================================================
// Embedded themes
// ============================================================================

static EMBEDDED_THEMES_ROOT: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/themes");

pub(crate) const EMBEDDED_DEFAULT_THEME: &str = "blank";
const EMBEDDED_THEME_NAMES: &[&str] = &["blank", "blank", "digitaltide"];

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
/// `Embedded` resolves files from the binary-bundled theme tree.
/// `Custom` resolves files from a directory on the filesystem.
#[derive(Clone, Debug)]
pub(crate) enum Theme {
    Embedded(&'static str),
    Custom(PathBuf),
}

impl Theme {
    /// Resolve theme selection from CLI `-t/--theme`.
    ///
    /// Resolution order for provided values:
    /// 1. embedded top-level theme directory (`blank`, `blank`, `digitaltide`)
    /// 2. local filesystem directory
    pub fn resolve(selected: Option<&Path>) -> Result<Self> {
        match selected {
            None => Self::embedded_by_name(EMBEDDED_DEFAULT_THEME),
            Some(raw) => {
                if let Some(name) = top_level_name(raw) {
                    if EMBEDDED_THEME_NAMES.contains(&name) {
                        return Self::embedded_by_name(name);
                    }
                }

                let dir = raw
                    .canonicalize()
                    .with_context(|| format!("failed to resolve theme path {}", raw.display()))?;
                if !dir.is_dir() {
                    bail!("theme path is not a directory: {}", dir.display());
                }
                Ok(Theme::Custom(dir))
            }
        }
    }

    /// Human-readable theme id for diagnostics.
    pub fn id(&self) -> String {
        match self {
            Theme::Embedded(name) => (*name).to_string(),
            Theme::Custom(path) => path.display().to_string(),
        }
    }

    /// Filesystem directory to watch for theme changes.
    ///
    /// Embedded themes live in the binary and therefore have no watch path.
    pub fn watch_directory(&self) -> Option<&Path> {
        match self {
            Theme::Embedded(_) => None,
            Theme::Custom(dir) => Some(dir.as_path()),
        }
    }

    /// Return logo bytes from theme assets (`assets/logo.*`) for DOCX front page.
    pub fn logo_asset(&self) -> Result<Option<(Vec<u8>, String)>> {
        let candidates = [
            ("logo.svg", "svg"),
            ("logo.png", "png"),
            ("logo.jpg", "jpg"),
            ("logo.jpeg", "jpeg"),
        ];
        for (name, ext) in candidates {
            if let Some(data) = self.read_bytes_file(&format!("{ASSETS_DIR}/{name}"))? {
                return Ok(Some((data, ext.to_string())));
            }
        }
        Ok(None)
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

        self.read_text_file(name)?.ok_or_else(|| {
            anyhow!(
                "theme '{}' is missing required template {}",
                self.id(),
                name
            )
        })
    }

    /// Return the CSS content from `style.css`, if available.
    pub fn style_css(&self) -> Result<Option<String>> {
        self.read_text_file(STYLE_CSS)
    }

    /// Return the PlantUML theme content from `theme.puml`, if available.
    pub fn plantuml_theme(&self) -> Result<Option<String>> {
        self.read_optional_theme_file(THEME_PUML)
    }

    /// Return the Mermaid theme content from `theme.mmd`, if available.
    pub fn mermaid_theme(&self) -> Result<Option<String>> {
        self.read_optional_theme_file(THEME_MMD)
    }

    /// Return the D2 theme content from `theme.d2`, if available.
    pub fn d2_theme(&self) -> Result<Option<String>> {
        self.read_optional_theme_file(THEME_D2)
    }

    /// Copy the theme's `assets/` directory contents to `output_dir/assets/`.
    pub fn copy_assets_to(&self, output_dir: &Path) -> Result<()> {
        let target_dir = output_dir.join(ASSETS_DIR);
        match self {
            Theme::Embedded(name) => {
                let assets_path = format!("{name}/{ASSETS_DIR}");
                if let Some(src) = embedded_theme_dir(name)?.get_dir(assets_path) {
                    copy_embedded_dir_recursive(src, &target_dir)?;
                }
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

    /// Export the entire resolved theme to a directory (for `init-theme`).
    pub fn export_to(&self, target_dir: &Path) -> Result<()> {
        if target_dir.exists() {
            bail!("target directory already exists: {}", target_dir.display());
        }
        fs::create_dir_all(target_dir)
            .with_context(|| format!("failed to create {}", target_dir.display()))?;

        match self {
            Theme::Embedded(name) => {
                let src = embedded_theme_dir(name)?;
                copy_embedded_dir_recursive(src, target_dir)?;
            }
            Theme::Custom(dir) => {
                copy_dir_recursive(dir, target_dir)?;
            }
        }
        Ok(())
    }

    fn embedded_by_name(name: &str) -> Result<Self> {
        let name_static = EMBEDDED_THEME_NAMES
            .iter()
            .copied()
            .find(|candidate| *candidate == name)
            .ok_or_else(|| anyhow!("unknown embedded theme: {name}"))?;
        let _ = embedded_theme_dir(name_static)?;
        Ok(Theme::Embedded(name_static))
    }

    fn read_text_file(&self, rel: &str) -> Result<Option<String>> {
        match self {
            Theme::Embedded(name) => {
                let dir = embedded_theme_dir(name)?;
                match dir.get_file(format!("{name}/{rel}")) {
                    Some(file) => {
                        let text = file.contents_utf8().ok_or_else(|| {
                            anyhow!("embedded theme '{}' file {} is not UTF-8", name, rel)
                        })?;
                        Ok(Some(text.to_string()))
                    }
                    None => Ok(None),
                }
            }
            Theme::Custom(dir) => {
                let path = dir.join(rel);
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

    fn read_bytes_file(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        match self {
            Theme::Embedded(name) => {
                let dir = embedded_theme_dir(name)?;
                Ok(dir.get_file(format!("{name}/{rel}")).map(|file| file.contents().to_vec()))
            }
            Theme::Custom(dir) => {
                let path = dir.join(rel);
                if path.exists() {
                    Ok(Some(fs::read(&path).with_context(|| {
                        format!("failed to read {}", path.display())
                    })?))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Read an optional theme file and strip comment-only content.
    fn read_optional_theme_file(&self, name: &str) -> Result<Option<String>> {
        let Some(content) = self.read_text_file(name)? else {
            return Ok(None);
        };
        let trimmed = strip_comment_lines(&content);
        if trimmed.is_empty() {
            Ok(None)
        } else {
            Ok(Some(trimmed))
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn top_level_name(path: &Path) -> Option<&str> {
    let mut components = path
        .components()
        .skip_while(|component| matches!(component, Component::CurDir));
    match (components.next(), components.next()) {
        (Some(Component::Normal(value)), None) => value.to_str(),
        _ => None,
    }
}

fn embedded_theme_dir(name: &str) -> Result<&'static Dir<'static>> {
    EMBEDDED_THEMES_ROOT
        .get_dir(name)
        .ok_or_else(|| anyhow!("embedded theme directory '{}' is not bundled", name))
}

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

/// Recursively copy an embedded directory tree to filesystem.
fn copy_embedded_dir_recursive(src: &Dir<'_>, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    for entry in src.entries() {
        match entry {
            DirEntry::Dir(dir) => copy_embedded_dir_recursive(
                dir,
                &dst.join(dir.path().file_name().unwrap_or_default()),
            )?,
            DirEntry::File(file) => {
                let file_name = file.path().file_name().ok_or_else(|| {
                    anyhow!(
                        "embedded file path has no file name: {}",
                        file.path().display()
                    )
                })?;
                let target = dst.join(file_name);
                fs::write(&target, file.contents())
                    .with_context(|| format!("failed to write {}", target.display()))?;
            }
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

#[cfg(test)]
mod tests {
    use super::{Theme, EMBEDDED_DEFAULT_THEME};
    use std::path::Path;

    #[test]
    fn resolve_uses_blank_by_default() {
        let theme = Theme::resolve(None).unwrap();
        assert!(matches!(theme, Theme::Embedded(name) if name == EMBEDDED_DEFAULT_THEME));
    }

    #[test]
    fn resolve_prefers_embedded_name() {
        let theme = Theme::resolve(Some(Path::new("blank"))).unwrap();
        assert!(matches!(theme, Theme::Embedded("blank")));
    }

    #[test]
    fn resolve_falls_back_to_local_directory() {
        let dir = tempfile::tempdir().unwrap();
        let theme = Theme::resolve(Some(dir.path())).unwrap();
        assert!(matches!(theme, Theme::Custom(_)));
    }
}
