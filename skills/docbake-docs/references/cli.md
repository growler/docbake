# Builder commands

## Global flags

`-t/--theme <name-or-dir>`
:  Built-in theme name (`blank`) or a theme directory. The default is `blank`.

`--cache <dir>`
:  Directory for rendered diagrams. The default is the user cache directory of
   the platform.

`--no-cache`
:  Turn off the diagram cache.

`--debug-dump`
:  Write every parser pass to `stderr`.

## Build

```bash
docbake build -f pdf  -i doc/en/index.md -o target/doc-out/builder.pdf  -b doc
docbake build -f page -i doc/en/index.md -o target/doc-out/builder.html -b doc
docbake build -f site -i doc/en/index.md -o target/doc-out/builder      -b doc
docbake build -f docx -i doc/en/index.md -o target/doc-out/builder.docx -b doc
```

`-o` names a file when it ends in `.html`, `.pdf`, `.md`, or `.docx`. Otherwise
it names a directory and the file name comes from the input file. For `site`
and `typst`, `-o` is always a directory. `-b` sets the base directory used to resolve
absolute paths, and defaults to the directory of the input file.

`--broken-links warn|error` sets what a broken link does. `error` stops the
build and is the default for `build` and `publish`. `warn` prints the problem
and continues, and is the default for `serve` and `watch`. `check` has no such
flag. In all modes a broken link to an anchor in the same file only prints a
warning.

## Check

```bash
docbake check -f html -i doc/en/index.md -b doc
```

`check` writes no output files. `-f` takes a profile name: `html`, `pdf`,
`docx`, `confluence`, or `markdown`. A successful run reports the counts, for
example `check passed: 95 anchors, 82 links, 7 diagrams`. A broken link or a
diagram error in content that is visible in the profile produces a non-zero exit
status and a full error list. Hidden conditional blocks do not affect the
result.

## Format

```bash
docbake fmt doc/en
docbake fmt --check doc/en
```

`fmt` takes a file or a directory. It wraps ordinary paragraphs at 80 columns,
aligns pipe tables, changes setext headings to ATX headings, renumbers ordered
lists, strips trailing whitespace except for hard line breaks, collapses
repeated blank lines, and ensures a final newline. `--check` prints a
diff for each file that needs formatting and exits non-zero. Use it in CI and
pre-commit hooks.

## Preview and watch

```bash
docbake serve -f page -i doc/en/index.md -b doc
docbake serve -f page -i doc/en/index.md -b doc -p 3000
docbake watch -f page -i doc/en/index.md -o target/doc-out/builder.html -b doc
```

`serve` supports the `page` and `site` formats. It binds to `127.0.0.1`, picks a
free port unless `-p` is given, opens a browser, and reloads the page when a
file under the base directory changes. `watch` rebuilds to a file or directory
without serving.

## Publish

```bash
docbake publish confluence -i doc/en/index.md -b doc
docbake publish gdoc -i doc/en/index.md -b doc
docbake publish gdoc -i doc/en/index.md -b doc --write-id
```

`publish confluence` reads the target from the `confluence` front matter key.
Without `confluence.path`, the page title is the document `title`.

`publish gdoc` renders the document as DOCX and uploads it to Google Drive,
which converts it to a Google Doc. `google-docs.id` in the front matter selects
an existing document to update. `google-docs.folder` selects the target folder.
`--write-id` writes the ID of a new document back into the front matter of the
source file.

## Credentials

```bash
docbake auth confluence https://confluence.example.com
docbake auth confluence https://confluence.example.com <token>
docbake auth gdoc
docbake auth gdoc --service-account key.json
```

`auth confluence` stores a Personal Access Token. Without the token argument it
reads the token from standard input. `CONFLUENCE_TOKEN` takes priority over the
stored token.

For Google Docs, the order is `GOOGLE_APPLICATION_CREDENTIALS`, then a stored
service account key, then a stored OAuth2 refresh token. `auth gdoc` without
`--service-account` opens a browser for consent. `--service-account` stores a
key file and needs no browser, which suits CI. `DOCBAKE_GDOC_CLIENT_ID` and
`DOCBAKE_GDOC_CLIENT_SECRET` set the OAuth2 client and take priority over the
stored client.

## Read one metadata value

```bash
docbake meta -i doc/en/index.md title
docbake meta -i doc/en/index.md confluence.server
```

Exit status 0 means the front matter was found. A missing key prints nothing.
Exit status 1 means the file has no front matter. Arrays and objects print as
compact JSON.

## Metadata overrides

`--meta KEY VALUE`
:  Override one value. Repeatable. The key supports dotted notation for nested
   objects, and missing intermediate objects are created. For `variants`, each
   flag adds one value to the array from the front matter.

`--meta-drop KEY`
:  Remove a key before `--meta` is applied. This is the only way to remove an
   array or a nested object.

`--meta-file path.yml`
:  Merge a YAML file with the front matter at the top level of keys. The merge
   is shallow, so a `confluence` key in the file replaces the whole `confluence`
   object from the front matter.

Order of application: front matter, then `--meta-file`, then `--meta-drop`, then
`--meta`. These flags are available in `build`, `serve`, `watch`, `check`,
`publish confluence`, and `publish gdoc`.

Build with two variants:

```bash
docbake build -f page -i doc.md -o out.html \
  --meta variants enterprise \
  --meta variants foo
```

Move a document to a different Confluence space:

```bash
docbake publish confluence -i doc.md \
  --meta-drop confluence.path \
  --meta confluence.server "https://other.example.com" \
  --meta confluence.space "NEWSPACE"
```

## Themes

```bash
docbake init-theme my-theme
docbake -t my-theme init-theme my-copy
```

`init-theme` exports the selected theme to a new directory for customisation.
The target directory must not exist.

## Debugging the parser

```bash
docbake dump path/to/document.md
docbake parse path/to/document.md
docbake --debug-dump dump path/to/document.md
```

`dump` prints each document of the include graph: its includes with the level
shift, its headings with their final IDs, and its link rewrites. `parse` prints
the AST of every included file. `--debug-dump` writes debug log lines to
`stderr`.
