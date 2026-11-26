# docbake

A CLI tool that builds multi-file Markdown documentation into a number of output
formats: single-page HTML, multi-page HTML, PDF (with Typst), DOCX, Confluence,
and Google Docs.

You can run it in a CI pipeline to build and publish documentation.

The [documentation](doc/en/index.md) describes the supported syntax.

## Features

- Output formats: `page`, `site`, `pdf`, `typst`, `markdown`, `docx`
- Diagrams: PlantUML, Mermaid, D2, Typst, DrawIO -- docbake renders them to SVG
  and caches the result
- Themes: the built-in theme `blank` (the default) and custom themes
- Live reload: the `serve` command rebuilds the document when files change
- Validation: the `check` command validates links and renders all diagrams
- Formatting: the `fmt` command aligns tables and wraps paragraphs
- Publication: publication to Confluence Server through the REST API, and to
  Google Docs

## Usage

```bash
# Single-page HTML
docbake build -f page -i doc/en/index.md -o target/doc/output.html

# Multi-page HTML
docbake build -f site -i doc/en/index.md -o target/doc/html

# PDF
docbake build -f pdf -i doc/en/index.md -o target/doc/output.pdf

# Merged Markdown
docbake build -f markdown -i doc/en/index.md -o target/doc/output.md

# DOCX
docbake build -f docx -i doc/en/index.md -o target/doc/output.docx

# Live-reload server
docbake serve -f page -i doc/en/index.md

# Live-rebuild server (use it for PDF)
docbake watch -f pdf -i doc/en/index.md -o output.pdf

# Validate the document (links and diagrams) for a specific profile
docbake check -f html -i doc/en/index.md

# Format a Markdown file or directory
docbake fmt doc/en

# Check the formatting without a write (diff and exit code)
docbake fmt --check doc/en
```

The `-b/--base` flag sets the root directory for absolute paths in includes and
links. The default is the directory of the input file.

## Cross-compilation

`make mac-cross` cross-compiles on a macOS host with `cargo-zigbuild`. It builds
these targets:

- `x86_64-unknown-linux-musl`
- `x86_64-pc-windows-gnu`
- `aarch64-apple-darwin`
- `x86_64-apple-darwin`

## Themes

Export a theme to customize it (the default is `blank`):

```bash
docbake init-theme my-theme
```

The theme directory has this structure:

```
my-theme/
  template.html              # Tera template for single-page HTML
  template-multipage.html    # Tera template for multi-page HTML
  template.typ               # Tera template for Typst/PDF
  style.css                  # CSS (inserted as {{ style }})
  theme.puml                 # Theme for PlantUML diagrams
  theme.mmd                  # Theme for Mermaid diagrams
  theme.d2                   # Theme for D2 diagrams
  docx.yaml                  # DOCX style parameters
  assets/                    # Static files for the templates
```

Select a theme:

- `docbake -t blank build ...` -- the built-in theme, by name
- `docbake -t my-theme build ...` -- a local theme directory

## Publication to Confluence

The `publish confluence` command publishes a document to Confluence Server. Set
the connection parameters in the front matter of the input file or with CLI
flags:

```yaml
---
title: Document title
confluence:
  server: https://confluence.example.com
  space: PROJ
  path:
    - Parent Page            # find the page by title
    - id:12345678            # or by the page content ID
    - Page title
---
```

Each `path` item (and each `--parent` in the CLI) is a page title or a content
ID with the `id:` prefix. The last item is the name of the target page. The
other items are the chain of parent pages.

Use `id:` when the title of a parent page is long or can change.

Docbake uses a Confluence Personal Access Token. It reads the token from the
`CONFLUENCE_TOKEN` environment variable. If this variable is not set, docbake
uses the token that you stored with `docbake auth confluence <server>`. The
token owner must have write permission in the target Confluence space.

## Publication to Google Docs

The `publish gdoc` command renders the document as DOCX and uploads it to Google
Drive as a Google Docs document.

Before you use `docbake auth gdoc`, do these steps:

1. In Google Cloud, create an OAuth client of the type "Desktop app".

2. Give the client ID and the client secret to docbake. Use one of these methods:

   - Set the `DOCBAKE_GDOC_CLIENT_ID` and `DOCBAKE_GDOC_CLIENT_SECRET` environment
     variables.
   - Add the `client_id` and `client_secret` keys to the `gdoc` section of the
     credentials file `<config-dir>/docbake/credentials.json`.

   If both are set, the environment variables take precedence.

3. Run `docbake auth gdoc`. Docbake opens a browser for OAuth consent and stores
   the refresh token.

For CI/CD, use `docbake auth gdoc --service-account <key.json>` to store a
service account key.

## CI/CD

Examples of GitLab CI/CD jobs that build documentation.

```yaml
build:docs:html:
  stage: build
  script:
    - docbake build -f page -b doc -i doc/en/index.md -o public/index.html
  artifacts:
    paths:
      - public/

build:docs:html-multi:
  stage: build
  script:
    - docbake build -f site -b doc -i doc/en/index.md -o public
  artifacts:
    paths:
      - public/

build:docs:pdf:
  stage: build
  script:
    - docbake build -f pdf -b doc -i doc/en/index.md -o public/document.pdf
  artifacts:
    paths:
      - public/

build:docs:confluence:
  stage: build
  script:
    - |
      docbake publish confluence \
        --meta confluence.server https://confluence.example.com \
        --meta confluence.space PROJ \
        --meta confluence.path id:12345678 \
        --meta confluence.path "Document Publisher" \
        -b doc -i doc/en/index.md
```

For the Confluence job, set the `CONFLUENCE_TOKEN` CI/CD variable to a Personal
Access Token.
