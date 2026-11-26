# Themes and settings

## Theme directory {#theme-dir}

A theme is a directory with files that have conventional names:

~~~
theme-dir/
  template.html              Template for single-page HTML
  template-multipage.html    Template for multi-page HTML
  template.typ               Template for PDF (Typst)
  style.css                  CSS (embedded into HTML as {{ style }})
  theme.puml                 PlantUML theme (inserted into each diagram)
  theme.mmd                  Mermaid theme (inserted into each diagram)
  theme.d2                   D2 theme (inserted into each diagram)
  docx.yaml                  DOCX style settings (page, fonts, colors, styles)
  assets/                    Static files that the templates refer to
~~~

All files are optional. To build a given format, the theme needs only the
related template. For example, to build HTML, the theme needs only
`template.html`.

If a custom theme has no `docx.yaml` file, the Builder uses the `docx.yaml` file
of the `blank` theme. The `init-theme` command writes this file into the
exported theme.

## Default theme

If you do not give the `-t`/`--theme` flag, the Builder uses the built-in theme
`blank`. This theme is compiled into the executable file. The default theme uses
the ParaType fonts:

- PT Sans -- the main font for HTML and diagrams
- PT Serif -- the main font for PDF
- PT Mono -- the monospace font

To make a custom theme, export the built-in theme with the `init-theme` command.
Then edit the exported files:

~~~
$ docbake init-theme my-theme
$ ls my-theme/
docx.yaml  style.css  template-multipage.html  template.html  template.typ
theme.d2  theme.mmd  theme.puml
~~~

Then use the theme in a build:

~~~
$ docbake -t my-theme build -f page -i doc/en/index.md -o target/out.html -b doc
~~~

The Builder resolves the `-t`/`--theme` flag in this order:

1. The name of a built-in theme (`blank`)
2. The path to a local theme directory

For example, the command below selects the built-in theme `blank` explicitly:

~~~
$ docbake -t blank build -f page -i doc/en/index.md -o target/out.html -b doc
~~~

## Base directory {#base-dir}

The `-b`/`--base` flag sets the root directory for the resolution of absolute
paths in `include` directives and links. The Builder resolves paths that start
with `/` relative to the base directory. All included files and resources must
be in the base directory.

If you do not give the flag, the base directory is the directory of the input
file.

~~~
$ docbake build -f page -i doc/en/index.md -o out.html -b doc
~~~

In this example, the base directory is `doc/`. The absolute path
`/assets/image.png` in the Markdown source resolves to `doc/assets/image.png`.

## Templates {#templates}

The [Tera](https://keats.github.io/tera/) template engine processes the
templates. The section [Output formats](formats.md) describes the variables that
are available to a template. Each template in the default theme has a comment
that lists all available variables.

### CSS

If the theme directory contains the `style.css` file, the Builder reads it and
gives it to the template in the `style` variable. A template usually writes its
content in a `<style>` tag:

~~~html
<style>{{ style | safe }}</style>
~~~

### Static assets

The Builder copies the `assets/` directory of the theme into the output
directory. Templates can refer to these files with a relative path, for example:

~~~html
<img src="{{ asset_prefix }}assets/logo.svg" alt="logo" />
~~~

The `asset_prefix` variable is empty for single-page HTML and PDF. For
multi-page HTML, it contains the relative path back to the output root (for
example, `../`).

### Logo {#logo}

The theme logo is the first file of this list in the `assets/` directory of the
theme:

1. `logo.svg`
2. `logo.png`
3. `logo.jpg`
4. `logo.jpeg`

The theme logo is optional. The built-in theme `blank` has no logo. The `logo`
front matter parameter sets a different logo for a document (see [Document
parameters](../document.md#document-parameters)). The Builder copies this file
to the `static` subdirectory of the output directory.

The templates get the path to the logo in the `logo` variable. The path is
relative to the output root. If the document has no logo, the `logo` variable
is not set. Templates show the logo only when the variable is set:

~~~html
{% if logo %}
<img src="{{ asset_prefix }}{{ logo }}" alt="logo" />
{% endif %}
~~~

DOCX output and Google Docs publication put the logo on the title page and in
the page header. The `logo` section of `docx.yaml` sets the logo heights.

## Diagram themes {#diagram-themes}

The Builder applies the files `theme.puml`, `theme.mmd`, and `theme.d2` to each
diagram of the related type. Thus all diagrams in the document get the same
fonts, colors, and styles.

### PlantUML

The Builder inserts the content of `theme.puml` into each PlantUML diagram
immediately after the `@start...` line:

~~~
skinparam defaultFontName "PT Sans"
skinparam defaultMonospacedFontName "PT Mono"
~~~

### Mermaid

The Builder inserts the content of `theme.mmd` before the source of each Mermaid
diagram. Usually this content is a `%%{init: ...}%%` directive:

~~~
%%{init: {"theme": "base", "themeVariables": {"fontFamily": "PT Sans"}}}%%
~~~

If a diagram already contains its own `%%{init:` directive, the Builder does not
apply the theme. This prevents conflicts.

### D2

The Builder inserts the content of `theme.d2` before the source of each D2
diagram. D2 has its own theme system. Refer to the [D2
documentation](https://d2lang.com/tour/themes).

## Artifact cache {#cache}

The Builder keeps a cache of rendered diagrams (PlantUML, Mermaid, D2, Draw.io).
The cache makes repeated builds faster. By default, the cache is in the standard
platform directory:

- Linux: `~/.cache/docbake`
- macOS: `~/Library/Caches/docbake`

To use a different cache directory, use the `--cache <dir>` flag. To disable the
cache, use the `--no-cache` flag.
