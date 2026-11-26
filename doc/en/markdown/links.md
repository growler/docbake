# Links and Footnotes

## Links

A link in a document can be absolute or relative. An absolute link starts with
`/`. The [Builder] resolves absolute links from the project root[^vscode]. It
resolves relative links from the file that contains the link. The [Builder]
checks all links in the document. If a link points to a label that does not
exist, the build stops with an error. The Builder records all labels of the
document: automatic labels of headings, captions, and code blocks, and manual
labels (`{#id}`) on images, diagrams, inline elements (`[...]{#id}`), and blocks
(`:::{#id}`).

The same rules apply to links to image files.

The [Builder] supports absolute links. Use relative links if you want to move a
part of a document or include it in a different document. For example, you have
the documents HLD and LLD, and each document has its own main file. If the links
in these documents are relative, you can make a new document "Documentation set"
that includes the two documents. All links continue to work.

[Builder]: ../builder/index.md

[^vscode]: Visual Studio Code also resolves absolute links in Markdown files from the
    project root. Thus you can use VS Code to edit the documents.

The format gives several ways to define labels and links.

### Labels

A label identifies a section or an element of the document. You use labels as
link targets. A label is automatic or manual.

The Builder makes an automatic label from the text of the element (a heading, an
image, a table, a diagram, or a code block). The Builder changes the text to
lowercase and removes all characters except letters, digits, and underscores. It
replaces spaces and punctuation with a hyphen. For example, the heading
`Document parameters` gets the label `document-parameters`. For a code block,
the Builder makes the automatic label from the description text in the info
string (see [Extended info string](text.md#extended-info-string)).

To set a manual label, use the extended Markdown syntax for element attributes
or for element captions:

~~~markdown
# Heading {#my-label}

![Image](path/to/image.png){#my-label}

Table: Table caption {#my-label}

| Table ...

Figure: Diagram caption {#my-label}

```plantuml
diagram...
```

Listing: Listing caption {#my-label}

```rust
code with a caption
```

```rust Code block description
the code gets its label from the description
```

```rust {#my-label}
code with an explicit label
```

:::{#my-label}
A section that needs a label
:::

Some [text that needs a label]{#my-label}
~~~

### Automatic links

A URL or an email address in angle brackets becomes a link:

~~~markdown
<https://example.com>
<someone@somewhere.net>
~~~

:::related
# Result

<https://example.com>\
<someone@somewhere.net>
:::

### Inline links

An inline link has the link text in square brackets. The URL in parentheses
follows the text immediately:

~~~markdown
[Link text](https://example.com)

[Section](#section)

[Section in a document](document.md#section)
~~~

Result (with real links from this document):

[External link](https://example.com)

[Section Headings](text.md#headings)

[Document parameters](../document.md#document-parameters)

A link can also have a title:

~~~markdown
[Link text](https://example.com "Title")
~~~

:::related
# Result

[Link text](https://example.com "Title") -- in HTML, a tooltip shows when you
move the pointer over the link. The title becomes the `title` attribute of the
HTML element `a`. The PDF output does not use the title.
:::

### Reference links

A reference link has two parts: the link definition and the link use. The
definition can be at any position in the document, before or after the use. Put
all link definitions at the end of a document part. This makes the source easy
to read.

A link definition has two parts: the link identifier, and the URL with an
optional title. Put the link identifier in square brackets. Then write a colon
and the URL. You can put the URL in angle brackets. You can put the title in
double quotes or in parentheses.

~~~markdown
... as described in [RFC 7763]

[RFC 7763]: https://www.rfc-editor.org/rfc/rfc7763.html "The text/markdown Media Type"
~~~

:::related
# Result

... as described in [RFC 7763]

[RFC 7763]: https://www.rfc-editor.org/rfc/rfc7763.html "The text/markdown Media Type"
:::

### Links in the document

For links in the document, you can use this syntax:

~~~markdown
# Heading

[Link to the heading](#heading)

# Heading with an identifier {#header-id}

[Link to the heading with an identifier](#header-id)

[Link to an object in a different document](document.md#header-id)
~~~

Result (with real headings from this document):

[Link to the section Headings](text.md#headings)

[Link to a section with an explicit identifier](text.md#inline-markup)

[Link to a section in a different document](../document.md#document-parameters).

A special syntax lets you link to a heading without its automatic identifier.
Write `#` as the link target. Write the link title with the same text as the
heading, but without formatting:

~~~markdown
# Complex _heading_ with `code`

[Link to the heading](# "Complex heading with code")

[Link to a heading in a different document](part/document.md "Heading in a different document")
~~~

Result (with a real heading from a different document): [Link to the
parameters](../document.md "Document parameters").

You can also use only the file name of the document without an identifier. Then
the link points to the first heading in the document:

~~~markdown
[Section](part/document.md)
~~~

:::related
# Result

[Document structure](../document.md).
:::

For links to headings in the document, you can also use the short syntax:

~~~markdown
# Heading

... see [Heading]
~~~

Result (the link `[Builder]` with the definition `[Builder]:
../builder/index.md` in this document): see [Builder].

## Footnotes {#footnotes}

A footnote adds a note and does not break the main text. The syntax has two
parts: the footnote reference in the text and the footnote definition.

Write the footnote reference in the text as `[^label]`. The definition can be at
any position in the document. It starts with `[^label]:`, and the footnote text
follows.

For example, the text `statement[^note]` makes a reference to the footnote[^fn1]
`note`. Define this footnote as follows:

    [^note]: Footnote text.

To continue the definition, indent the lines. A footnote can have more than one
paragraph[^fn2]:

    [^note]: First paragraph.

        Second paragraph.

To align the text, you can start the first paragraph on a new line:

    [^note]:
        First paragraph.

        Second paragraph.

The text shows a superscript number at the position of the footnote reference.
In HTML output, all footnotes go to the end of the document with back links. In
PDF output, footnotes go to the bottom of the related page. In DOCX output,
footnotes also go to the bottom of the page. Each reference makes a new
footnote, also when the label repeats.

[^fn1]: This is a footnote.

[^fn2]:
    This is a footnote with more than one paragraph.

    Indent the paragraphs with four spaces.

A footnote definition can be in the same file as its references. When the
Builder assembles a document from many files, it numbers all footnotes in
sequence through the full document.
