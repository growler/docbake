use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::cache::Cache;
use crate::cli::BrokenLinkMode;
use crate::diagram::DiagramThemes;
use crate::graph::prepare_document;
use crate::DebugDumper;

use super::path_utils::compute_relative_href;
use super::{build_multi_page_html, build_multi_page_plan, RenderedPage};

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

fn write_custom_base_fixture(root: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
    let base_dir = root.path().to_path_buf();
    let root_doc = base_dir.join("docs/index.md");
    let part_doc = base_dir.join("docs/nested/part.md");
    let guide_doc = base_dir.join("shared/guide.md");

    write(
        &root_doc,
        "# Root\n\n[Guide](/shared/guide.md#sec)\n\n:::include\n# [Part](nested/part.md)\n:::\n",
    );
    write(
        &part_doc,
        "# Part\n\n[Guide From Part](/shared/guide.md#sec)\n\n:::include\n# [Guide](/shared/guide.md)\n:::\n",
    );
    write(&guide_doc, "# Guide\n\n## Section {#sec}\n");

    let canonical_base_dir = base_dir.canonicalize().unwrap();
    let canonical_root_doc = root_doc.canonicalize().unwrap();
    let root_source_dir = canonical_root_doc.parent().unwrap().to_path_buf();
    (canonical_base_dir, canonical_root_doc, root_source_dir)
}

fn page_html<'a>(pages: &'a [RenderedPage], path: &str) -> &'a str {
    let path = Path::new(path);
    pages
        .iter()
        .find(|page| page.path == path)
        .map(|page| page.html.as_str())
        .unwrap()
}

#[test]
fn relative_href_same_dir() {
    assert_eq!(
        compute_relative_href(Path::new("index.html"), Path::new("document.html")),
        "document.html"
    );
}

#[test]
fn relative_href_into_subdir() {
    assert_eq!(
        compute_relative_href(Path::new("index.html"), Path::new("builder/index.html")),
        "builder/index.html"
    );
}

#[test]
fn relative_href_up_from_subdir() {
    assert_eq!(
        compute_relative_href(Path::new("builder/index.html"), Path::new("index.html")),
        "../index.html"
    );
}

#[test]
fn relative_href_sibling_in_subdir() {
    assert_eq!(
        compute_relative_href(
            Path::new("builder/config.html"),
            Path::new("builder/formats.html")
        ),
        "formats.html"
    );
}

#[test]
fn relative_href_cross_dirs() {
    assert_eq!(
        compute_relative_href(Path::new("builder/config.html"), Path::new("document.html")),
        "../document.html"
    );
}

#[test]
fn plan_handles_docs_outside_root_source_with_custom_base() {
    let temp = TempDir::new().unwrap();
    let (base_dir, root_doc, root_source_dir) = write_custom_base_fixture(&temp);
    let prepared = prepare_document(
        &DebugDumper::new(false),
        &base_dir,
        &root_doc,
        Some("test"),
        BrokenLinkMode::Error,
    )
    .unwrap();

    let plan = build_multi_page_plan(&prepared, &root_source_dir, &base_dir).unwrap();
    let paths: Vec<PathBuf> = plan.pages.iter().map(|p| p.output_path.clone()).collect();

    assert!(paths.contains(&PathBuf::from("index.html")));
    assert!(paths.contains(&PathBuf::from("nested/part.html")));
    assert!(paths.contains(&PathBuf::from("shared/guide.html")));
}

#[test]
fn rewrites_absolute_markdown_links_with_custom_base() {
    let temp = TempDir::new().unwrap();
    let (base_dir, root_doc, _root_source_dir) = write_custom_base_fixture(&temp);
    let prepared = prepare_document(
        &DebugDumper::new(false),
        &base_dir,
        &root_doc,
        Some("test"),
        BrokenLinkMode::Error,
    )
    .unwrap();

    let context = tera::Context::new();
    let cache = Cache::new(None, true).unwrap();
    let pages = build_multi_page_html(
        "{{ body }}",
        &context,
        &prepared,
        &base_dir,
        &cache,
        &DiagramThemes::default(),
    )
    .unwrap();

    let root_html = page_html(&pages, "index.html");
    let part_html = page_html(&pages, "nested/part.html");

    assert!(root_html.contains("href=\"shared/guide.html#sec\""));
    assert!(part_html.contains("href=\"../shared/guide.html#sec\""));
}

#[test]
fn renders_page_footnotes_in_multi_page_output() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write(
        &root.join("doc/index.md"),
        "# Root\n\nText[^note].\n\n[^note]: Footnote body\n",
    );

    let prepared = prepare_document(
        &DebugDumper::new(false),
        root,
        &root.join("doc/index.md"),
        Some("test"),
        BrokenLinkMode::Error,
    )
    .unwrap();
    let pages = build_multi_page_html(
        "{{ body }}",
        &tera::Context::new(),
        &prepared,
        root,
        &Cache::new(None, true).unwrap(),
        &DiagramThemes::default(),
    )
    .unwrap();

    let root_html = page_html(&pages, "index.html");
    assert!(root_html.contains("class=\"footnote-ref\""));
    assert!(root_html.contains("<div class=\"footnotes\" role=\"doc-endnotes\">"));
    assert!(root_html.contains("class=\"footnote-backref\""));
}

#[test]
fn builds_expanded_and_intra_page_toc_for_current_page() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    write(
        &root.join("doc/index.md"),
        "# Root\n\n:::include\n# [Part](part.md)\n:::\n",
    );
    write(&root.join("doc/part.md"), "# Part\n\n## Inside\n");

    let prepared = prepare_document(
        &DebugDumper::new(false),
        root,
        &root.join("doc/index.md"),
        Some("test"),
        BrokenLinkMode::Error,
    )
    .unwrap();
    let pages = build_multi_page_html(
        "{{ toc_expanded }}\n---\n{{ toc_page }}",
        &tera::Context::new(),
        &prepared,
        root,
        &Cache::new(None, true).unwrap(),
        &DiagramThemes::default(),
    )
    .unwrap();

    let part_html = page_html(&pages, "part.html");
    assert!(part_html.contains("<li class=\"current\"><a href=\"part.html\">Part</a>"));
    assert!(part_html.contains("href=\"#inside\""));
    assert!(part_html.contains("Inside"));
    assert!(!part_html.contains("href=\"#part\""));
}
