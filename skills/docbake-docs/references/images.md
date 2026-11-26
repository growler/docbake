# Image size in each output format

## Default size

An image without `width` and `height` has its own size, limited by the space
for text:

- HTML shows the image at its own size, at most the column width. The CSS of
  the theme sets `max-width: 100%`.
- PDF uses the pixels at the resolution of the file, or at 72 DPI when the file
  sets no resolution. Typst makes the image smaller to fit the text area and
  keeps its aspect ratio.
- DOCX uses the pixels at 96 DPI. DOCX makes the image smaller to fit the text
  area and keeps its aspect ratio.

## Units

`width` and `height` set the image size. Each output format accepts different
units:

- HTML writes the value to the `<img>` element. The CSS of the theme sets
  `max-width: 100%; height: auto`, thus the width sets the size and the height
  keeps the aspect ratio.
- PDF accepts `%`, `cm`, `mm`, `in`, `pt` and `em`. Typst rejects `px` and a
  number without a unit.
- DOCX accepts `%`, `cm`, `mm`, `in`, `pt`, `em`, `px` and a number without a
  unit (pixels at 96 DPI). `em` is the font size of the body text. A width in
  `%` is a part of the text width. A height in `%` is a part of the text
  height. `auto` is the same as no value. An invalid value stops the build.

Only `%` works in all formats. Use `width` in `%` for an image that all
formats show.

## DOCX details

In DOCX, an image with only `width` or only `height` keeps its aspect ratio. An
image with both values uses the two values. An image without the two values has
its pixel size at 96 DPI. An image in a line of text without the two values has
the height of the font size of the `Normal` style (`1em`). If an image is wider
or taller than the text area of the page, DOCX makes the image smaller to fit
the text area. The width and the height change by the same factor. With both
values, the image keeps the ratio of the two values.
