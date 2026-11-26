# Builder

The Builder is the executable file `docbake` or `docbake.exe`. It validates the
Markdown documentation of a project and generates the build results. The Builder
supports these output formats:

- page -- a single-page HTML document with embedded styles and a table of contents
- site -- multi-page HTML with navigation between pages
- PDF -- a document made with Typst
- Typst -- the intermediate `.typ` file
- DOCX (docx) -- DOCX in the same format that `publish gdoc` uses
- Markdown -- one Markdown file. The Builder expands all `include` blocks,
  rewrites the links, and collects the footnotes into this file.

For diagrams, the Builder also uses external tools. These tools make SVG from
PlantUML, D2, and Draw.io sources.

~~~
$ docbake -h
Produce single-page HTML, multi-page HTML, PDF, or DOCX from markdown

Usage: docbake [OPTIONS] <COMMAND>

Commands:
  build       Build a document in the specified format
  dump        Dump the document include-graph for a markdown file
  meta        Read a metadata value from the document's YAML front matter
  parse       Dump the parsed AST for a markdown file
  serve       Serve a document with live-reload on file changes
  watch       Watch source files and rebuild on changes
  check       Validate the document: check all links and render all diagrams
  fmt         Format markdown files in place
  init-theme  Export the selected theme to a directory for customisation
  publish     Publish a document to an external platform
  auth        Manage authentication credentials for external platforms
  help        Print this message or the help of the given subcommand(s)

Options:
      --debug-dump
  -t, --theme <THEME>  Theme name or directory
      --cache <CACHE>  Override the default cache directory for rendered
                       artifacts
      --no-cache       Disable artifact caching entirely
  -h, --help           Print help (see more with '--help')
  -V, --version        Print version
~~~

~~~
$ docbake build -h
Build a document in the specified format

Usage: docbake build [OPTIONS] --format <FORMAT> --input <INPUT>
                         --output <OUTPUT>

Options:
      --debug-dump
  -f, --format <FORMAT>              Output format [possible values: page,
                                     site, pdf, typst, markdown, docx]
  -i, --input <INPUT>                Root markdown file
  -t, --theme <THEME>                Theme name or directory
      --cache <CACHE>                Override the default cache directory
                                     for rendered artifacts
  -o, --output <OUTPUT>              Output file or directory
  -b, --base <BASE>                  Base directory for absolute path
                                     resolution. Defaults to the parent
                                     directory of the input file
      --no-cache                     Disable artifact caching entirely
      --broken-links <BROKEN_LINKS>  How to handle broken links: "warn"
                                     prints diagnostics and continues,
                                     "error" aborts the build.  Defaults to
                                     "error" [default: error] [possible
                                     values: warn, error]
      --meta <KEY> <VALUE>           Override a front-matter value.
                                     Repeatable. Key may use dot-notation
                                     (e.g. "confluence.server")
      --meta-drop <KEY>              Remove a front-matter key before
                                     applying --meta overrides. Repeatable.
                                     Key may use dot-notation (e.g.
                                     "confluence.path")
      --meta-file <PATH>             Merge top-level metadata keys from a
                                     YAML file. Keys with the same name
                                     replace front-matter keys
  -h, --help                         Print help (see more with '--help')
~~~

~~~
$ docbake serve -h
Serve a document with live-reload on file changes

Usage: docbake serve [OPTIONS] --format <FORMAT> --input <INPUT>

Options:
      --debug-dump
  -f, --format <FORMAT>              Output format (only page and site are
                                     supported) [possible values: page,
                                     site, pdf, typst, markdown, docx]
  -i, --input <INPUT>                Root markdown file
  -t, --theme <THEME>                Theme name or directory
  -b, --base <BASE>                  Base directory for absolute path
                                     resolution. Defaults to the parent
                                     directory of the input file
      --cache <CACHE>                Override the default cache directory
                                     for rendered artifacts
      --no-cache                     Disable artifact caching entirely
  -p, --port <PORT>                  Port to listen on.  If not specified,
                                     an available port is chosen
                                     automatically
      --broken-links <BROKEN_LINKS>  How to handle broken links: "warn"
                                     prints diagnostics and continues,
                                     "error" aborts the build.  Defaults to
                                     "warn" [default: warn] [possible
                                     values: warn, error]
      --meta <KEY> <VALUE>           Override a front-matter value.
                                     Repeatable. Key may use dot-notation
                                     (e.g. "confluence.server")
      --meta-drop <KEY>              Remove a front-matter key before
                                     applying --meta overrides. Repeatable.
                                     Key may use dot-notation (e.g.
                                     "confluence.path")
      --meta-file <PATH>             Merge top-level metadata keys from a
                                     YAML file. Keys with the same name
                                     replace front-matter keys
  -h, --help                         Print help (see more with '--help')
