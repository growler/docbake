use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "docbake",
    about = "Produce single-page HTML, multi-page HTML, or PDF from markdown"
)]
pub struct Cli {
    #[arg(long)]
    pub debug_dump: bool,

    /// Theme directory.  When omitted the built-in default theme is used.
    #[arg(short = 't', long)]
    pub theme: Option<PathBuf>,

    /// Override the default cache directory for rendered artifacts.
    #[arg(long)]
    pub cache: Option<PathBuf>,

    /// Disable artifact caching entirely.
    #[arg(long)]
    pub no_cache: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Html,
    HtmlMulti,
    Pdf,
    Markdown,
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

    /// Export the default theme to a directory for customisation.
    InitTheme {
        /// Target directory to create (must not already exist).
        target: PathBuf,
    },
}
