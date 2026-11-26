# GitHub Action

The docbake repository contains a GitHub Action. Other repositories use this
action to check and build their documentation in a GitHub Actions workflow.

The action does these steps:

1. It downloads the docbake release binary and verifies its SHA-256 checksum.
2. It installs the diagram tools that you select.
3. It runs `docbake check`. This step fails on broken links and on diagrams that
   the tools cannot render.
4. It runs `docbake build`.

The action runs only on Linux x64 runners, for example `ubuntu-latest`.

## Example

This workflow builds a multi-page HTML site and deploys it to GitHub Pages.
Replace `vX.Y.Z` with a docbake release tag.

~~~yaml
name: Docs

on:
  push:
    branches: [main]

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: growler/docbake@vX.Y.Z
        with:
          format: site
          base: doc
          input: doc/en/index.md
          output: site
          plantuml: true
          d2: true
      - uses: actions/upload-pages-artifact@v5
        with:
          path: site

  deploy:
    needs: build
    runs-on: ubuntu-latest
    permissions:
      pages: write
      id-token: write
    environment:
      name: github-pages
    steps:
      - uses: actions/deploy-pages@v5
~~~

To build a PDF, set `format: pdf`. To keep the PDF, add a step with
`actions/upload-artifact` and use the `output` output of the action as the path.

## Inputs

`input`
:  Root Markdown file. Required.

`output`
:  Output file or directory. Required.

`format`
:  Output format: `page`, `site`, `pdf`, `typst`, `markdown`, or `docx`. Required.

`base`
:  Base directory for absolute paths in includes and links. The default is the
   directory of the input file.

`theme`
:  Theme name or theme directory. The default is the built-in theme.

`check`
:  When `true`, the action runs `docbake check` before the build. The default is
   `true`. The `build` command exits with code 0 when a diagram tool fails. Keep
   this input set to `true` to make the workflow fail on these errors.

`version`
:  Release tag of docbake to download, for example `v0.3.0`. The default is the tag
   in the `uses:` line. Set this input when the `uses:` line refers to a branch or
   a commit.

`binary`
:  Path to a docbake binary. When you set this input, the action does not download
   docbake.

`plantuml`
:  When `true`, the action installs PlantUML. PlantUML needs Java. GitHub-hosted
   Ubuntu runners have Java.

`d2`
:  When `true`, the action installs D2.

`typst`
:  When `true`, the action installs Typst. The action always installs Typst when
   `format` is `pdf`.

`drawio`
:  When `true`, the action installs Draw.io and Xvfb with `sudo apt-get`.

The default value of all tool inputs is `false`. For Mermaid diagrams, the
Builder uses its built-in renderer when `mmdc` is not available.

## Outputs

`output`
:  Path of the build output. This is the value of the `output` input.
