use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "docbake",
    version,
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
    /// DOCX output using the same renderer as `publish gdoc`.
    /// Useful for inspecting and debugging Google Docs rendering locally.
    DocxGdoc,
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

        /// Override a front-matter value.  Repeatable.
        /// Key may use dot-notation (e.g. "confluence.server").
        #[arg(long = "meta", value_names = ["KEY", "VALUE"], num_args = 2)]
        meta: Vec<String>,

        /// Remove a front-matter key before applying --meta overrides.
        /// Repeatable.  Key may use dot-notation (e.g. "confluence.path").
        #[arg(long = "meta-drop", value_name = "KEY")]
        meta_drop: Vec<String>,
    },

    /// Dump the document include-graph for a markdown file.
    Dump {
        path: PathBuf,

        /// Base directory for absolute path resolution.
        /// Defaults to the parent directory of the input file.
        #[arg(short = 'b', long)]
        base: Option<PathBuf>,
    },

    /// Read a metadata value from the document's YAML front matter.
    Meta {
        /// Root markdown file.
        #[arg(short = 'i', long)]
        input: PathBuf,

        /// Metadata key (dot-separated for nested access, e.g. "confluence.server").
        key: String,
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

        /// Override a front-matter value.  Repeatable.
        /// Key may use dot-notation (e.g. "confluence.server").
        #[arg(long = "meta", value_names = ["KEY", "VALUE"], num_args = 2)]
        meta: Vec<String>,

        /// Remove a front-matter key before applying --meta overrides.
        /// Repeatable.  Key may use dot-notation (e.g. "confluence.path").
        #[arg(long = "meta-drop", value_name = "KEY")]
        meta_drop: Vec<String>,
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

        /// Override a front-matter value.  Repeatable.
        /// Key may use dot-notation (e.g. "confluence.server").
        #[arg(long = "meta", value_names = ["KEY", "VALUE"], num_args = 2)]
        meta: Vec<String>,

        /// Remove a front-matter key before applying --meta overrides.
        /// Repeatable.  Key may use dot-notation (e.g. "confluence.path").
        #[arg(long = "meta-drop", value_name = "KEY")]
        meta_drop: Vec<String>,
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

    /// Format markdown files in place.
    Fmt {
        /// Print diffs instead of writing formatted files.
        #[arg(long)]
        check: bool,

        /// Markdown file or directory containing markdown files.
        path: PathBuf,
    },

    /// Export the default theme to a directory for customisation.
    InitTheme {
        /// Target directory to create (must not already exist).
        target: PathBuf,
    },

    /// Publish a document to an external platform.
    Publish {
        #[command(subcommand)]
        target: PublishTarget,
    },

    /// Manage authentication credentials for external platforms.
    Auth {
        #[command(subcommand)]
        target: AuthTarget,
    },
}

#[derive(Debug, Subcommand)]
pub enum AuthTarget {
    /// Store a Confluence Personal Access Token (PAT).
    Confluence {
        /// Confluence server URL (e.g. https://confluence.example.com).
        server: String,

        /// Token value.  If omitted, the token is read from standard
        /// input (with a prompt when stdin is a terminal).
        token: Option<String>,
    },

    /// Authenticate with Google for Google Docs publishing.
    ///
    /// Without --service-account, opens a browser for OAuth2 consent
    /// and stores the refresh token for future use.
    ///
    /// With --service-account, stores the service account key file
    /// for JWT-based authentication (suitable for CI/CD).
    Gdoc {
        /// Path to a Google service account JSON key file.
        /// When provided, stores the key for JWT-based auth
        /// (no browser interaction).
        #[arg(long)]
        service_account: Option<std::path::PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum PublishTarget {
    /// Publish to Confluence.
    Confluence {
        /// Root markdown file.
        #[arg(short = 'i', long)]
        input: PathBuf,

        /// Base directory for absolute path resolution.
        /// Defaults to the parent directory of the input file.
        #[arg(short = 'b', long)]
        base: Option<PathBuf>,

        /// How to handle broken links: "warn" prints diagnostics and
        /// continues, "error" aborts.  Defaults to "error".
        #[arg(long, default_value = "error")]
        broken_links: BrokenLinkMode,

        /// Override a front-matter value.  Repeatable.
        /// Key may use dot-notation (e.g. "confluence.server").
        #[arg(long = "meta", value_names = ["KEY", "VALUE"], num_args = 2)]
        meta: Vec<String>,

        /// Remove a front-matter key before applying --meta overrides.
        /// Repeatable.  Key may use dot-notation (e.g. "confluence.path").
        #[arg(long = "meta-drop", value_name = "KEY")]
        meta_drop: Vec<String>,
    },

    /// Publish to Google Docs.
    ///
    /// Renders the document as DOCX and uploads it to Google Drive with
    /// automatic conversion to native Google Docs format.
    ///
    /// Use `google-docs.id` in front matter to update an existing document.
    /// Use `google-docs.folder` in front matter to specify the target folder.
    Gdoc {
        /// Root markdown file.
        #[arg(short = 'i', long)]
        input: PathBuf,

        /// Base directory for absolute path resolution.
        /// Defaults to the parent directory of the input file.
        #[arg(short = 'b', long)]
        base: Option<PathBuf>,

        /// How to handle broken links: "warn" prints diagnostics and
        /// continues, "error" aborts.  Defaults to "error".
        #[arg(long, default_value = "error")]
        broken_links: BrokenLinkMode,

        /// Override a front-matter value.  Repeatable.
        #[arg(long = "meta", value_names = ["KEY", "VALUE"], num_args = 2)]
        meta: Vec<String>,

        /// Remove a front-matter key before applying --meta overrides.
        #[arg(long = "meta-drop", value_name = "KEY")]
        meta_drop: Vec<String>,

        /// Write the new document ID back into the source file's front
        /// matter after creating a new Google Doc.
        #[arg(long)]
        write_id: bool,
    },
}
