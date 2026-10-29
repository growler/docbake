# Styles Matrix (Draft)

This file is a working draft for a unified style naming model across outputs.

The primary goal is to make all the styled renderers to emit a semantic 
markup styled later by templates rather than emit styling directives.

Important:

- This list is **not complete**.
- Names are logical/semantic first, renderer-specific second.
- Some rows describe styles that should exist but are missing or only partially
  implemented.
- Caption naming is normalized to:
  - `table.caption`
  - `figure.caption`
  - `code.caption`
- Special constructions if-format and if-variant *are not styles*
  and should not be considered as such.

Legend:

- `x` - implemented
- `~` - to be implemented
- ` ` - not required

## Document And Navigation

| style                   | html | pdf | docx |
| ----------------------- | :--: | :-: | :--: |
| `document_title`        |  x   |  x  |  x   |
| `document_subtitle`     |  x   |  x  |  x   |
| `document_author`       |  x   |  x  |  x   |
| `document_date`         |  x   |  x  |  x   |
| `document_version`      |  x   |  x  |  x   |
| `toc`                   |  x   |  x  |  x   |
| `toc_page_tree` (site)  |  x   |     |      |
| `toc_page_local` (site) |  x   |     |      |
| `page.header`           |      |  x  |  x   |
| `page.footer`           |      |  x  |  x   |

## Headings, Text, And Blocks

| style                | html | pdf | docx |
| -------------------- | :--: | :-: | :--: |
| `heading_h1`         |  x   |  x  |  x   |
| `heading_h2`         |  x   |  x  |  x   |
| `heading_h3`         |  x   |  x  |  x   |
| `heading_h4`         |  x   |  x  |  x   |
| `heading_h5`         |  x   |  x  |  x   |
| `heading_h6`         |  x   |  x  |  x   |
| `heading_unlisted`   |  x   |     |      |
| `heading_unnumbered` |  x   |     |      |
| `heading_numbering`  |      |  x  |      |
| `text`               |  x   |  x  |  x   |
| `blockquote`         |  x   |  x  |  ~   |
| `rule.thematic`      |  x   |  x  |  ~   |
| `strong`             |  x   |  x  |  x   |
| `emphasize`          |  x   |  x  |  x   |
| `overstrike`         |  x   |  x  |  x   |
| `inline_code`        |  x   |  x  |  x   |

## Lists

| style                  | html | pdf | docx |
| ---------------------- | :--: | :-: | :--: |
| `list.unordered`       |  x   |  x  |  x   |
| `list.ordered`         |  x   |  x  |  x   |
| `list.level1`          |  x   |  x  |  x   |
| `list.level2`          |  x   |  x  |  x   |
| `list.level3`          |  x   |  x  |  x   |
| `list.level4`          |  x   |  x  |  ~   |
| `list.level5`          |  x   |  x  |  ~   |
| `list.level6`          |  x   |  x  |  ~   |
| `list.task.checked`    |  x   |  x  |  ~   |
| `list.task.unchecked`  |  x   |  x  |  ~   |
| `list.definition.term` |  x   |  x  |  x   |
| `list.definition.body` |  x   |  x  |  x   |

## Code And Captions (Uniform Naming)

| style                      | html | pdf | docx |
| -------------------------- | :--: | :-: | :--: |
| `code.block`               |  x   |  x  |  x   |
| `code.language`            |  x   |  x  |      |
| `code.line_numbers`        |  x   |  x  |      |
| `code.caption`             |  x   |  x  |  x   |
| `code.caption.numbering`   |      |  x  |      |
| `table.caption`            |  x   |  x  |  x   |
| `table.caption.numbering`  |      |  x  |      |
| `figure.caption`           |  x   |  x  |  x   |
| `figure.caption.numbering` |      |  x  |      |

## Tables, Figures, Images, Diagrams

| style                                       | html | pdf | docx |
| ------------------------------------------- | :--: | :-: | :--: |
| `table.block`                               |  x   |  x  |  x   |
| `table.header`                              |  x   |  x  |  x   |
| `table.cell`                                |  x   |  x  |  x   |
| `table.cell.align`                          |  x   |  x  |      |
| `figure.block` (dedicated figure container) |  x   |  x  |      |
| `image.block`                               |  x   |  x  |  x   |
| `image.inline`                              |  x   |  x  |  x   |
| `image.size` (`width`/`height`)             |  x   |  x  |      |
| `diagram`                                   |  x   |  x  |  x   |
| `diagram.mermaid`                           |  x   |  x  |  x   |
| `diagram.plantuml`                          |  x   |  x  |  x   |
| `diagram.typst`                             |  x   |  x  |  x   |
| `diagram.drawio`                            |  x   |  x  |  x   |
| `diagram.d2`                                |  x   |  x  |  x   |
| `diagram.as_code`                           |  x   |  x  |  x   |

## Admonitions (Explicit Structure)

