# Illustrations

To insert an image, use this syntax:

~~~markdown
![Description](path/to/image "Tooltip"){#id .class attr=value}
~~~

If you do not set the identifier, the Builder makes it from the title. If the
title is not set, the Builder makes it from the description.

The HTML output keeps the classes without changes.

If an empty line comes before and after the image, the image is a separate
illustration with its own caption and identifier. Otherwise, the image is part
of the text.

The supported formats are PNG, JPEG, and SVG.

If the output format needs it, the build copies the image file to the target
directory. For the HTML format, the build copies all files to the `static`
subdirectory and changes the links in the HTML documents to match.

## Inline images {.inline-image}

If an image is in a paragraph with text (the image is not the only content of
the paragraph), it gets the CSS class `.inline-image`. This class sets the image
size to the height of the text line and sets the vertical alignment.

~~~markdown
An illustration ![Cat](../assets/cat.svg) directly in the text.
~~~

An illustration ![Cat](../../assets/cat.svg) directly in the text.

The Builder adds the `.inline-image` class. You do not set it manually.

## Image attributes

Write the attributes in curly brackets after the image link:

~~~markdown
![Description](path/to/image){#id width=50%}
~~~

### `width`

The `width` attribute sets the width of the image. The value is a percentage or
an absolute unit:

~~~markdown
![Illustration, 50%](../assets/illustration.png){width=50%}
~~~

![Illustration, 50%](../../assets/illustration.png){width="50%"}

~~~markdown
![Illustration, SVG](../assets/cat.svg){#cat width=25%}
~~~

![Illustration, SVG](../../assets/cat.svg){#cat width="25%"}

The code `[link to the illustration](#cat)` makes a working [link to the
illustration](#cat).

### `height`

The `height` attribute sets the height of the image:

~~~markdown
![Illustration](../assets/illustration.png){height=3cm}
~~~

### Units

Each format accepts its own units:

HTML

:   The value goes to the `<img>` element without changes. The theme CSS sets
    `max-width: 100%; height: auto`. Thus the width sets the image size, and the
    height keeps the aspect ratio.

PDF

:   Typst accepts `%`, `cm`, `mm`, `in`, `pt`, and `em`. Typst does not accept the
    `px` unit or a number without a unit.

DOCX

:   The accepted units are `%`, `cm`, `mm`, `in`, `pt`, `em`, `px`, and a number
    without a unit (pixels at 96 DPI). `em` is the font size of the body text. A
    width in `%` is a part of the available width. The section [Image size in
    DOCX](# "Image size in DOCX") describes the available width. A height in `%` is
    a part of the text height on the page. The value `auto` is the same as no
    attribute. If the value is not correct, the build stops with an error.

Only `%` works in all formats. For an image that must show in all formats, set
`width` in `%`.

### Image size in DOCX

If you set only `width` or only `height`, the image keeps its aspect ratio. If
you set the two attributes, the image gets the two values.

An image without attributes gets its size in pixels at 96 DPI. An inline image
without attributes gets a height equal to the font size of the `Normal` style
(`1em`).

The available width depends on the paragraph that contains the image:

- In a paragraph of a list item -- the text width on the page minus the indent of
  the list level.
- In a table cell -- the cell width minus the left and right cell margins. The
  width of a merged cell is the sum of the widths of its columns.
- In other paragraphs -- the text width on the page minus the left indent of the
  paragraph style. If the style does not set an indent, the indent of the base
  style (`based_on`) applies.

The paragraph style of the image:

- `ImageBlock` -- an image without text, and a diagram.
- `Normal` -- an inline image.
- `Quote` -- an image in a quote.
- `DefinitionBody` -- an image in a definition.
- The body style of the admonition -- an image in an admonition.
- `Heading1`--`Heading6`, `DefinitionTerm`, `Caption` -- an image in a heading, in
  a term, and in a caption.

The paragraph style does not depend on the block around the paragraph. For
example, an image in an admonition in a quote gets the width of the admonition
body style. It does not get the width of the `Quote` style.

A diagram and an image with a caption in a list item do not get the list indent.
For them, the available width is the text width on the page minus the indent of
the `ImageBlock` style.

If the image is wider than the available width or higher than the text area of
the page, the image becomes smaller. The width and the height decrease by the
same factor. If you set the two attributes, the image keeps the ratio of the two
values.

## Image captions

To add a caption, put a line with the prefix `Figure:` (or `figure:`) before the
image:

~~~markdown
Figure: Image caption {#label}

![Description](../assets/illustration.png){width=50%}
~~~

Figure: Image caption {#illustration-caption}

![Description](../../assets/illustration.png){width="50%"}

You can write an identifier in curly brackets after the caption text. Use this
identifier to link to the illustration.

## Image without attributes

The size of an image without attributes depends on the format:

HTML

:   The natural size of the image, but not more than the width of the text column.
    The theme CSS sets `max-width: 100%`.

PDF

:   The natural size of the image: pixels at the resolution from the file, or at 72
    DPI if the file does not set a resolution. If the image does not fit in the text
    area, the image becomes smaller and keeps its aspect ratio.

DOCX

:   Pixels at 96 DPI. If the image is wider than the available width or higher than
    the text area, the image becomes smaller and keeps its aspect ratio. The section
    [Image size in DOCX](# "Image size in DOCX") describes the available width.

~~~markdown
![Illustration](../assets/illustration.png)
~~~

![Illustration](../../assets/illustration.png)
