use std::fmt::Write as _;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{anyhow, Result};
use regex::Regex;
use std::sync::LazyLock;

use crate::graph::PreparedDocument;

use super::{ConfluenceAttachment, ConfluenceRenderer, SVG_DEFAULT_BLOCK_WIDTH, SVG_INLINE_WIDTH};

impl<'a> ConfluenceRenderer<'a> {
    /// Emit `ac:width` / `ac:height` attributes for an `<ac:image>`.
    ///
    /// For SVG attachments Confluence requires an explicit pixel `ac:width`
    /// (otherwise SVGs render at 0x0). The rules are:
    ///
    /// - **Inline SVG** -> `ac:width="16"` (icon-sized).
    /// - **Block SVG, no explicit width** -> intrinsic width from the SVG
    ///   (capped at 800), or 800 if intrinsic width is unavailable.
    /// - **Block SVG, width=N%** -> percent of `min(intrinsic, 800)`.
    /// - **Block SVG, width=Npx or plain N** -> `ac:width="N"`.
    /// - **Non-SVG** -> pass through width/height as-is (browsers honour
    ///   intrinsic raster dimensions).
    ///
    /// `svg_data` should contain the raw SVG bytes when available (for
    /// diagrams and local image attachments) so that the intrinsic width
    /// can be extracted. Pass `None` for external URLs where the data is
    /// not available.
    pub(super) fn write_image_size_attrs(
        &self,
        out: &mut String,
        width: Option<&str>,
        height: Option<&str>,
        is_svg: bool,
        is_inline: bool,
        svg_data: Option<&[u8]>,
    ) {
        if is_svg {
            if is_inline {
                let _ = write!(out, " ac:width=\"{SVG_INLINE_WIDTH}\"");
            } else {
                let intrinsic = svg_data.and_then(parse_svg_intrinsic_width);
                let px = svg_width_to_pixels(width, intrinsic);
                let _ = write!(out, " ac:width=\"{px}\"");
            }
            // For SVGs we intentionally omit ac:height so the aspect ratio
            // is preserved by the browser.
            return;
        }
        // Non-SVG: pass through as before.
        if let Some(w) = width {
            let w = normalize_dimension(w);
            let _ = write!(out, " ac:width=\"{w}\"");
        }
        if let Some(h) = height {
            let h = normalize_dimension(h);
            let _ = write!(out, " ac:height=\"{h}\"");
        }
    }

    /// Register a local image as an attachment. Returns the filename
    /// to reference via `<ri:attachment>`.
    pub(super) fn resolve_image_attachment(
        &self,
        dest: &str,
        prepared: &PreparedDocument,
    ) -> Result<String> {
        // `dest` is in the form `static/relative/path.png` after graph
        // resolution. Strip the `static/` prefix to get the relative
        // path, then resolve against base_dir.
        let relative = dest.strip_prefix("static/").unwrap_or(dest);
        let full_path = self.base_dir.join(relative);
        let source_path = self.resolve_attachment_source_path(dest, &full_path, prepared);
        let key = source_path.to_string_lossy().to_string();

        // Check if already registered.
        if let Some(existing) = self.attachment_names.borrow().get(&key) {
            return Ok(existing.clone());
        }

        // Derive a unique filename (use the file's own name; disambiguate
        // if multiple files from different directories share a name).
        let original_name = source_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("attachment-{}", self.attachments.borrow().len()));
        let filename = {
            let names = self.attachment_names.borrow();
            let mut candidate = original_name.clone();
            let mut counter = 1u32;
            while names.values().any(|v| v == &candidate) {
                let stem = source_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let ext = source_path
                    .extension()
                    .map(|e| e.to_string_lossy().to_string())
                    .unwrap_or_default();
                candidate = if ext.is_empty() {
                    format!("{stem}-{counter}")
                } else {
                    format!("{stem}-{counter}.{ext}")
                };
                counter += 1;
            }
            candidate
        };

