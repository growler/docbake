---
title:   Documentation Build System
lang:    en-US
version: 0.1.0
logo: ../assets/logo.svg
confluence: 
  server: https://confluence.example.com
  space: DOCS
  path:
    - "Documentation"
    - "Documentation Build System"
---
::: {.warning if-format="confluence"}
Do not edit this page. Docbake generates this page from Markdown source files.
:::

# Purpose

Docbake validates technical documentation and generates artifacts for
publication and distribution. You write the documentation in Markdown with the
extensions of this project.

You can use the system to keep the documentation for a product, for its
components, and for its deployment projects.

The documentation is a [set of source files](document.md). You write these files
in [Markdown](markdown.md) with a number of extensions.

The output formats are single-page HTML, PDF made with Typst, and merged
Markdown. Docbake also generates SVG images of diagrams from PlantUML sources.
For this, it calls external tools.

The [builder](builder/index.md) builds and validates the documentation.

:::include
# [Document structure](document.md)
# [Markdown syntax](markdown.md)
# [Build system](builder/index.md)
# [TODO](TODO.md)
:::
