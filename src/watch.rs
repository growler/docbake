use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use notify::RecursiveMode;
use notify_debouncer_mini::new_debouncer;

use crate::cache::Cache;
use crate::cli::{BrokenLinkMode, OutputFormat};
use crate::theme::Theme;
use crate::DebugDumper;

#[allow(clippy::too_many_arguments)]
pub(crate) fn watch_command(
    logger: &DebugDumper,
    cache: &Cache,
    theme: &Theme,
    format: OutputFormat,
    input: &Path,
    output: &Path,
    base: Option<&Path>,
    broken_links: BrokenLinkMode,
    meta_drops: &[String],
    meta_overrides: &[(String, String)],
) -> Result<()> {
    // Resolve base_dir once up front so we know what to watch.
    let (_source, base_dir) = crate::resolve_input_and_base(input, base)?;

    // Initial build.
    crate::build_command(
        logger,
        cache,
        theme,
        crate::BuildCommandOptions {
            format,
            input,
            output,
            base,
            broken_links,
            meta_drops,
            meta_overrides,
        },
    )?;

    // Clone everything the watcher callback needs.
    let logger_w = logger.clone();
    let cache_w = cache.clone();
    let theme_w = theme.clone();
    let input_w = input.to_path_buf();
    let output_w = output.to_path_buf();
    let base_w = base.map(Path::to_path_buf);
    let meta_drops_w: Vec<String> = meta_drops.to_vec();
    let meta_w: Vec<(String, String)> = meta_overrides.to_vec();
    let rebuild_lock = Mutex::new(());

    let _debouncer = {
        let mut debouncer = new_debouncer(Duration::from_millis(200), move |result| match result {
            Ok(_events) => {
                let _guard = rebuild_lock.lock().unwrap();
                eprintln!("File change detected, rebuilding\u{2026}");
                match crate::build_command(
                    &logger_w,
                    &cache_w,
                    &theme_w,
                    crate::BuildCommandOptions {
                        format,
                        input: &input_w,
                        output: &output_w,
                        base: base_w.as_deref(),
                        broken_links,
                        meta_drops: &meta_drops_w,
                        meta_overrides: &meta_w,
                    },
                ) {
                    Ok(()) => {
                        eprintln!("Rebuild complete.");
                    }
                    Err(e) => {
                        eprintln!("Rebuild failed: {e:#}");
                    }
                }
            }
            Err(e) => {
                eprintln!("File watcher error: {e}");
            }
        })
        .context("failed to create file watcher")?;

        debouncer
            .watcher()
            .watch(base_dir.as_ref(), RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch {}", base_dir.display()))?;

        if let Theme::Custom(ref theme_dir) = *theme {
            debouncer
                .watcher()
                .watch(theme_dir.as_ref(), RecursiveMode::Recursive)
                .with_context(|| {
                    format!("failed to watch theme directory {}", theme_dir.display())
                })?;
        }

        debouncer // keep alive; dropped on function exit
    };

    eprintln!("Watching for changes. Press Ctrl+C to stop.");

    // Block the main thread until the process is terminated (Ctrl+C).
    // The file watcher runs on its own thread; the debouncer must stay
    // in scope to remain active.
    loop {
        std::thread::sleep(Duration::from_secs(86400));
    }
}
