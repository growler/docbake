//! Logo selection for a document build.
//!
//! The `logo` front-matter field selects the document logo. If the field is
//! not set, the theme logo (`assets/logo.*`) is the document logo. If the
//! theme has no logo, the document has no logo.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::graph::resolve::{resolve_base_path, static_asset_path};
use crate::theme::Theme;

/// The file extensions of a logo.
const EXTENSIONS: [&str; 4] = ["svg", "png", "jpg", "jpeg"];

/// The logo of a document.
pub(crate) struct Logo {
    /// The contents of the logo file.
    pub data: Vec<u8>,
    /// The lowercase file extension: `svg`, `png`, `jpg` or `jpeg`.
    pub ext: String,
    /// The path of the logo relative to the output root.
    pub output_path: String,
    /// The file of a front-matter logo. The build copies it to
    /// `output_path`. `None` for the theme logo, which the build copies with
    /// the theme assets.
    pub source: Option<PathBuf>,
}

impl Logo {
    /// Selects the logo of a document. A relative `logo` path resolves from
    /// `source_dir`. A path that starts with `/` resolves from `base_dir`.
    pub(crate) fn resolve(
        theme: &Theme,
        metadata: &JsonMap<String, JsonValue>,
        base_dir: &Path,
        source_dir: &Path,
    ) -> Result<Option<Self>> {
        match metadata.get("logo") {
            Some(value) => Self::from_front_matter(value, base_dir, source_dir).map(Some),
            None => Self::from_theme(theme),
        }
    }

