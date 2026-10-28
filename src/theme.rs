use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use include_dir::{include_dir, Dir, DirEntry};

use crate::cli::OutputFormat;
use crate::docx::params::DocxParams;

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
const DOCX_YAML: &str = "docx.yaml";
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

    /// Return logo bytes from theme assets (`assets/logo.*`) for the DOCX
    /// title page and page header.
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

    /// Return the DOCX style parameters from `docx.yaml`.
    ///
    /// A theme without `docx.yaml` uses the file of the embedded default
    /// theme.
    pub fn docx_params(&self) -> Result<DocxParams> {
        let (owner, text) = self.docx_yaml()?;
        DocxParams::from_yaml(&text)
            .with_context(|| format!("invalid {DOCX_YAML} in theme '{owner}'"))
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
                let target = target_dir.join(DOCX_YAML);
                if !target.exists() {
                    let (_, text) = self.docx_yaml()?;
                    fs::write(&target, text)
                        .with_context(|| format!("failed to write {}", target.display()))?;
                }
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

    /// Return the id of the theme that owns the `docx.yaml` in use, and the
    /// file text.
    fn docx_yaml(&self) -> Result<(String, String)> {
        if let Some(text) = self.read_text_file(DOCX_YAML)? {
            return Ok((self.id(), text));
        }
        // A broken symlink or a path that cannot be checked is an error. Only
        // a missing file selects the default theme.
        if let Theme::Custom(dir) = self {
            let path = dir.join(DOCX_YAML);
            match fs::symlink_metadata(&path) {
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(err).with_context(|| format!("failed to check {}", path.display()))
                }
                Ok(_) => bail!(
                    "{} is a symlink to a missing or inaccessible target",
                    path.display()
                ),
            }
        }
        let default = Theme::Embedded(EMBEDDED_DEFAULT_THEME);
        let text = default.read_text_file(DOCX_YAML)?.ok_or_else(|| {
            anyhow!("embedded theme '{EMBEDDED_DEFAULT_THEME}' has no {DOCX_YAML}")
        })?;
        Ok((default.id(), text))
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
                Ok(dir
                    .get_file(format!("{name}/{rel}"))
                    .map(|file| file.contents().to_vec()))
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
    use super::{Theme, DOCX_YAML, EMBEDDED_DEFAULT_THEME, EMBEDDED_THEME_NAMES};
    use crate::docx::params::{Border, BorderSpace, EighthPoints, LineSpacing, Rgb, Twips};
    use std::fs;
    use std::path::Path;

    /// The DOCX style IDs that a theme `docx.yaml` must define.
    const DOCX_STYLE_IDS: &[&str] = &[
        "Normal",
        "Title",
        "Subtitle",
        "DocumentAuthor",
        "DocumentDate",
        "DocumentVersion",
        "TOC1",
        "TOC2",
        "TOC3",
        "TOC4",
        "TOC5",
        "TOC6",
        "TOC7",
        "TOC8",
        "TOC9",
        "Header",
        "Footer",
        "Heading1",
        "Heading2",
        "Heading3",
        "Heading4",
        "Heading5",
        "Heading6",
        "Quote",
        "ThematicBreak",
        "InlineCode",
        "ListParagraph",
        "DefinitionTerm",
        "DefinitionBody",
        "CodeBlock",
        "Caption",
        "TableHeader",
        "TableCell",
        "ImageBlock",
        "AdmonitionHeader",
        "AdmonitionBody",
        "AdmonitionNoteHeader",
        "AdmonitionNoteBody",
        "AdmonitionTipHeader",
        "AdmonitionTipBody",
        "AdmonitionWarningHeader",
        "AdmonitionWarningBody",
        "AdmonitionCautionHeader",
        "AdmonitionCautionBody",
        "AdmonitionImportantHeader",
        "AdmonitionImportantBody",
        "AdmonitionRelatedHeader",
        "AdmonitionRelatedBody",
        "Hyperlink",
        "FootnoteReference",
        "FootnoteText",
    ];

    fn default_docx_yaml() -> String {
        Theme::Embedded(EMBEDDED_DEFAULT_THEME)
            .read_text_file(DOCX_YAML)
            .unwrap()
            .unwrap()
    }

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

    #[test]
    fn embedded_themes_have_docx_params() {
        for name in EMBEDDED_THEME_NAMES {
            let theme = Theme::Embedded(name);
            assert_eq!(
                theme.read_text_file(DOCX_YAML).unwrap(),
                Some(default_docx_yaml()),
                "{name}"
            );
            let params = theme.docx_params().unwrap();
            for id in DOCX_STYLE_IDS {
                assert!(params.styles.contains_key(*id), "{name}: {id}");
            }
            assert_eq!(params.styles.len(), DOCX_STYLE_IDS.len(), "{name}");
            for role in ["body", "heading", "mono"] {
                assert!(params.fonts.contains_key(role), "{name}: {role}");
            }
        }
    }

    #[test]
    fn blank_docx_params_match_template() {
        let params = Theme::Embedded("blank").docx_params().unwrap();
        let accent = Rgb([0xEE, 0x72, 0x1C]);
        let accent_rule = Border {
            width: EighthPoints(4),
            color: accent,
            space: BorderSpace(1),
        };

        let m = &params.page.margins;
        assert_eq!(
            [m.top, m.bottom, m.left, m.right, m.header, m.footer],
            [1134, 1134, 1134, 567, 851, 992].map(Twips)
        );
        assert_eq!(params.logo.title_height, Twips(522));
        assert_eq!(params.logo.header_height, Twips(210));
        let c = &params.table.cell_margins;
        assert_eq!(
            [c.top, c.bottom, c.left, c.right],
            [0, 0, 10, 10].map(Twips)
        );
        assert_eq!(params.table.border.width, EighthPoints(2));
        assert_eq!(params.table.border.color, Rgb([0, 0, 0]));
        assert!(params.table.header.bold);
        assert_eq!(params.table.header.fill, Rgb([0xF2, 0xF2, 0xF2]));
        assert_eq!(params.lists.indent, Twips(720));
        assert_eq!(params.lists.hanging, Twips(360));

        let normal = &params.styles["Normal"];
        assert_eq!(normal.font.as_deref(), Some("PT Sans"));
        assert_eq!(normal.size.map(|s| s.0), Some(22));
        assert_eq!(normal.after, Some(Twips(120)));
        assert_eq!(normal.line, Some(LineSpacing::Auto(240)));

        let h1 = &params.styles["Heading1"];
        assert_eq!(h1.based_on.as_deref(), Some("Normal"));
        assert_eq!(h1.font.as_deref(), Some("PT Sans"));
        assert_eq!(h1.size.map(|s| s.0), Some(56));
        assert_eq!(h1.bold, Some(true));
        assert_eq!(h1.color, Some(accent));
        assert_eq!(h1.before, Some(Twips(360)));
        assert_eq!(h1.borders.bottom, Some(accent_rule));

        let toc3 = &params.styles["TOC3"];
        assert_eq!(toc3.indent, Some(Twips(400)));
        assert_eq!(toc3.line, Some(LineSpacing::Auto(276)));
        assert_eq!(toc3.snap_to_grid, Some(false));

        let title = &params.styles["Title"];
        assert_eq!(title.char_spacing.map(|s| s.0), Some(-10));
        assert_eq!(title.kern.map(|k| k.0), Some(28));
        assert_eq!(title.borders.bottom, Some(accent_rule));

        let code = &params.styles["CodeBlock"];
        assert_eq!(code.font.as_deref(), Some("PT Mono"));
        assert_eq!(code.contextual_spacing, Some(true));
        let code_border = |space| Border {
            width: EighthPoints(4),
            color: Rgb([0xD9, 0xD9, 0xD9]),
            space: BorderSpace(space),
        };
        assert_eq!(code.borders.top, Some(code_border(1)));
        assert_eq!(code.borders.bottom, Some(code_border(1)));
        assert_eq!(code.borders.left, Some(code_border(4)));
        assert_eq!(code.borders.right, Some(code_border(4)));

        assert_eq!(params.styles["Caption"].size.map(|s| s.0), Some(18));
    }

    #[test]
    fn custom_theme_without_docx_yaml_uses_default() {
        let dir = tempfile::tempdir().unwrap();
        let theme = Theme::Custom(dir.path().to_path_buf());
        let params = theme.docx_params().unwrap();
        assert_eq!(params.styles["Heading1"].size.map(|s| s.0), Some(56));
    }

    #[cfg(unix)]
    #[test]
    fn custom_theme_broken_docx_yaml_symlink_is_error() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(dir.path().join("missing.yaml"), dir.path().join(DOCX_YAML))
            .unwrap();
        let theme = Theme::Custom(dir.path().to_path_buf());
        assert!(theme.docx_params().is_err());
    }

    #[test]
    fn custom_theme_docx_yaml_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let text = default_docx_yaml().replace("  mono: PT Mono", "  mono: Courier New");
        fs::write(dir.path().join(DOCX_YAML), text).unwrap();
        let params = Theme::Custom(dir.path().to_path_buf())
            .docx_params()
            .unwrap();
        assert_eq!(params.fonts["mono"], "Courier New");
    }

    #[test]
    fn invalid_custom_docx_yaml_names_theme_and_key() {
        let dir = tempfile::tempdir().unwrap();
        let text = default_docx_yaml().replacen("before: 18pt", "before: 18", 1);
        fs::write(dir.path().join(DOCX_YAML), text).unwrap();
        let theme = Theme::Custom(dir.path().to_path_buf());
        let err = format!("{:#}", theme.docx_params().unwrap_err());
        let context = format!("invalid docx.yaml in theme '{}'", theme.id());
        assert!(err.starts_with(&context), "{err}");
        assert!(
            err.contains("styles.Heading1.before: missing unit"),
            "{err}"
        );
    }

    #[test]
    fn export_embedded_theme_writes_docx_yaml() {
        let dir = tempfile::tempdir().unwrap();
        for name in EMBEDDED_THEME_NAMES {
            let target = dir.path().join(name);
            Theme::Embedded(name).export_to(&target).unwrap();
            let expected = Theme::Embedded(name)
                .read_bytes_file(DOCX_YAML)
                .unwrap()
                .unwrap();
            assert_eq!(fs::read(target.join(DOCX_YAML)).unwrap(), expected);
        }
    }

    #[test]
    fn export_custom_theme_adds_default_docx_yaml() {
        let src = tempfile::tempdir().unwrap();
        fs::write(src.path().join("style.css"), "body {}").unwrap();
        let out = tempfile::tempdir().unwrap();
        let target = out.path().join("theme");
        Theme::Custom(src.path().to_path_buf())
            .export_to(&target)
            .unwrap();
        assert_eq!(
            fs::read_to_string(target.join(DOCX_YAML)).unwrap(),
            default_docx_yaml()
        );
        assert!(target.join("style.css").exists());
    }

    #[test]
    fn export_custom_theme_keeps_its_docx_yaml() {
        let src = tempfile::tempdir().unwrap();
        fs::write(src.path().join(DOCX_YAML), "custom").unwrap();
        let out = tempfile::tempdir().unwrap();
        let target = out.path().join("theme");
        Theme::Custom(src.path().to_path_buf())
            .export_to(&target)
            .unwrap();
        assert_eq!(
            fs::read_to_string(target.join(DOCX_YAML)).unwrap(),
            "custom"
        );
    }
}
