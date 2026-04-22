# Formatter Design

## Goals

- Keep formatter logic isolated from build/render pipelines (`src/fmt/` module only).
- Apply deterministic formatting rules with minimal semantic risk.
- Support both write mode and CI-friendly check mode.

## Scope

The formatter processes one markdown file or all `.md` files under a directory.
It does not expand includes and does not resolve links. It is a source formatter,
not a document compiler.

## Rule Constants

Rules are configured in `src/fmt/rules.rs` and currently enabled by default:

- `PARAGRAPH_LINE_WIDTH = 100`
- `ALIGN_TABLES = true`
- `TRIM_TRAILING_WHITESPACE = true`
- `ENSURE_FINAL_NEWLINE = true`
- `MAX_CONSECUTIVE_BLANK_LINES = 1`

## Formatting Model

Formatting is AST-based and uses the same parser stack as the main engine:

1. Preserve YAML front matter exactly as written.
2. Parse markdown body with `crate::build::markdown_parser()`.
3. Apply caption AST transform (`CaptionAstTransformer`) for consistent caption nodes.
4. Render markdown from AST using `src/fmt/renderer.rs`.
5. Apply global cleanup rules (trailing whitespace, blank lines, final newline).

Important: formatter parsing intentionally skips diagram image transformation
(`transform_diagram_images`) and does not perform link resolution. The formatter
is source-oriented and operates on each file independently.

## Check Mode

`docbake fmt --check <path>` computes formatted output in memory and prints a
unified diff for each changed file. It exits with a non-zero status when at least
one file requires reformatting.

Diff rendering uses the `similar` crate:

- non-terminal output: plain unified diff (CI-friendly);
- terminal output (`stdout` is a terminal): ANSI-colored inline highlights.

## Tradeoffs

- AST rendering may canonicalize formatting details (for example fenced-div
  opening syntax), while preserving markdown semantics.
- Paragraph wrapping is applied to rendered paragraph text, with hard line breaks
  kept intact.
- Table alignment is derived from AST table nodes and cell alignments.

These choices prioritize structural correctness and parser-aligned behavior.