| style                    | html | pdf | docx |
| ------------------------ | :--: | :-: | :--: |
| `admonition` (container) |  x   |  x  |  x   |
| `admonition.header`      |  x   |  x  |  x   |
| `admonition.body`        |  x   |  x  |  x   |
| `admonition.note`        |  x   |  x  |  x   |
| `admonition.tip`         |  x   |  x  |  x   |
| `admonition.warning`     |  x   |  x  |  x   |
| `admonition.caution`     |  x   |  x  |  x   |
| `admonition.important`   |  x   |  x  |  x   |
| `admonition.related`     |  x   |  x  |  x   |

## Links, Anchors, Footnotes, Spans

| style                             | html | pdf | docx |
| --------------------------------- | :--: | :-: | :--: |
| `link.external`                   |  x   |  x  |  ~   |
| `link.internal`                   |  x   |  x  |  ~   |
| `link.title` (tooltip/meta title) |  x   |     |      |
| `anchor.explicit`                 |  x   |  x  |      |
| `span.bracketed.id`               |  x   |  x  |      |
| `span.bracketed.class`            |  x   |     |      |
| `footnote.reference`              |  x   |  x  |  ~   |
| `footnote.body`                   |  x   |  x  |  ~   |
| `footnote.backref`                |  x   |     |      |

## Generic Containers And Attributes

| style                                      | html | pdf | docx |
| ------------------------------------------ | :--: | :-: | :--: |
| `fenced_div.generic`                       |  x   |  ~  |      |
| `fenced_div.class`                         |  x   |     |      |
| `fenced_div.attrs` (non-conditional attrs) |  x   |     |      |
| `conditional.if_format`                    |  x   |  x  |  x   |
| `conditional.if_variant`                   |  x   |  x  |  x   |

## Logical Styles That Should Exist (Target Model)

| style                            | html | pdf | docx |
| -------------------------------- | :--: | :-: | :--: |
| `caption.prefix.table`           |      |     |      |
| `caption.prefix.figure`          |      |     |      |
| `caption.prefix.code`            |      |     |      |
| `caption.position.table`         |      |     |      |
| `caption.position.figure`        |      |     |      |
| `caption.position.code`          |      |     |      |
| `link.external.icon`             |      |     |      |
| `link.visited`                   |      |     |      |
| `image.align`                    |      |     |      |
| `table.cell.vertical_align`      |      |     |      |
| `admonition.icon`                |      |     |      |
| `footnote.backref` (all formats) |  x   |     |      |

## DOCX Style IDs

This section gives the DOCX style for each semantic name that has `x` or `~`
in the docx column.

Each entry gives the style ID, the style type and the base style
(`w:basedOn`). For a numbering definition, the entry gives its key. The
style types are:

- paragraph style -- the ID goes in `w:pStyle`.
- character style -- the ID goes in `w:rStyle`.
- table style -- the ID goes in `w:tblStyle`.
- numbering style -- a style of the type `numbering` in `styles.xml`.
- numbering definition -- a `w:abstractNum` in `numbering.xml`. A list
  item paragraph refers to it through `w:numPr/w:numId`. The `w:numId`
  selects a `w:num`, and the `w:num` gives the `w:abstractNumId` of the
  definition. The definition has no `w:name`. The renderer identifies it
  by a fixed `w:abstractNumId`. The key names the definition in the
  renderer and in the theme file `docx.yaml`.

Rules for style IDs:

- A built-in Word style keeps its Word style ID and its English built-in
  name (`w:name`). Example: ID `Heading1`, name `heading 1`.
- A custom style has an ASCII PascalCase ID with no spaces. Its name has the
  same words with spaces. Example: ID `CodeBlock`, name `Code Block`.

Some semantic names have no DOCX style. Their entry gives the DOCX property
that the renderer uses.

The conditional blocks (`if-format`, `if-variant`) are not styles. This
section has no entry for them.

The style sheet also contains the Word base styles `DefaultParagraphFont`
(character style), `TableNormal` (table style) and `NoList` (numbering
style). No semantic name refers to them.

### Document And Navigation

`document_title`

:  `Title` -- paragraph style, based on `Normal`.

`document_subtitle`

:  `Subtitle` -- paragraph style, based on `Normal`.

`document_author`

:  `DocumentAuthor` -- paragraph style, based on `Normal`.

`document_date`

:  `DocumentDate` -- paragraph style, based on `Normal`.

`document_version`

:  `DocumentVersion` -- paragraph style, based on `Normal`.

`toc`

:  `TOC1` to `TOC9` -- paragraph styles, based on `Normal`. The number in the
   ID is the heading level of the entry.

`page.header`

:  `Header` -- paragraph style, based on `Normal`.

`page.footer`

:  `Footer` -- paragraph style, based on `Normal`.

### Headings, Text, And Blocks

`heading_h1`
`heading_h2`
`heading_h3`
`heading_h4`
`heading_h5`
`heading_h6`

:  `Heading1` to `Heading6` -- paragraph styles, based on `Normal`. The
   number in the ID is the heading level.

`text`

