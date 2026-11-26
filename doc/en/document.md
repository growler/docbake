# Document source structure

A document has one or more files in [Markdown] format. Files can include [other
files](#include). The root file of the document is the _main_ file.

## Document parameters

Each file can start with a front matter block in [YAML] format. The front matter
sets the document parameters. The block starts and ends with a line of three
hyphens. All parameters are optional.

`lang`

:  The document language in [RFC 5646](https://tools.ietf.org/html/rfc5646) format,
   for example `en-US` or `ru-RU`.

`title`

:  The document title. The PDF title page and the PDF document properties use it.

`subtitle`

:  The document subtitle. The PDF title page and the PDF document properties use
   it.

`author`

:  The document author. The PDF title page and the PDF document properties use it.

`version`

:  The document version. The PDF title page uses it.

`keywords`

:  The document keywords. The PDF document properties and the HTML document
   metadata use them. You can set them as an array or as a string.

`date`

:  The document modification date. The PDF document properties and the HTML
   document metadata use it. The format is `YYYY-MM-DD` or `YYYY-MM-DD HH:MM:SS`.

`variants`

:  An array of document variant strings. The `if-variant` attribute uses them to
   show or hide fenced divs.

   Example:

   ~~~yaml
   variants: ["enterprise", "internal"]
   ~~~

   You can also add a variant with the `--meta variants <value>` flag. You can use
   this flag more than one time.

`confluence`

:  A dictionary of parameters for publication to Confluence. It contains these
   keys:

   `server`

   :  The URL of the Confluence server (for example,
      `https://confluence.example.com`).

   `space`

   :  The key of the Confluence space (for example, `PROJ`).

   `path`

   :  The path to the page. It can be a string or an array of strings.

      If `path` is a string, it is the title of a page at the root of the space.

      If `path` is an array, the last item is the title of the page. The other items
      are the chain of ancestor pages. Each item can be a page title or a literal ID
      in the format `id:12345678`.

All parameters are optional.

You can override each front matter parameter at build time with the `--meta`,
`--meta-drop`, and `--meta-file` flags (refer to [Metadata
override](builder/index.md#metadata-overrides)).

Example:

~~~markdown
‑‑‑
title:    Documentation Build System
author:   Jane Doe
lang:     en-US
version:  0.1.0
keywords: documentation, markdown, tools
variants: ["enterprise"]
confluence:
  server: https://confluence.example.com
  space:  PROJ
  path:
    - "Parent page"
    - "Document title"
‑‑‑
~~~

The builder reads the document parameters from the YAML front matter of the main
file.

## File inclusion {#include}

A file can include other files with the special `include` block. The block
contains links to other files in the project. The link text is only for
reference. It does not change the output document.

The links in the `include` block must have the form of [Markdown] headings. The
heading level sets the shift of the heading levels in the included file. For
example, if you include a file with a level 2 heading, all headings in that file
move down by one level.

For example, a project has this structure:

`project`

:  ~~~plantuml
   @startsalt
   {
       {T
        + project/
        ++ hld/
        +++ arch.md
        +++ index.md
        ++ index.md
       }
   }
   @endsalt
   ~~~

`project/index.md`

:  ~~~markdown
   # System purpose

    :::include
   # [HLD](hld/index.md)
   :::
   ~~~

`project/hld/index.md`

:  ~~~ markdown
   # High-level design

    :::include
   ## [Architecture](arch.md)
   :::
   ~~~

`project/hld/arch.md`

:  ~~~markdown
   # Architecture

    ## Basic principles
   ~~~

The result has this structure:

~~~markdown
# System purpose

# High-level design

## Architecture

### Basic principles
~~~

<!-- Links -->

[YAML]: https://yaml.org/

[Markdown]: markdown.md

[Builder]: builder/index.md
