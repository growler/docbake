use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "docbake",
    about = "Produce single-page HTML, multi-page HTML, or PDF from markdown"
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub debug_dump: bool,

    /// Theme directory.  When omitted the built-in default theme is used.
    #[arg(short = 't', long, global = true)]
    pub theme: Option<PathBuf>,

    /// Override the default cache directory for rendered artifacts.
    #[arg(long, global = true)]
    pub cache: Option<PathBuf>,

    /// Disable artifact caching entirely.
    #[arg(long, global = true)]
    pub no_cache: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Html,
    HtmlMulti,
    Pdf,
    Typst,
    Markdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BrokenLinkMode {
    /// Report broken links as warnings and continue.
    Warn,
    /// Treat broken links as errors and abort.
    Error,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Build a document in the specified format.
    Build {
        /// Output format.
        #[arg(short = 'f', long)]
        format: OutputFormat,

        /// Root markdown file.
        #[arg(short = 'i', long)]
        input: PathBuf,

        /// Output file or directory.
        #[arg(short = 'o', long)]
        output: PathBuf,

        /// Base directory for absolute path resolution.
        /// Defaults to the parent directory of the input file.
        #[arg(short = 'b', long)]
        base: Option<PathBuf>,

        /// How to handle broken links: "warn" prints diagnostics and
        /// continues, "error" aborts the build.  Defaults to "error".
        #[arg(long, default_value = "error")]
        broken_links: BrokenLinkMode,
    },

    /// Dump the document include-graph for a markdown file.
    Dump {
        path: PathBuf,

        /// Base directory for absolute path resolution.
        /// Defaults to the parent directory of the input file.
        #[arg(short = 'b', long)]
        base: Option<PathBuf>,
    },

    /// Dump the parsed AST for a markdown file.
    Parse {
        path: PathBuf,

        /// Base directory for absolute path resolution.
        /// Defaults to the parent directory of the input file.
        #[arg(short = 'b', long)]
        base: Option<PathBuf>,
    },

    /// Serve a document with live-reload on file changes.
    Serve {
        /// Output format (only html and html-multi are supported).
        #[arg(short = 'f', long)]
        format: OutputFormat,

        /// Root markdown file.
        #[arg(short = 'i', long)]
        input: PathBuf,

        /// Base directory for absolute path resolution.
        /// Defaults to the parent directory of the input file.
        #[arg(short = 'b', long)]
        base: Option<PathBuf>,

        /// Port to listen on.  If not specified, an available port is
        /// chosen automatically.
        #[arg(short = 'p', long)]
        port: Option<u16>,

        /// How to handle broken links: "warn" prints diagnostics and
        /// continues, "error" aborts the build.  Defaults to "warn".
        #[arg(long, default_value = "warn")]
        broken_links: BrokenLinkMode,
    },

    /// Watch source files and rebuild on changes.
    Watch {
        /// Output format.
        #[arg(short = 'f', long)]
        format: OutputFormat,

        /// Root markdown file.
        #[arg(short = 'i', long)]
        input: PathBuf,

        /// Output file or directory.
        #[arg(short = 'o', long)]
        output: PathBuf,

        /// Base directory for absolute path resolution.
        /// Defaults to the parent directory of the input file.
        #[arg(short = 'b', long)]
        base: Option<PathBuf>,

        /// How to handle broken links: "warn" prints diagnostics and
        /// continues, "error" aborts the build.  Defaults to "warn".
        #[arg(long, default_value = "warn")]
        broken_links: BrokenLinkMode,
    },

    /// Validate the document: check all links and render all diagrams.
    Check {
        /// Root markdown file.
        #[arg(short = 'i', long)]
        input: PathBuf,

        /// Base directory for absolute path resolution.
        /// Defaults to the parent directory of the input file.
        #[arg(short = 'b', long)]
        base: Option<PathBuf>,
    },

    /// Export the default theme to a directory for customisation.
    InitTheme {
        /// Target directory to create (must not already exist).
        target: PathBuf,
    },
}
