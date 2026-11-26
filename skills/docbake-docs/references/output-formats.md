# Output formats

## The formats

`page`
:  One linear HTML page holding the whole document. Alias `html`.

`site`
:  A set of HTML pages that mirrors the source structure. Each `.md` file
   becomes a page. Alias `html-multi`.

`pdf`
:  PDF produced through the Typst typesetting system.

`typst`
:  The intermediate `.typ` file, with no call to `typst compile`. Useful when
   debugging PDF layout.

`markdown`
:  One linear Markdown file, produced after include expansion, link rewriting,
   footnote collection, and heading level normalisation. Useful for exporting to
   plain Markdown systems and for feeding a document to an AI agent.

`docx`
:  A DOCX document. The same renderer makes the DOCX for `publish gdoc`. Alias:
   `docx-gdoc`.

Publishing targets are `publish confluence` and `publish gdoc`. `cli.md` gives
the commands.

## Visibility profiles

Conditional fenced divs (`if-format`, `if-variant`) are evaluated against a
profile. `SKILL.md` gives the syntax and the evaluation rules. There are five
profile names, and they are the only values `if-format` accepts:

| Command                          | Profile      |
| -------------------------------- | ------------ |
| `build -f page`, `build -f site` | `html`       |
| `build -f pdf`, `build -f typst` | `pdf`        |
| `build -f docx`                  | `docx`       |
| `publish gdoc`                   | `docx`       |
| `publish confluence`             | `confluence` |
| `build -f markdown`              | `markdown`   |

`check -f <profile>` takes the same names.

### Variants

Variants come from the `variants` metadata array:

```yaml
variants: ["enterprise", "foo"]
```

They can also be supplied on the command line with repeated
`--meta variants <value>`. `cli.md` describes the metadata overrides.

## Behaviour that differs between formats

| Feature          | HTML                              | PDF                            |
| ---------------- | --------------------------------- | ------------------------------ |
| Inline HTML      | Passed through unchanged          | Dropped                        |
| Footnotes        | End of document, with back-links  | Bottom of the referencing page |
| Mermaid          | Rendered in the browser           | SVG from `mmdc` or built-in    |
| Link titles      | Tooltip                           | Not shown                      |
| Internal links   | HTML anchors                      | Typst labels                   |

PDF specifics:

- An internal link resolves to `#link(<label>)[...]`. A link to a label that
  does not exist renders as plain text.
- Table and figure captions become `#figure(caption: [...])` and appear below
  the block.
- `Listing:` captions become `#figure(kind: raw, caption: [...])`, appear above
  the block, and are numbered in their own sequence.
- Code blocks become `#code("...", lang: ..., start-line: ...)`.
- `.unlisted` and `.unnumbered` heading classes have no effect.

Confluence specifics:

- A native table-of-contents macro is inserted at the top of the page.
- Admonitions become Confluence macros: `note` to note, `tip` to tip,
  `warning`, `important`, and `caution` to warning, `related` to info. The
  default titles are not added.
- When the first visible node of the assembled document is a `warning`,
  `caution`, or `important` admonition, that block is placed above the table of
  contents. This is the way to put a critical notice at the very top:

```markdown
:::{.warning if-format="confluence"}
This page is generated automatically.
:::
```

Image size rules for each format are in `images.md`.