~~~

## Usage

To build the document in PDF format, run this command:

~~~
$ docbake build -f pdf -i doc/en/index.md -o target/doc-out/builder.pdf -b doc
~~~

To build the document in HTML format:

~~~
$ docbake build -f page -i doc/en/index.md -o target/doc-out/builder.html -b doc
~~~

To build multi-page HTML:

~~~
$ docbake build -f site -i doc/en/index.md -o target/doc-out/builder -b doc
~~~

To build the document in DOCX format:

~~~
$ docbake build -f docx -i doc/en/index.md -o target/doc-out/builder.docx -b doc
~~~

To build one combined Markdown file:

~~~
$ docbake build -f markdown -i doc/en/index.md -o target/doc-out/builder.md -b doc
~~~

The `-o` flag sets the output file or directory. If `-o` points to a file with
the extension `.html`, `.pdf`, `.md`, or `.docx`, the Builder writes the result
to this file. If not, `-o` is a directory, and the Builder takes the file name
from the name of the input file. For the `site` format, `-o` is always a
directory.

The `-b` flag sets the base directory for the resolution of absolute paths. The
default is the directory of the input file. The `-t` flag sets the directory of
a custom theme.

### Metadata overrides {#metadata-overrides .unnumbered}

The `--meta KEY VALUE` flag overrides a value from the YAML front matter of the
document for one build. You can give the flag more than one time. The key
accepts dot notation to refer to nested objects.

~~~
$ docbake build -f page -i doc.md -o out.html \
    --meta title "New title" \
    --meta confluence.server "https://other.example.com"
~~~

If an intermediate object does not exist, the Builder creates it. The override
applies only to the current build. It does not change the source file.

For the `variants` key, repeated `--meta` flags make an array of values:

~~~
$ docbake build -f page -i doc.md -o out.html \
    --meta variants enterprise \
    --meta variants foo
~~~

The `if-variant` condition in a fenced div uses this array.

The `--meta-file path.yml` flag loads a YAML file and merges it with the front
matter of the document by top-level keys. The merge is not deep. If the file
contains the `confluence` key, this key replaces the full `confluence` object
from the front matter.

The Builder applies metadata in this order: document front matter ->
`--meta-file` -> `--meta-drop` -> `--meta`.

The `--meta-drop KEY` flag removes the key from the metadata before the Builder
applies `--meta`. Use it to remove arrays and nested objects that `--meta`
cannot override. The key also accepts dot notation.

For example, when you change the Confluence space, remove the `path` array. If
you do not remove it, it points to pages that do not exist in the new space:

~~~
$ docbake publish confluence -i doc.md \
    --meta-drop confluence.path \
    --meta confluence.server "https://other.example.com" \
    --meta confluence.space "NEWSPACE"
~~~

You can also remove the full nested object and set it again:

~~~
$ docbake publish confluence -i doc.md \
    --meta-drop confluence \
    --meta confluence.server "https://other.example.com" \
    --meta confluence.space "NEWSPACE"
~~~

The flags `--meta`, `--meta-drop`, and `--meta-file` are available in the
commands `build`, `serve`, `watch`, `check`, `publish confluence`, and `publish
gdoc`.

## Preview {.unnumbered}

The `serve` command starts a local HTTP server and opens the document in the
browser. When a file in the base directory changes, the Builder rebuilds the
document and the browser reloads the page.

The command supports only the HTML formats: `page` and `site`.

For single-page HTML:

~~~
$ docbake serve -f page -i doc/en/index.md -b doc
~~~

For multi-page HTML:

~~~
$ docbake serve -f site -i doc/en/index.md -b doc
~~~

The server selects a free port, prints the URL, and opens it in the browser. To
set a specific port, use the `-p` flag:

~~~
$ docbake serve -f page -i doc/en/index.md -b doc -p 3000
~~~

The server listens on `127.0.0.1` (loopback). Only the local host can connect to
it. To stop the server, press `Ctrl+C`.

## Watch mode {.unnumbered}

The `watch` command builds the document and monitors the files in the base
directory. When a file changes, the Builder rebuilds the document. The `watch`
command does not start an HTTP server. It writes the result to a file or a
directory.

~~~
$ docbake watch -f page -i doc/en/index.md -o target/doc-out/builder.html -b doc
~~~

To stop the command, press `Ctrl+C`.

## Document check {.unnumbered}

The `check` command loads the document, checks the links, and renders the
diagrams for the selected output profile. The command does not make output
files. Its only purpose is validation.

