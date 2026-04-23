mod diff;
mod discovery;
mod engine;
mod renderer;
mod rules;

use std::fs;
use std::io::IsTerminal;
use std::path::Path;

use anyhow::{bail, Context, Result};

pub(crate) fn format_markdown(input: &str) -> Result<String> {
    engine::format_markdown(input)
}

pub(crate) fn format_prepared_markdown(
    prepared: &crate::graph::PreparedDocument,
    profile: &crate::conditional::RenderProfile,
) -> Result<String> {
    engine::format_prepared_markdown(prepared, profile)
}

pub(crate) fn fmt_command(path: &Path, check: bool) -> Result<()> {
    let files = discovery::discover_markdown_files(path)?;
    if files.is_empty() {
        eprintln!("fmt: no markdown files found in {}", path.display());
        return Ok(());
    }

    let mut changed = 0usize;
    let ansi_inline = check && std::io::stdout().is_terminal();

    for file in files {
        let original = fs::read_to_string(&file)
            .with_context(|| format!("failed to read {}", file.display()))?;
        let formatted = format_markdown(&original)
            .with_context(|| format!("failed to format {}", file.display()))?;
        if original == formatted {
            continue;
        }

        changed += 1;
        if check {
            print!(
                "{}",
                diff::unified_file_diff(&file, &original, &formatted, ansi_inline)
            );
        } else {
            fs::write(&file, formatted)
                .with_context(|| format!("failed to write {}", file.display()))?;
            eprintln!("formatted {}", file.display());
        }
    }

    if check {
        if changed == 0 {
            eprintln!("fmt --check: all files are properly formatted");
            return Ok(());
        }
        bail!("fmt --check failed: {changed} file(s) need formatting");
    }

    eprintln!("fmt: {changed} file(s) formatted");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;

    #[test]
    fn check_mode_fails_when_reformatting_is_needed() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("doc.md");
        fs::write(
            &file,
            "This is a very long paragraph that should be wrapped by the formatter because it exceeds the configured width and needs normalization.\n",
        )?;

        let result = super::fmt_command(&file, true);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn directory_mode_formats_markdown_files_recursively() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let nested = dir.path().join("a").join("b");
        fs::create_dir_all(&nested)?;
        let md = nested.join("note.md");
        let txt = nested.join("note.txt");

        fs::write(&md, "A short line with trailing spaces.   \n")?;
        fs::write(&txt, "keep me")?;

        super::fmt_command(dir.path(), false)?;

        let md_after = fs::read_to_string(&md)?;
        let txt_after = fs::read_to_string(&txt)?;
        assert!(md_after.ends_with("\n"));
        assert_eq!(txt_after, "keep me");
        Ok(())
    }
}
