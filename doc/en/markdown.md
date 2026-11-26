# Markdown

Markdown is a simple text markup language. You write text in a simple and
readable format. The text also contains sufficient information to parse it into
an abstract syntax tree. Then the builder translates the tree into the target
format.

There are many Markdown dialects. The documentation build system uses the
[CommonMark](https://spec.commonmark.org/) specification as the base syntax. The
system also supports these extensions:

- strikethrough text (`~~text~~`)
- [tables](markdown/tables-lists.md#tables) (pipe tables)
- [definition lists](markdown/tables-lists.md#definition-lists)
- [task lists](markdown/tables-lists.md#task-lists)
- [footnotes](markdown/links.md#footnotes)
- [syntax highlighting](markdown/text.md#code-blocks) in code blocks
- [document metadata](document.md#document-parameters) in YAML format (front
  matter)
- [element attributes](markdown/links.md#labels) (`{#id .class attr=value}`)
- [inline spans with attributes](markdown/links.md#labels) (`[text]{#id .class}`)
- [fenced blocks](builder/formats.md#format-specific-blocks)
  (`:::{if-format="html"}`, `:::{if-variant="enterprise"}`)
- [admonitions](markdown/admonitions.md) (`:::note`, `:::warning`, `:::tip`,
  `:::important`, `:::caution`, `:::related`)
- [file inclusion](document.md#include) (`:::include`)
- [captions for tables, diagrams, and
  listings](markdown/tables-lists.md#table-caption) (`Table:`, `Figure:`,
  `Listing:`)
- [PlantUML](markdown/diagrams.md#plantuml-diagrams),
  [Mermaid](markdown/diagrams.md#mermaid-diagrams),
  [D2](markdown/diagrams.md#d2-diagrams), and
  [Draw.io](markdown/diagrams.md#drawio-diagrams) diagrams

:::include
## [Text markup](markdown/text.md)
## [Links and footnotes](markdown/links.md)
## [Tables and lists](markdown/tables-lists.md)
## [Admonitions](markdown/admonitions.md)
## [Illustrations](markdown/illustrations.md)
## [Diagrams](markdown/diagrams.md)
:::