:  `Normal` -- paragraph style. All other paragraph styles derive from it
   through `w:basedOn`, directly or through another style.

`blockquote`

:  `Quote` -- paragraph style, based on `Normal`.

`rule.thematic`

:  `ThematicBreak` -- paragraph style, based on `Normal`. The paragraph is
   empty and has a bottom border.

`strong`

:  No style. The run has `w:b`.

`emphasize`

:  No style. The run has `w:i`.

`overstrike`

:  No style. The run has `w:strike`.

`inline_code`

:  `InlineCode` -- character style, based on `DefaultParagraphFont`.

### Lists

`list.unordered`

:  `BulletList` -- numbering definition. Each list item paragraph uses
   `ListParagraph`, a paragraph style based on `Normal`.

`list.ordered`

:  `OrderedList` -- numbering definition. Each list item paragraph uses
   `ListParagraph`, a paragraph style based on `Normal`.

`list.level1`
`list.level2`
`list.level3`
`list.level4`
`list.level5`
`list.level6`

:  No style. Each name is a level (`w:ilvl`) of the numbering definition.
   Level N is `w:ilvl` N-1: `list.level1` is `w:ilvl` 0, `list.level6` is
   `w:ilvl` 5.

`list.task.checked`
`list.task.unchecked`

:  `TaskList` -- numbering definition with bullet-free levels. Each task
   item paragraph uses `ListParagraph`. A checked or unchecked box
   character is at the start of the item.

`list.definition.term`

:  `DefinitionTerm` -- paragraph style, based on `Normal`.

`list.definition.body`

:  `DefinitionBody` -- paragraph style, based on `Normal`.

### Code And Captions

`code.block`

:  `CodeBlock` -- paragraph style, based on `Normal`.

`code.caption`
`table.caption`
`figure.caption`

:  `Caption` -- paragraph style, based on `Normal`. The number is a `SEQ`
   field. The field identifier is `Listing` for `code.caption`, `Table`
   for `table.caption` and `Figure` for `figure.caption`.

### Tables, Images, Diagrams

`table.block`

:  `DataTable` -- table style, based on `TableNormal`. Its `firstRow`
   conditional format sets the bold text and the fill of the header row.

`table.header`

:  `TableHeader` -- paragraph style, based on `TableCell`. The paragraphs in
   header cells use it. It sets only spacing and alignment.

`table.cell`

:  `TableCell` -- paragraph style, based on `Normal`. The paragraphs in body
   cells use it.

`image.block`

:  `ImageBlock` -- paragraph style, based on `Normal`. The paragraph contains
   only the image.

`image.inline`

:  No style. The image is an inline drawing (`wp:inline`) in a run of the
   paragraph.

`diagram`
`diagram.mermaid`
`diagram.plantuml`
`diagram.typst`
`diagram.drawio`
`diagram.d2`

:  `ImageBlock` -- paragraph style, based on `Normal`. The diagram is a PNG
   image in its own paragraph.

   `DiagramError` -- paragraph style, based on `Normal`. If the diagram
   tool cannot make the image, a paragraph with this style shows the error
   text.

`diagram.as_code`

:  `CodeBlock` -- paragraph style, based on `Normal`. The diagram source shows
   as a code block.

### Admonitions

`admonition`

:  No style. An admonition is one header paragraph and one or more body
   paragraphs. A paragraph has only one paragraph style. Thus a list, a
   code block or a table in an admonition uses its own style.

`admonition.header`

:  `AdmonitionHeader` -- paragraph style, based on `Normal`. It is the base
   of the header style of each admonition type.

`admonition.body`

:  `AdmonitionBody` -- paragraph style, based on `Normal`. It is the base of
   the body style of each admonition type.

`admonition.note`
`admonition.tip`
`admonition.warning`
`admonition.caution`
`admonition.important`
`admonition.related`

:  `Admonition<Type>Header` and `Admonition<Type>Body` -- paragraph styles,
   based on `AdmonitionHeader` and `AdmonitionBody`. `<Type>` is `Note`,
   `Tip`, `Warning`, `Caution`, `Important` or `Related`. Example:
   `AdmonitionNoteHeader` and `AdmonitionNoteBody`.

### Links And Footnotes

`link.external`
`link.internal`

:  `Hyperlink` -- character style, based on `DefaultParagraphFont`.

`footnote.reference`

:  `FootnoteReference` -- character style, based on `DefaultParagraphFont`.

`footnote.body`

:  `FootnoteText` -- paragraph style, based on `Normal`.

## Notes For Naming Consistency

- Keep semantic names stable (`figure.caption`, `table.caption`, `code.caption`)
  and map renderer details under the hood.
- Treat admonitions as 3 logical layers everywhere: `admonition`,
  `admonition.header`, `admonition.body`.
- Keep variant/type as suffixes (`admonition.warning`, `admonition.note`) rather
  than separate ad-hoc names.
- Prefer capability styles over backend terms (for example, `code.line_numbers`
  instead of `typst.start-line`).
- Expand this file together with renderer updates; do not treat it as final.
