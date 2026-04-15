# AGENTS.md — docbakes-rs

## What This Is

Single-crate Rust CLI (`docbake`) that compiles multi-file Markdown documentation into single-page HTML, multi-page HTML, or PDF (via Typst). Not a workspace.

## Key Architecture

- **Entrypoint**: `src/main.rs` → `docbake::run()` (in `src/lib.rs`)
- **CLI** (`src/cli.rs`): subcommands `build`, `dump`, `parse`, `init-theme`. Uses clap derive.
- **Core** (`src/lib.rs`): orchestration — `run()`, `build_command()`, output path resolution.
- **Graph** (`src/graph/`): document graph construction from `:::include` fenced-div directives, heading ID assignment, link rewriting, footnote collection. All tests live in `src/graph/mod.rs`.
- **HTML renderer** (`src/render_html.rs`): single-page HTML via `rushdown` HTML renderer + Tera templates.
- **HTML multi-page** (`src/render_html_multi.rs`): multi-page HTML output.
- **Typst/PDF renderer** (`src/render_typst.rs`): emits `.typ` markup, then shells out to external `typst compile` to produce PDF. **`typst` must be installed on PATH.**
- **Theme** (`src/theme.rs`): loads theme from a directory or embedded default. Convention-based file names.
- **Build helpers** (`src/build.rs`): markdown parser construction, template context building, asset copying, Typst compilation.

## Markdown Engine

Uses `rushdown` (crates.io) — **not** `pulldown-cmark` or `comrak`. The rushdown extension crates (`rushdown-footnote`, `rushdown-highlighting`, `rushdown-meta`, `rushdown-definition-list`, `rushdown-fenced-div`, `rushdown-link-attribute`) each have their own API. Consult crate docs or existing usage in `src/build.rs` before adding features.

The crate implements a number of own rushdown extensions: `src/bracketed_span.rs`, `src/caption.rs` and `src/diagram.rs`.

## CLI Usage

```bash
cargo build                          # build the tool
cargo test                           # run tests

# Build a document
cargo run -- build -f html -i doc/ru/index.md -o target/doc-out/builder.html
cargo run -- build -f pdf -i doc/ru/index.md -o target/doc-out/builder.pdf
cargo run -- build -f html-multi -i doc/ru/index.md -o target/doc-out/builder

# With a custom theme
cargo run -- -t my-theme build -f html -i doc/ru/index.md -o target/out.html

# Export default theme for customisation
cargo run -- init-theme my-theme

# Debug/development
cargo run -- dump doc/ru/index.md        # dump the include-graph
cargo run -- parse doc/ru/index.md       # dump parsed AST
```

### CLI Flags

Global: `--debug-dump`, `-t/--theme <dir>`, `--cache <dir>`, `--no-cache`

Build: `-f/--format <html|html-multi|pdf|markdown>`, `-i/--input <file.md>`, `-o/--output <target>`, `-b/--base <dir>`

No custom lint, fmt, or CI config exists — standard `cargo fmt`, `cargo clippy`, `cargo test` apply with Rust defaults.

## Constraints

- **MSRV: Rust 1.91** (set in `Cargo.toml`).
- PDF output requires the `typst` binary on PATH — it is invoked as a subprocess.
- Document content is in Russian (`doc/ru/`). The tool itself is language-agnostic.
- `doc/` is user-facing documentation for document writers on how to use this system — not test fixtures or sample data. Treat it as product content.

## Theme System

Themes are directories with files named by convention:

```
theme-dir/
  template.html              # Tera template for single-page HTML
  template-multipage.html    # Tera template for multi-page HTML
  template.typ               # Tera template for Typst/PDF
  style.css                  # CSS (auto-injected as {{ style }} in HTML templates)
  theme.puml                 # PlantUML theme (inserted after @start* in diagrams)
  theme.mmd                  # Mermaid theme (prepended to diagrams)
  theme.d2                   # D2 theme (prepended to diagrams)
  assets/                    # Static files referenced by templates
```

The default theme is embedded in the binary (`default_theme/` directory, compiled via `include_str!`). Use `init-theme` to export it for customisation.

Template context variables are documented in comments at the top of each default template file.

Artifact cache (rendered diagrams) uses platform-standard user cache directory by default (`~/.cache/docbake` on Linux, `~/Library/Caches/docbake` on macOS); override with `--cache <dir>` or disable with `--no-cache`.

## Testing

All tests are `#[cfg(test)]` in `src/graph/mod.rs` and other modules, using `tempfile` for filesystem fixtures. Tests cover include resolution, link rewriting, heading ID de-duplication, front matter stripping, footnotes, definition lists, captions, and diagram transforms.

## Cross-Compilation

The project uses `cargo-zigbuild` (via Nix flake) to cross-compile for distribution. After every major change, **all three targets must compile successfully**:

```bash
cargo build --release                                              # macOS aarch64 (native)
cargo zigbuild --release --target x86_64-unknown-linux-musl        # Linux x86_64 (static)
cargo zigbuild --release --target x86_64-pc-windows-gnu            # Windows x86_64
```

The Nix devshell (`flake.nix`) provides Rust with cross-compilation targets, `zig`, and `cargo-zigbuild`. `CARGO_ZIGBUILD_CACHE_DIR` is set to `target/zigbuild-cache` to keep build artifacts out of the project root.

## Include Mechanism

Documents use `:::include{path=relative/file.md}` fenced-div blocks for transclusion. The system:
- Resolves includes recursively with cycle detection
- Strips YAML front matter from non-root includes
- Rewrites `.md` link targets to `#heading-id` anchors
- De-duplicates heading IDs across repeated includes of the same file

The `-b/--base` flag sets the root directory for absolute path resolution (paths starting with `/` in includes and links). Defaults to the parent directory of the input file.