The `-f/--format` flag is mandatory. It sets the visibility profile for
conditional fenced divs (refer to [Output
formats](formats.md#format-specific-blocks)). The supported profiles are `html`,
`pdf`, `docx`, `confluence`, and `markdown`.

~~~
$ docbake check -f html -i doc/en/index.md -b doc
~~~

If the check passes, the command prints statistics:

~~~
check passed: 95 anchors, 82 links, 7 diagrams
~~~

If the content that is visible for the profile has a broken link or a diagram
render error, the command prints all errors and exits with a non-zero code.
Hidden conditional blocks do not affect the result of the check.

The command renders diagrams in the same way as a PDF build. PlantUML, Mermaid,
D2, Draw.io, and Typst diagrams go through the related external tools. Thus you
find syntax errors in diagrams before you build the document.

As in a build, the flags `--theme`, `--cache`, and `--no-cache` control the
theme and the artifact cache.

## Markdown formatting {.unnumbered}

The `fmt` command formats a Markdown file, or all `*.md` files in a directory
recursively. The command applies these formatting rules:

- It wraps regular paragraphs at a width of 100 characters.
- It aligns pipe tables.
- It removes trailing spaces (but not hard line breaks).
- It normalizes empty lines (not more than one in sequence).
- It adds a final line break at the end of the file.

~~~
$ docbake fmt doc/en
~~~

For CI and pre-commit hooks, use the check mode:

~~~
$ docbake fmt --check doc/en
~~~

In the `--check` mode, the command prints a diff for each file that needs
formatting. Then it exits with a non-zero code.

## Reading metadata {.unnumbered}

The `meta` command reads a value from the YAML front matter of the document and
writes it to `stdout`. Use this command in scripts.

~~~
$ docbake meta -i doc/en/index.md title
Documentation Build System
~~~

The key accepts dot notation to get nested objects:

~~~
$ docbake meta -i doc/en/index.md confluence.server
https://confluence.example.com
~~~

Exit codes:

- 0 -- the command found the front matter. If the key exists, the command writes
  its value to `stdout`. If the key does not exist, the command writes nothing.
- 1 -- the file has no YAML front matter. The command writes an error message to
  `stderr`.

For compound values (arrays, dictionaries), the command writes compact JSON.

## Publishing to Confluence {.unnumbered}

The `publish confluence` command publishes the document to Confluence Server.
The connection parameters are in the `confluence` dictionary of the YAML front
matter (refer to [Document parameters](../document.md#document-parameters)):

~~~yaml
confluence:
  server: https://confluence.example.com
  space: PROJ
  path:
    - "Parent page"
    - "Document title"
~~~

The `path` value is a string or an array. If it is a string, the Builder creates
the page in the root of the space. If it is an array, the last item is the title
of the page, and the other items are the chain of ancestor pages. Each item of
the array is a page title or a literal ID in the format `id:12345678`.

For authentication, set the `CONFLUENCE_TOKEN` environment variable to a
Confluence personal access token (PAT). You can also store the token with the
`docbake auth confluence <server> [token]` command. The environment variable
takes precedence over the stored token.

~~~
$ export CONFLUENCE_TOKEN=...
$ docbake publish confluence -i doc/en/index.md -b doc
~~~

You can override any front matter parameter with the `--meta` flag. To remove
keys (for example, `confluence.path` when you change the space), use
`--meta-drop`:

~~~
$ docbake publish confluence -i doc/en/index.md -b doc \
    --meta-drop confluence.path \
    --meta confluence.server "https://other.example.com" \
    --meta confluence.space "NEWSPACE"
~~~

The section [Output formats](formats.md#confluence) describes the Confluence
renderer. This includes the behavior of the TOC and of the warnings at the start
of the page.

## Publishing to Google Docs {.unnumbered}

The `publish gdoc` command renders the document as DOCX and uploads it to Google
Drive. Google Drive converts the file to the native Google Docs format. The
target is in the `google-docs` dictionary of the YAML front matter:

~~~yaml
google-docs:
  id: https://docs.google.com/document/d/<document-id>
  folder: https://drive.google.com/drive/folders/<folder-id>
~~~

`id`

:  The existing document to update. If you do not give `id`, the command creates a
   new document. To write the ID of the new document back into the front matter of
   the source file, use the `--write-id` flag.

`folder`

:  The Google Drive folder for a new document.

Before you publish, run the `docbake auth gdoc` command. The command opens a
browser for OAuth2 consent and stores the refresh token in the credentials file.

The OAuth2 flow needs a Google Cloud OAuth client of the type "Desktop app".
Create this client in the Google Cloud console. Then give its ID and secret to
the Builder in one of these ways:

- Set the environment variables `DOCBAKE_GDOC_CLIENT_ID` and
  `DOCBAKE_GDOC_CLIENT_SECRET`.
- Set the keys `client_id` and `client_secret` in the `gdoc` section of the
  credentials file.

The environment variables take precedence over the credentials file.

~~~json
{
  "gdoc": {
    "client_id": "<id>.apps.googleusercontent.com",
    "client_secret": "<secret>"
  }
}
~~~

The credentials file is `<config-dir>/docbake/credentials.json`:

- Linux: `$XDG_CONFIG_HOME/docbake/credentials.json` (the default is
  `~/.config/docbake/credentials.json`)
- macOS: `~/Library/Application Support/docbake/credentials.json`
- Windows: `%APPDATA%\docbake\credentials.json`

For CI/CD, use a Google service account. The `--service-account` flag stores the
JSON key file of the account. With a service account, the Builder does not open
a browser:

~~~
$ docbake auth gdoc --service-account key.json
$ docbake publish gdoc -i doc/en/index.md -b doc
~~~

## Debugging

To see the final stream of Markdown events after the Builder expands `include`
blocks, rewrites links, and does the other internal transformations, run:

~~~
$ docbake dump path/to/document.md
~~~

The `dump` command writes to `stdout` the events that the parser gets from the
final flattened Markdown text of the document. Use it to see how the parser
reads the document.

To see the AST of one Markdown file (without the expansion of `include` blocks):

~~~
$ docbake parse path/to/document.md
~~~

For a detailed debug output of the parser, add the global `--debug-dump` flag:

~~~
$ docbake --debug-dump dump path/to/document.md
~~~

In this mode, the Builder writes all parser passes to `stderr`. This includes
the read operations of included files and the internal repeated parsing of
rewritten blocks.

## Installation

You can install the Builder in one of these ways:

- On macOS and Linux, use [Homebrew]:

    ~~~
  $ brew install growler/tap/docbake
    ~~~

  The formula also installs Typst. To install the diagram tools, add the options
  `--with-d2`, `--with-mermaid-cli`, and `--with-plantuml`.

- If Rust is installed, use the `cargo install` command:

    ~~~
  $ cargo install --path .
    ~~~

- Download the binary file from the releases and put it into a directory that is
  in the `PATH` environment variable. On Linux and macOS, also run `chmod +x
  docbake`.

To make PDF documents, also install these tools:

[Typst](https://typst.app/)

:  The Builder uses Typst to make PDF documents. Download Typst [from this
   link](https://github.com/typst/typst/releases/latest). You can download only the
   binary file `typst` or `typst.exe` and put it into the same directory as the
   Builder.

    Typst is also available in [Homebrew] and [Chocolatey].

[PlantUML](https://plantuml.com/)

:  The Builder uses PlantUML to make diagrams from source code. Download PlantUML
   [from this link](https://plantuml.com/download). Use the version "Compiled Jar
   BSD License".

[JRE or JDK](https://en.wikipedia.org/wiki/Java_virtual_machine#Java_Runtime_Environment)

:  PlantUML needs a JRE or a JDK. To find the installed version, run this command:

   ~~~
   java -version
   ~~~

   If a JRE or a JDK of version 8 or higher is not installed, download version 8
   from the [Oracle site](https://www.java.com/en/download/manual.jsp), or install
   the latest version of OpenJDK. To install OpenJDK, use the package manager of
   the system, or download the installer from the [Adoptium
   site](https://adoptium.net/en-GB/).

[ParaType Fonts](https://fonts.google.com/?query=ParaType):

:  The built-in default theme uses the ParaType fonts:

   - [PT Serif](https://fonts.google.com/specimen/PT+Serif) -- the main font for PDF
   - [PT Sans](https://fonts.google.com/specimen/PT+Sans) -- the main font for HTML
   - [PT Mono](https://fonts.google.com/specimen/PT+Mono) -- the monospace font

   Download these fonts and install them on your system. On Debian and Ubuntu,
   install the `fonts-paratype` package.

   The theme files set the fonts: `template.typ` and `style.css` for the main text,
   `theme.puml` and `theme.mmd` for PlantUML and Mermaid diagrams. To make a theme
   with different fonts, export the built-in theme with the `docbake init-theme`
   command and edit the files.

[Draw.io](https://www.drawio.com/)

:  The Builder can insert diagrams made in Draw.io into the document. Download
   Draw.io [from this link](https://get.diagrams.net/), or install it with a
   package manager.

[D2](https://d2lang.com/)

:  The Builder uses D2 to make diagrams from source code in the D2 language.
   Download D2 [from this link](https://github.com/terrastruct/d2/releases/latest),
   or install it with a package manager ([Homebrew], [Chocolatey], and others).

[Chocolatey]: https://chocolatey.org/ "Chocolatey -- a package manager for Windows"

[Homebrew]: https://brew.sh/ "Homebrew -- a package manager for macOS"

:::include
## [Themes and settings](config.md)
## [Formats](formats.md)
## [GitHub Action](github-action.md)
:::
