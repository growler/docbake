// ============================================================================
// Theme application helpers
// ============================================================================

/// Apply a PlantUML theme to a diagram source.
///
/// The theme content is inserted right after the first `@start*` line
/// (e.g. `@startuml`, `@startmindmap`).  Returns the source unchanged
/// when `theme` is `None`.
pub(crate) fn apply_plantuml_theme(src: &str, theme: Option<&str>) -> String {
    let Some(content) = theme else {
        return src.to_string();
    };
    if content.is_empty() {
        return src.to_string();
    }
    // Insert after the first line (@startuml, @startsalt, etc.)
    if let Some(pos) = src.find('\n') {
        let mut out = String::with_capacity(src.len() + content.len() + 1);
        out.push_str(&src[..pos + 1]);
        out.push_str(content);
        if !content.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&src[pos + 1..]);
        out
    } else {
        // Single-line source (unusual) — append theme.
        format!("{src}\n{content}")
    }
}

/// Apply a Mermaid theme to a diagram source.
///
/// The theme content (typically a `%%{init: ...}%%` directive) is prepended
/// before the diagram source.  If the source already contains `%%{init`,
/// the theme is skipped to avoid conflicts.  Returns the source unchanged
/// when `theme` is `None`.
pub(crate) fn apply_mermaid_theme(src: &str, theme: Option<&str>) -> String {
    let Some(content) = theme else {
        return src.to_string();
    };
    if content.is_empty() {
        return src.to_string();
    }
    // Don't clobber an existing init directive.
    if src.contains("%%{init") {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len() + content.len() + 1);
    out.push_str(content);
    if !content.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(src);
    out
}

/// Apply a D2 theme to a diagram source.
///
/// The theme content is prepended before the diagram source.  Returns
/// the source unchanged when `theme` is `None`.
pub(crate) fn apply_d2_theme(src: &str, theme: Option<&str>) -> String {
    let Some(content) = theme else {
        return src.to_string();
    };
    if content.is_empty() {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len() + content.len() + 1);
    out.push_str(content);
    if !content.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(src);
    out
}