    fn from_front_matter(value: &JsonValue, base_dir: &Path, source_dir: &Path) -> Result<Self> {
        let Some(path) = value.as_str().filter(|path| !path.is_empty()) else {
            bail!("logo: the value must be a path to an image file");
        };
        let file = resolve_base_path(base_dir, source_dir, path)
            .with_context(|| format!("logo: cannot use {path}"))?;
        let ext = file
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|ext| EXTENSIONS.contains(&ext.as_str()))
            .ok_or_else(|| anyhow!("logo: {path} must be an SVG, PNG or JPEG file"))?;
        let data = fs::read(&file).with_context(|| format!("logo: failed to read {path}"))?;
        let output_path = static_asset_path(base_dir, &file)?
            .to_string_lossy()
            .replace('\\', "/");
        Ok(Self {
            data,
            ext,
            output_path,
            source: Some(file),
        })
    }

    fn from_theme(theme: &Theme) -> Result<Option<Self>> {
        Ok(theme.logo_asset()?.map(|(data, ext)| Self {
            output_path: format!("assets/logo.{ext}"),
            data,
            ext,
            source: None,
        }))
    }

    /// Sets the `logo` template variable to the logo path relative to the
    /// output root. Removes the variable if there is no logo.
    pub(crate) fn insert_into(logo: Option<&Self>, context: &mut tera::Context) {
        match logo {
            Some(logo) => context.insert("logo", &logo.output_path),
            None => {
                context.remove("logo");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{json, Map as JsonMap, Value as JsonValue};

    use super::Logo;
    use crate::theme::Theme;

    /// A project root `root/` with the document directory `doc/en`.
    struct Project {
        _dir: tempfile::TempDir,
        base: std::path::PathBuf,
        source_dir: std::path::PathBuf,
    }

    impl Project {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let base = dir.path().canonicalize().unwrap().join("root");
            let source_dir = base.join("doc/en");
            fs::create_dir_all(&source_dir).unwrap();
            Self {
                _dir: dir,
                base,
                source_dir,
            }
        }

        fn write(&self, rel: &str, data: &[u8]) {
            let path = self.base.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, data).unwrap();
        }

        fn resolve(&self, theme: &Theme, logo: Option<JsonValue>) -> anyhow::Result<Option<Logo>> {
            let mut metadata = JsonMap::new();
            if let Some(logo) = logo {
                metadata.insert("logo".to_string(), logo);
            }
            Logo::resolve(theme, &metadata, &self.base, &self.source_dir)
        }
    }

    /// A custom theme directory without a logo.
    fn theme_without_logo() -> (tempfile::TempDir, Theme) {
        let dir = tempfile::tempdir().unwrap();
        let theme = Theme::Custom(dir.path().to_path_buf());
        (dir, theme)
    }

    /// The SVG logo for tests.
    const LOGO_SVG: &[u8] = include_bytes!("testdata/logo.svg");

    /// A custom theme directory with the logo `assets/logo.svg`.
    fn theme_with_logo() -> (tempfile::TempDir, Theme) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("assets")).unwrap();
        fs::write(dir.path().join("assets/logo.svg"), LOGO_SVG).unwrap();
        let theme = Theme::Custom(dir.path().to_path_buf());
        (dir, theme)
    }

    fn error(result: anyhow::Result<Option<Logo>>) -> String {
        format!("{:#}", result.err().expect("resolve must fail"))
    }

    #[test]
    fn relative_path_resolves_from_source_dir() {
        let project = Project::new();
        project.write("doc/en/img/Logo.PNG", b"png");
        let (_dir, theme) = theme_without_logo();
        let logo = project
            .resolve(&theme, Some(json!("img/Logo.PNG")))
            .unwrap()
            .unwrap();
        assert_eq!(logo.data, b"png");
        assert_eq!(logo.ext, "png");
        assert_eq!(logo.output_path, "static/doc/en/img/Logo.PNG");
        assert_eq!(
            logo.source.unwrap(),
            project.base.join("doc/en/img/Logo.PNG")
        );
    }

    #[test]
    fn absolute_path_resolves_from_base_dir() {
        let project = Project::new();
        project.write("assets/logo.svg", b"<svg/>");
        let (_dir, theme) = theme_without_logo();
        let logo = project
            .resolve(&theme, Some(json!("/assets/logo.svg")))
            .unwrap()
            .unwrap();
        assert_eq!(logo.ext, "svg");
        assert_eq!(logo.output_path, "static/assets/logo.svg");
    }

    #[test]
    fn front_matter_logo_replaces_theme_logo() {
        let project = Project::new();
        project.write("doc/en/logo.jpg", b"jpg");
        let (_dir, theme) = theme_with_logo();
        let logo = project
            .resolve(&theme, Some(json!("logo.jpg")))
            .unwrap()
            .unwrap();
        assert_eq!(logo.data, b"jpg");
        assert_eq!(logo.output_path, "static/doc/en/logo.jpg");
    }

    #[test]
    fn theme_logo_is_the_default() {
        let project = Project::new();
        let (_dir, theme) = theme_with_logo();
        let logo = project.resolve(&theme, None).unwrap().unwrap();
        assert_eq!(logo.ext, "svg");
        assert_eq!(logo.output_path, "assets/logo.svg");
        assert!(logo.source.is_none());
    }

    #[test]
    fn no_logo_without_front_matter_and_theme_logo() {
        let project = Project::new();
        let (_dir, theme) = theme_without_logo();
        assert!(project.resolve(&theme, None).unwrap().is_none());
    }

    #[test]
    fn blank_theme_has_no_logo() {
        let project = Project::new();
        let theme = Theme::Embedded("blank");
        assert!(project.resolve(&theme, None).unwrap().is_none());
    }

    #[test]
    fn missing_file_is_an_error() {
        let project = Project::new();
        let (_dir, theme) = theme_without_logo();
        let message = error(project.resolve(&theme, Some(json!("missing.svg"))));
        assert!(
            message.starts_with("logo: cannot use missing.svg"),
            "{message}"
        );
    }

    #[test]
    fn unsupported_format_is_an_error() {
        let project = Project::new();
        project.write("doc/en/logo.gif", b"gif");
        let (_dir, theme) = theme_without_logo();
        let message = error(project.resolve(&theme, Some(json!("logo.gif"))));
        assert_eq!(message, "logo: logo.gif must be an SVG, PNG or JPEG file");
    }

    #[test]
    fn value_that_is_not_a_path_is_an_error() {
        let project = Project::new();
        let (_dir, theme) = theme_without_logo();
        for value in [json!(""), json!(null), json!(1), json!(["logo.svg"])] {
            let message = error(project.resolve(&theme, Some(value)));
            assert_eq!(message, "logo: the value must be a path to an image file");
        }
    }

    #[test]
    fn path_outside_base_dir_is_an_error() {
        let project = Project::new();
        project.write("../outside.svg", b"<svg/>");
        let (_dir, theme) = theme_without_logo();
        let message = error(project.resolve(&theme, Some(json!("../../../outside.svg"))));
        assert!(message.contains("escapes the project root"), "{message}");
    }

    /// Builds `doc/en/index.md` (front matter `front_matter`) in the format
    /// `format` with the theme `theme`. Gives the output path.
    fn build(
        project: &Project,
        theme: &Theme,
        front_matter: &str,
        format: crate::cli::OutputFormat,
        output: &str,
    ) -> std::path::PathBuf {
        project.write(
            "doc/en/index.md",
            format!("---\ntitle: T\n{front_matter}---\n\n# Heading\n\nText.\n").as_bytes(),
        );
        let output = project.base.join("out").join(output);
        crate::build_command(
            &crate::DebugDumper::new(false),
            &crate::cache::Cache::new(None, true).unwrap(),
            theme,
            crate::BuildCommandOptions {
                format,
                input: &project.source_dir.join("index.md"),
                output: &output,
                base: Some(&project.base),
                broken_links: crate::cli::BrokenLinkMode::Warn,
                meta_drops: &[],
                meta_overrides: &[],
                meta_file: None,
            },
        )
        .unwrap();
        output
    }

    /// The blank theme exported to a directory, with the logo
    /// `assets/logo.svg`.
    fn exported_theme_with_logo() -> (tempfile::TempDir, Theme) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme");
        Theme::Embedded("blank").export_to(&path).unwrap();
        fs::create_dir(path.join("assets")).unwrap();
        fs::write(path.join("assets/logo.svg"), LOGO_SVG).unwrap();
        (dir, Theme::Custom(path))
    }

    const FRONT_MATTER_LOGO: &str = "logo: img/logo.svg\n";

    #[test]
    fn typst_without_logo_has_no_image() {
        use crate::cli::OutputFormat;
        let project = Project::new();
        let theme = Theme::Embedded("blank");
        let dir = build(&project, &theme, "", OutputFormat::Typst, "typst");
        let typ = fs::read_to_string(dir.join("index.typ")).unwrap();
        assert!(!typ.contains("image(\""), "{typ}");
    }

    #[test]
    fn typst_uses_front_matter_logo() {
        use crate::cli::OutputFormat;
        let project = Project::new();
        project.write("doc/en/img/logo.svg", LOGO_SVG);
        let theme = Theme::Embedded("blank");
        let dir = build(
            &project,
            &theme,
            FRONT_MATTER_LOGO,
            OutputFormat::Typst,
            "typst",
        );
        let typ = fs::read_to_string(dir.join("index.typ")).unwrap();
        assert_eq!(
            typ.matches(r#"image("static/doc/en/img/logo.svg""#).count(),
            2,
            "{typ}"
        );
        assert!(dir.join("static/doc/en/img/logo.svg").is_file());
    }

    #[test]
    fn html_without_logo_has_no_logo_image() {
        use crate::cli::OutputFormat;
        let project = Project::new();
        let theme = Theme::Embedded("blank");
        let html = build(&project, &theme, "", OutputFormat::Html, "doc.html");
        let html = fs::read_to_string(html).unwrap();
        assert!(html.contains("<h1 class=\"title\">T</h1>"), "{html}");
        assert!(!html.contains("<img class=\"site-logo\""), "{html}");
    }

    #[test]
    fn html_uses_theme_logo() {
        use crate::cli::OutputFormat;
        let project = Project::new();
        let (_dir, theme) = exported_theme_with_logo();
        let html = build(&project, &theme, "", OutputFormat::Html, "doc.html");
        let html = fs::read_to_string(html).unwrap();
        assert!(
            html.contains(r#"<img class="site-logo" src="assets/logo.svg""#),
            "{html}"
        );
    }

    #[test]
    fn html_site_uses_front_matter_logo() {
        use crate::cli::OutputFormat;
        let project = Project::new();
        project.write("doc/en/img/logo.svg", LOGO_SVG);
        let theme = Theme::Embedded("blank");
        let site = build(
            &project,
            &theme,
            FRONT_MATTER_LOGO,
            OutputFormat::HtmlMulti,
            "site",
        );
        let html = fs::read_to_string(site.join("index.html")).unwrap();
        assert!(
            html.contains(r#"<img class="site-logo" src="static/doc/en/img/logo.svg""#),
            "{html}"
        );
        assert!(site.join("static/doc/en/img/logo.svg").is_file());
    }

    #[test]
    fn docx_embeds_front_matter_logo_only_when_set() {
        use crate::cli::OutputFormat;
        let has_media = |path: std::path::PathBuf| {
            let bytes = fs::read(path).unwrap();
            bytes.windows(11).any(|w| w == b"word/media/")
        };
        let project = Project::new();
        let theme = Theme::Embedded("blank");
        assert!(!has_media(build(
            &project,
            &theme,
            "",
            OutputFormat::Docx,
            "a.docx"
        )));
        project.write("doc/en/img/logo.svg", LOGO_SVG);
        assert!(has_media(build(
            &project,
            &theme,
            FRONT_MATTER_LOGO,
            OutputFormat::Docx,
            "b.docx"
        )));
    }

    #[test]
    fn insert_into_sets_and_removes_the_variable() {
        let project = Project::new();
        let (_dir, theme) = theme_with_logo();
        let logo = project.resolve(&theme, None).unwrap().unwrap();
        let mut context = tera::Context::new();
        context.insert("logo", "front-matter value");
        Logo::insert_into(Some(&logo), &mut context);
        assert_eq!(
            context.get("logo").unwrap().as_str(),
            Some("assets/logo.svg")
        );
        Logo::insert_into(None, &mut context);
        assert!(context.get("logo").is_none());
    }
}