        // Read file data.
        let data = match std::fs::read(&source_path) {
            Ok(data) => data,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                self.logger.log(format!(
                    "warning: image not found: {}",
                    source_path.display()
                ));
                // Reserve filename for this exact source identity so another
                // file with the same basename does not reuse it.
                self.attachment_names
                    .borrow_mut()
                    .insert(key, filename.clone());
                return Ok(filename);
            }
            Err(err) => {
                return Err(anyhow!(
                    "failed to read image {}: {err}",
                    source_path.display()
                ));
            }
        };

        let hash = format!("blake3:{}", blake3::hash(&data).to_hex());
        self.attachment_names
            .borrow_mut()
            .insert(key, filename.clone());
        self.attachments.borrow_mut().push(ConfluenceAttachment {
            filename: filename.clone(),
            data,
            content_hash: hash,
        });

        Ok(filename)
    }

    /// Resolve source identity for a local attachment.
    ///
    /// Preference order:
    /// 1) canonicalized filesystem path when available;
    /// 2) exact match in `asset_targets` by full `static/...` target;
    /// 3) full joined path under `base_dir`.
    fn resolve_attachment_source_path(
        &self,
        dest: &str,
        full_path: &Path,
        prepared: &PreparedDocument,
    ) -> std::path::PathBuf {
        if let Ok(canonical) = full_path.canonicalize() {
            return canonical;
        }

        let dest_path = Path::new(dest);
        if let Some(exact) = prepared.asset_targets().iter().find(|candidate| {
            crate::graph::resolve::static_asset_path(self.base_dir, candidate)
                .map(|p| p == dest_path)
                .unwrap_or(false)
        }) {
            return exact.clone();
        }

        full_path.to_path_buf()
    }
}

/// Normalize a CSS dimension value to a plain pixel number for Confluence
/// `ac:width` / `ac:height` attributes.
fn normalize_dimension(value: &str) -> String {
    let trimmed = value.trim();
    // Already a plain number.
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return trimmed.to_string();
    }
    // Strip "px" suffix.
    if let Some(num) = trimmed.strip_suffix("px") {
        return num.trim().to_string();
    }
    // Percentage -- not directly supported by ac:width, pass through as-is.
    trimmed.to_string()
}

/// Extract the intrinsic pixel width from the root `<svg>` element.
///
/// Returns `Some(px)` when the `width` attribute is a plain number or ends
/// with `px`. Returns `None` for percentages, `em`, `pt`, viewBox-only
/// SVGs, or when the `<svg>` tag cannot be found.
pub(super) fn parse_svg_intrinsic_width(svg_data: &[u8]) -> Option<u32> {
    // Match the opening <svg ...> tag (single-line; SVG generators
    // typically emit it on one line).
    static RE_SVG_TAG: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<svg\b[^>]*>").unwrap());
    static RE_WIDTH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"\bwidth\s*=\s*\"([^\"]*)\""#).unwrap());

    let text = std::str::from_utf8(svg_data).ok()?;
    let svg_tag = RE_SVG_TAG.find(text)?.as_str();
    let width_val = RE_WIDTH.captures(svg_tag)?[1].trim().to_string();

    // Accept "94px" or "94"; reject "%", "em", "pt", etc.
    let num_str = width_val.strip_suffix("px").unwrap_or(&width_val);
    num_str.trim().parse::<u32>().ok()
}

/// Convert a user-specified width attribute to a pixel value for SVG images.
///
/// Confluence requires a numeric pixel `ac:width` for SVGs; without it
/// the image renders at 0x0.
///
/// When `intrinsic_width` is available (parsed from the SVG itself), we use
/// `min(intrinsic, 800)` as the effective base instead of a flat 800.
/// Percentages are then computed relative to this effective base.
pub(super) fn svg_width_to_pixels(width: Option<&str>, intrinsic_width: Option<u32>) -> u32 {
    let effective_base = match intrinsic_width {
        Some(w) => w.min(SVG_DEFAULT_BLOCK_WIDTH),
        None => SVG_DEFAULT_BLOCK_WIDTH,
    };

    let Some(w) = width else {
        return effective_base;
    };
    let trimmed = w.trim();
    // Percentage: "50%" -> 50% of effective_base
    if let Some(pct) = trimmed.strip_suffix('%') {
        if let Ok(n) = pct.trim().parse::<f64>() {
            return ((n / 100.0) * effective_base as f64).round() as u32;
        }
    }
    // Pixels: "200px" or "200" -- explicit user choice, pass through as-is.
    let num_str = trimmed.strip_suffix("px").unwrap_or(trimmed);
    if let Ok(n) = num_str.trim().parse::<u32>() {
        return n;
    }
    effective_base
}
