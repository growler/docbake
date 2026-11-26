# Output formats

The builder supports these formats:

`page`

:  A linear HTML page with all content.

`site`

:  A set of HTML pages that shows the structure of the source document. Each `.md`
   document becomes a separate page.

`pdf`

:  A PDF document. The [`typst`](https://typst.app/) typesetting system makes this
   document.

`typst`

:  An intermediate Typst file (`.typ`). The builder does not run `typst compile`
   for this format.

`docx`

:  A DOCX document. The same renderer makes the DOCX file for `publish gdoc`. The
   name `docx-gdoc` is an alias of `docx`.

`markdown`

:  A linear Markdown document with all content, after the builder processes all
   `include` blocks. AI agents can use this format.

:::note
After `publish gdoc`, the Google Docs document contains a Google Docs table of
contents. The page numbers in this table of contents are incorrect until you
update it. To update the table of contents, click it in Google Docs. Then click
the "Update table of contents" button.
:::

## Format-specific blocks

Some parts of the text must appear only in specified output profiles. For this
purpose, use the conditional attributes of a fenced div:

- `if-format="..."` -- a condition on the format.
- `if-variant="..."` -- a condition on the document variant.

~~~markdown
:::{if-format="html confluence"}
This block is visible only in HTML and Confluence.
:::

:::{if-variant="enterprise"}
This block is visible only when the metadata contains variants: ["enterprise"].
:::

:::{if-format="html" if-variant="enterprise"}
This block is visible only when both conditions are true.
:::
~~~

### Condition rules

- Spaces separate the values in `if-format`. The values use the OR rule.
- Spaces separate the values in `if-variant`. The values use the OR rule.
- Different attributes (`if-format` and `if-variant`) use the AND rule.
- A block without conditional attributes is visible.
- An empty `if-format` or `if-variant` value causes a build error.
- An unknown format in `if-format` causes a build error.

A hidden conditional block does not exist in the selected profile. The builder
does not use it for rendering, the TOC, the search index, footnote placement,
diagram checks, or asset copies.

Conditional attributes are control attributes. They do not appear in the output
(HTML, Markdown, DOCX, or Confluence).

### Format names in `if-format`

Conditions use only these names:

- `html`
- `pdf`
- `docx`
- `confluence`
- `markdown`

These names match the commands as follows:

- `build -f page` and `build -f site` → `html`
- `build -f pdf` and `build -f typst` → `pdf`
- `build -f docx` → `docx`
- `publish confluence` → `confluence`
- `publish gdoc` → `docx`
- `build -f markdown` → `markdown`
- `check -f ...` uses the same profile names

### Variants (`if-variant`)

The document metadata (`variants`) gives the variants.

~~~yaml
variants: ["enterprise", "foo"]
~~~

You can also set variants on the command line:

~~~bash
docbake build -f page -i doc.md -o out.html \
  --meta variants enterprise \
  --meta variants foo
~~~

The builder collects repeated `--meta variants ...` values into an array. The
values in `variants` must be strings.

> Note: Only the `if-format` and `if-variant` attributes control conditional
> visibility. Classes such as `:::pdf` or `:::{.html .pdf}` are regular CSS
> classes of a fenced div.

## HTML {#html}

The builder makes a single-page HTML document. This file contains all content,
including the styles. The builder copies image files and other resources to the
`static` subdirectory of the output directory. It also changes the links to
these files in the HTML.

### HTML template

The [Tera](https://keats.github.io/tera/) template engine makes the HTML page.
The template is the `template.html` file in the [theme
directory](config.md#theme-dir).

The template gets these variables:

`title`
:  The document title from the metadata.

`subtitle`
:  The document subtitle (if set).

`lang`
:  The document language (for example, `en-US`). The default is `en`.

`date`
:  The document date (if set).

`authors`
:  An array of document authors (if set).

`abstract`
:  The document abstract (if set), in HTML format.

`abstract_title`
:  The title of the abstract. The default is `Abstract`.

`style`
:  The content of the `style.css` file from the theme directory (if the file
   exists). The template outputs it with the `| safe` filter because it contains
   CSS code.

`body`
:  The rendered HTML code of the document body. The template outputs it with the `|
   safe` filter.

`toc`
:  The table of contents as nested `<ul>/<li>` elements. The template outputs it
   with the `| safe` filter.

`front_matter`
:  An object with all metadata from the YAML front matter of the document. If the
   build uses `--meta` flags, this object contains their values.

`asset_prefix`
:  The relative path to the output root. For single-page HTML, it is an empty
   string. For multi-page HTML, the builder calculates it for each page (for
   example, `../`). Use it for links to files in the `assets/` directory of the
   theme.

### HTML template example

~~~html
<!DOCTYPE html>
<html lang="{{ lang }}">
<head>
  <meta charset="utf-8" />
  <title>{{ title }}</title>
  <style>{{ style | safe }}</style>
</head>
<body>
  {% if title %}
  <header>
    <h1 class="title">{{ title }}</h1>
    {% if subtitle %}<p class="subtitle">{{ subtitle }}</p>{% endif %}
  </header>
  {% endif %}

  <nav id="toc" role="doc-toc">
    {{ toc | safe }}
  </nav>

  <main>
    {{ body | safe }}
  </main>
</body>
</html>
~~~

### Table of contents

The builder makes the table of contents from all document headings, including
the headings from included files. Headings with the `.unlisted` class are not in
the table of contents. Headings with the `.unnumbered` class are in the table of
contents, but they have no numbers.

### Syntax highlighting

In HTML output, the builder highlights code blocks that specify a language. The
highlight styles are in the HTML file.

### Footnotes

The builder collects footnotes at the end of the document in a `<div
class="footnotes">` block. Each footnote has a back link to the place where the
text uses it.

## PDF {#pdf}

The builder makes a PDF through an intermediate Typst representation. First, the
builder translates the document content to Typst markup. Then it makes the final
`.typ` file from the template. Then it runs the external command `typst compile`
to make the PDF.

PDF output needs `typst` installed and available in `PATH` (see
[Installation](index.md#installation)).

### PDF template

The [Tera](https://keats.github.io/tera/) template engine also processes the PDF
template. The result is Typst markup, not HTML.

The template gets the same metadata variables as HTML (`title`, `subtitle`,
`lang`, `date`, `author`, `version`, and more). It also gets these variables:

`body`
:  The rendered Typst code of the document body. The template outputs it with the
   `| safe` filter.

`lang`
:  The language code without the region, for example `en`. The default is `en`.

`region`
:  The region code (if set in the metadata), for example `US`.

`asset_prefix`
:  The relative path to the output root. For PDF, it is always an empty string.

The `template.typ` template sets the fonts, colors, resource paths (for example,
the logo), and other design parameters. To make your own design, export the
built-in theme with the `docbake init-theme` command. Then edit the file.

### PDF template example

~~~typst {.as-code}
#set page(paper: "a4", margin: 2cm)
#set text(lang: "{{ lang | default(value="en") }}", size: 11pt)

{% if title %}
#align(center, text(size: 22pt, weight: "bold", "{{ title }}"))
{% endif %}
{% if author %}
#align(center, text(size: 12pt, "{{ author }}"))
{% endif %}

{{ body | safe }}
~~~

### PDF output details

- Footnotes appear at the bottom of the applicable page. This is the standard
  behavior of Typst `#footnote[...]`.
- The builder ignores inline HTML. Use only Markdown markup.
- The builder translates links to labels in the document to Typst links to labels
  (`#link(<label>)[...]`). If the label does not exist, the link is plain text.
- Table and diagram captions become `#figure(caption: [...])`.
- Code block captions (`Listing:`) become `#figure(kind: raw, caption: [...])`.
  Their numbers are separate from the numbers of tables and figures. Their prefix
  is "Listing" (or the equivalent word for the document language).
- Code blocks become `#raw()` calls with the language.

## Confluence {#confluence}

The `publish confluence` command uses the `confluence` profile.

In Confluence output, the builder adds the native table of contents (TOC) macro
at the start of the page.

The first visible node of the final document can be a fenced div admonition with
the `warning`, `caution`, or `important` class. The builder determines this node
after it expands `include` and applies `if-format` and `if-variant`. In this
case, the builder puts this block before the TOC.

Thus you can put a critical warning at the top of the page:

~~~markdown
:::{.warning if-format="confluence"}
This page is generated automatically.
:::
~~~

## Markdown {#format-markdown}

The Markdown format makes one Markdown file. The builder expands all
`:::include` directives, rewrites links, collects footnotes, and normalizes
heading levels. Use this format to debug a document, or to export a document to
systems that use plain Markdown.

~~~
$ docbake build -f markdown -i doc/en/index.md -o target/doc-out/ -b doc
~~~
