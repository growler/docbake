# Text Markup

## Inline Text Markup

You can use these markup elements in text:

table: Inline markup syntax {#inline-markup}

**Syntax**          | **Result**
------------------- | -----------------
`*italic*`          | *italic*
`_italic_`          | _italic_
`**bold**`          | **bold**
`__bold__`          | __bold__
`~~strikethrough~~` | ~~strikethrough~~
`` `unformatted` `` | `unformatted`
`\*escaped\*`       | \*escaped\*

## Backslash Escapes

You can escape any ASCII punctuation character with a backslash. The output then
shows the character as it is, and the parser does not read it as markup:

~~~markdown
\*not italic\*  
\# not a heading  
\[not a link\]  
~~~

:::related
# Result

\*not italic\*\
\# not a heading\
\[not a link\]
:::

Escapes work for these characters:

`` \ ` * _ { } [ ] ( ) # + - . ! | ~ ``

Escapes do not work in code spans and fenced code blocks. In code, the output
shows all characters as they are.

## Paragraphs and Line Breaks

An empty line separates paragraphs. The parser reads a usual line break in a
paragraph as a space (soft line break).

To make a hard line break in a paragraph, put two or more spaces at the end of
the line, or put a backslash `\` before the line break:

~~~markdown
First line ␣ ␣  
Second line (break with two spaces)

First line\
Second line (break with a backslash)
~~~

:::related
# Result

First line  
Second line (break with two spaces)

First line\
Second line (break with a backslash)
:::

## Headings

A heading starts with one or more `#` characters. The number of characters sets
the heading level. The maximum level is 6.

You can use [inline markup](#inline-markup) in headings, but we do not recommend
it.

Put an empty line before and after a heading.

The builder makes the heading identifier from the heading text. You can also set
the identifier explicitly in this format:

~~~markdown
# Heading {#header-id}
~~~

The document table of contents includes all headings. To remove a heading from
the table of contents, give it the `{.unlisted}` attribute:

~~~markdown
# Heading {.unlisted}
~~~

To remove a heading from the numbering, give it the `{.unnumbered}` attribute:

~~~markdown
# Heading {.unnumbered}
~~~

## Code Blocks {#code-blocks}

### Fenced Code Blocks

Lines of three or more backticks (```` ``` ````) or tildes (`~~~`) start and end
a code block. After the opening fence, you can write the language for syntax
highlighting:

~~~markdown
```rust
fn main() {
    println!("Hello, world!");
}
```
~~~

:::related
# Result

```rust
fn main() {
    println!("Hello, world!");
}
```
:::

If the code block contains triple backticks, the fences must have more
characters than the content:

`````markdown
````markdown
```rust
code...
```
````
`````

### Extended Info String {#extended-info-string}

After the language in the info string, you can add descriptive text. The builder
makes the code block identifier from this text. It uses the same rules as for
[heading identifiers](#headings): it changes the text to lowercase, replaces
spaces with hyphens, and removes special characters.

~~~markdown Code block link example
```rust Basic function
fn basic() {}
```
~~~

This [code block](#code-block-link-example) gets the identifier
`basic-function`. You can use this identifier in [links](links.md#labels):

~~~markdown
See the [basic function example](#basic-function).
~~~

If two or more code blocks have the same descriptive text, the builder adds a
suffix (`-1`, `-2`, and so on) to make each identifier unique. It does the same
for headings. Code blocks and headings use one identifier namespace. Thus a code
block cannot get the same identifier as a heading.

### Code Block Attributes {#code-block-attributes}

Code blocks accept attributes in the format `{#id .class key=val}`, as
[headings](#headings) and other elements do. Write the attributes after the
language and after the descriptive text, if there is one:

~~~markdown
```rust {#my-example data-line="5"}
fn main() {}
```
~~~

In HTML output, the attributes go on the `<pre>` element:

```html
<pre id="my-example" data-line="5" style="...">
  <code class="language-rust">...</code>
</pre>
```

An explicit identifier `{#id}` has priority over an automatic identifier. You
can use descriptive text and attributes together:

~~~markdown
```python HTTP client {.important data-version="2"}
import requests
```
~~~

Here the builder makes the identifier `http-client` from the description. It
adds the `.important` and `data-version` attributes to the element.

If a code block has both descriptive text and an explicit identifier, the
builder uses the explicit identifier:

~~~markdown
```rust Basic function {#basic-fn .example}
fn basic() {}
```
~~~

The identifier is `basic-fn`. The builder does not make an identifier from the
description.

For diagram languages (`mermaid`, `plantuml`, `typst`, `d2`), the special class
`.as-code` stops the conversion of the code block to a diagram. For more
information, refer to [Show Diagram Code](diagrams.md#show-diagram-code).

The `start` attribute adds line numbers to the code:

~~~~markdown
~~~rust {start=1}
fn main() {
    println!("Hello, world!");
}
~~~
~~~~

:::related
## Result

~~~rust {start=1}
fn main() {
    println!("Hello, world!");
}
~~~
:::

### Code Block Caption {#code-block-caption}

A code block can have a caption, as [tables](tables-lists.md#table-caption) and
[diagrams](diagrams.md) can. To add a caption, put a separate paragraph with the
prefix `Listing:` (or `listing:`) before the code block.

~~~markdown
Listing: Example of the main function {#listing-main}

```rust
fn main() {
    println!("Hello, world!");
}
```
~~~

:::related
# Result

Listing: Example of the main function {#listing-main}

```rust
fn main() {
    println!("Hello, world!");
}
```
:::

The caption is above the code block in HTML output and in PDF output.

In PDF (Typst), listings have a numbering sequence that is separate from tables
and figures. The prefix is "Listing" for English documents and «Листинг» for
Russian documents.

To set the identifier for links, write it in curly brackets after the caption
text: `{#id}`. If there is no identifier, the builder makes it from the caption
text. It uses the same rules as for [heading identifiers](#headings). A link to
the listing above: `[listing](#listing-main)` -- [listing](#listing-main).

### Code Spans

Use backticks for inline code. If the code contains a backtick, use double
backticks:

~~~markdown
Call the `main()` function.
The literal `` `backtick` `` in code.
~~~

:::related
# Result

Call the `main()` function.\
The literal `` `backtick` `` in code.
:::

### Indented Code Blocks

Lines that start with four or more spaces also make a code block. Use fenced
code blocks instead of indented code blocks. Fenced code blocks support syntax
highlighting and show the code limits more clearly.

~~~markdown
Example output

␣ ␣ ␣ ␣ $ echo ok
␣ ␣ ␣ ␣ ok

Text continues
~~~

:::related
# Result

Example output

    $ echo ok
    ok

Text continues
:::

## Block Quotes {#block-quotes}

A block quote starts with the `>` character. Block quotes can contain all block
elements, and you can put a block quote in a block quote:

~~~markdown
> This is a quote.
>
> > A nested quote.
>
> The quote continues.
~~~

:::related
# Result

> This is a quote.
>
> > A nested quote.
>
> The quote continues.
:::

In block quotes, you can use lists, code blocks, and other markup elements:

~~~markdown
> An example with code:
>
> ```
> code in a quote
> ```
>
> - list item
~~~

:::related
# Result

> An example with code:
>
> ```
> code in a quote
> ```
>
> - list item
:::

## Thematic Break

A line of three or more `-`, `*`, or `_` characters makes a thematic break:

~~~markdown
Text before the break.

---

Text after the break.
~~~

:::related
# Result

Text before the break.

---

Text after the break.
:::

## Inline HTML

You can use HTML markup in text. HTML output contains this markup without
changes:

~~~markdown
Text with an <abbr title="Hyper Text Markup Language">HTML</abbr> tag.

<div style="color: red">
An HTML block.
</div>
~~~

::: important
## Attention

HTML output contains inline and block HTML without changes. PDF output ignores
it. Do not use HTML markup in a document that you build to PDF.
:::
