use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

pub(crate) fn discover_markdown_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        if !is_markdown_file(path) {
            bail!("path is not a markdown file: {}", path.display());
        }
        return Ok(vec![path.to_path_buf()]);
    }

    if !path.is_dir() {
        bail!(
            "path does not exist or is not accessible: {}",
            path.display()
        );
    }

    let mut files = Vec::new();
    walk_directory(path, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk_directory(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(dir).with_context(|| format!("failed to read directory {}", dir.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            walk_directory(&path, out)?;
            continue;
        }
        if path.is_file() && is_markdown_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}
