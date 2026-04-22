use std::path::Path;

/// Compute the relative prefix from a page's output path back to the output
/// root directory.
/// For `index.html` -> `""`, for `builder/config.html` -> `"../"`.
pub(super) fn compute_asset_prefix(page_output_path: &Path) -> String {
    let dir_depth = page_output_path
        .parent()
        .map(|p| p.components().count())
        .unwrap_or(0);
    "../".repeat(dir_depth)
}

/// Compute a relative href from `from_path` to `to_path`, where both are
/// relative paths from the same root (the document output directory).
pub(super) fn compute_relative_href(from: &Path, to: &Path) -> String {
    let from_dir = from.parent().unwrap_or(Path::new(""));
    let to_str = to.to_string_lossy().replace('\\', "/");
    let from_dir_str = from_dir.to_string_lossy().replace('\\', "/");

    if from_dir_str.is_empty() {
        return to_str;
    }

    // Split into components (early return above guarantees non-empty).
    let from_parts: Vec<&str> = from_dir_str.split('/').collect();
    let to_parts: Vec<&str> = to_str.split('/').collect();

    // Find common prefix length.
    let common = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let ups = from_parts.len() - common;
    let mut result = String::new();
    for _ in 0..ups {
        result.push_str("../");
    }
    let remaining = &to_parts[common..];
    result.push_str(&remaining.join("/"));
    if result.is_empty() {
        // Same file.
        to.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        result
    }
}
