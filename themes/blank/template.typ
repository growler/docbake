// ─────────────────────────────────────────────────────────────────────────
// Typst/PDF template for docbake.
//
// This is a Tera template that produces a Typst document.  The tool
// renders it with Tera first, then compiles the result with `typst`.
//
// Available Tera variables (injected by the tool):
//
//   body           — rendered document body as Typst markup
//   asset_prefix   — relative path prefix to the output root (always "")
//   logo           — logo path relative to the output root; not set if
//                    the document has no logo
//
// Front-matter metadata (from the root Markdown file):
//
//   title          — document title
//   subtitle       — document subtitle
//   date           — document date
//   author         — author name(s) as a single string
//   version        — document version
//   lang           — language code (default: "en")
//   region         — region subtag (parsed from lang, e.g. "ru" → "RU")
//   front_matter   — full front-matter object for custom fields
//
// All front-matter keys are optional.
//
// Theme choices (fonts, colours) are hardcoded below.
// To customise, run `docbake init-theme <dir>` and edit the copy.
// ─────────────────────────────────────────────────────────────────────────

// the highlight color
#let hfcolor = color.rgb("#0f4c81")


// text size
#let fontsize = 12pt
#let scale-main-font(scale) = calc.ceil(fontsize.pt() * scale) * 1pt 

// footnote size
#let footnotesize = scale-main-font(0.8) 

// ── Renderer-callable functions ─────────────────────────────────────────
// The document renderer emits calls to these functions.  Override them
// in a custom theme to change the visual appearance without touching
// the renderer itself.

// Task-list checkbox.  `checked` is a boolean.
#let task-checkbox(checked) = box(
  width: 1em,
  height: 1em,
  baseline: 15%,
  stroke: 0.5pt + luma(120),
  radius: 1.5pt,
  if checked {
    align(center + horizon,
      text(size: fontsize - 1pt, weight: "bold", sym.checkmark))
  },
)

// Bullet marker restored for non-task items inside a task list
// (where the list's own marker is suppressed).
#let task-bullet() = sym.bullet

// Thematic break (Markdown `---` / `***`).
#let thematic-break() = line(length: 100%)

// Inline image wrapper — sizes the image to the current line height
// so it sits on the text baseline.
#let inline-image(..args) = box(height: 1em, image(..args))

// ── Admonition (callout block) ──────────────────────────────────────────
// Called by the renderer for fenced divs with admonition classes:
//   note, warning, important, caution, tip, related.
#let admonition-colors = (
  note:      rgb("#2563eb"),
  tip:       rgb("#16a34a"),
  warning:   rgb("#d97706"),
  caution:   rgb("#ea580c"),
  important: rgb("#dc2626"),
  related:   rgb("#64748b"),
)

#let admonition(kind, title, body) = {
  let accent = admonition-colors.at(kind, default: rgb("#2563eb"))
  let bg = accent.lighten(92%)
  block(
    width: 100%,
    above: 1em,
    below: 1em,
    stroke: (left: 3pt + accent),
    radius: (right: 4pt),
    fill: bg,
    clip: true,
    stack(dir: ttb,
      // Title bar
      block(
        width: 100%,
        inset: (x: 10pt, y: 7pt),
        fill: accent.lighten(85%),
        text(font: "PT Sans", weight: "bold", size: fontsize, fill: accent, title),
      ),
      // Body
      block(
        width: 100%,
        inset: (x: 10pt, y: 8pt),
        body,
      ),
    ),
  )
}

#set page(
  paper: "a4",
  margin: (top: 2cm, bottom: 2cm, left: 2cm, right: 1.5cm),
  numbering: "1",
  header: context {
    if here().page() > 1 {
      stack(dir: ttb,
        stack(dir: ltr,
          h(6pt),
          {% if logo %}image("{{ asset_prefix }}{{ logo }}", height: footnotesize),{% endif %}
          align(right, text(size: footnotesize, fill: hfcolor, "{{ title | default(value="") }}")),
          h(6pt),
        ),
        v(4pt),
        line(length: 100%, stroke: (thickness: 0.5pt, cap: "round", paint: hfcolor)),
      )
    }
  },
  footer: context {
    if here().page() > 1 {
      stack(dir: ttb,
        line(length: 100%, stroke: (thickness: 0.5pt, cap: "round", paint: hfcolor)),
        v(4pt),
        align(top, text(fill: hfcolor, size: fontsize - 1pt)[
          {% if version %}#h(6pt) {{ version }}{% endif %} #h(1fr) #counter(page).display() #h(6pt)
        ]),
      )
    }
  },
)

#set par(justify: true)

{% if region %}
#set text(lang: "{{ lang | default(value="en") }}", region: "{{ region }}", font: "PT Serif", size: fontsize)
{% else %}
#set text(lang: "{{ lang | default(value="en") }}", font: "PT Serif", size: fontsize)
{% endif %}

#set heading(numbering: "1.1.1.")

// ── Heading styles ──────────────────────────────────────────────────────
// Sans-serif for all headings; larger sizes and accent for top levels.
#show heading: set text(font: "PT Sans")

#show heading.where(level: 1): it => {
  v(0.4em)
  block(below: 0.6em,
    stack(dir: ttb, spacing: 6pt,
      {
        if it.numbering != none {
          text(
            size: scale-main-font(1.8), 
            weight: "bold", 
            fill: hfcolor, 
            counter(heading).display(it.numbering),
          )
          h(0.3em)
        }
        text(size: scale-main-font(1.8), weight: "bold", fill: hfcolor, it.body)
      },
      line(length: 100%, stroke: 0.5pt + hfcolor),
    ),
  )
}

#show heading.where(level: 2): it => {
  v(0.3em)
  block(below: 0.5em, {
    if it.numbering != none {
      text(size: scale-main-font(1.45), weight: "bold", counter(heading).display(it.numbering))
      h(0.3em)
    }
    text(size: scale-main-font(1.45), weight: "bold", it.body)
  })
}

#show heading.where(level: 3): set text(size: scale-main-font(1.18))

// ── Code ────────────────────────────────────────────────────────────────
// Block code: full-width grey box.
// Strip link styling inside code so auto-detected URLs render as plain text.

#let style-number(number) = text(gray)[#number]

#let code(
  body,
  lang: none,
  start-line: none,
) = block(
  width: 100%,
  fill: luma(246),
  stroke: 0.5pt + luma(180),
  radius: 4pt,
  inset: 8pt,
  {
    show link: it => it.body
    show underline: it => it.body
    text(
      font: "PT Mono",
      size: scale-main-font(0.9),
      if start-line == none {
        raw(body, block: true, lang: lang) 
      } else {
        show raw.where(block: true): it => grid(
          columns: 2,
          align: (right, left),
          inset: (x: 0.25em, y: 0.3em),
          stroke: (x, y) => if x > 0 { (left: 0.5pt + luma(180)) },
          ..it.lines
            .enumerate()
            .map(((i, line)) => (style-number(i + start-line), line))
            .flatten()
        )
        raw(body, block: true, lang: lang) 
      }
    )
  }
)

#show figure.where(kind: raw): set block(breakable: true)

// Inline code: light background pill.
// Strip link styling inside code so auto-detected URLs render as plain text.
#let inline-code(
  body,
  lang: none,
) = box(
  fill: luma(240),
  outset: (x: 2pt, y: 2.5pt),
  radius: 2pt,
  {
    show link: it => it.body
    show underline: it => it.body
    text(
      font: "PT Mono",
      size: scale-main-font(0.9),
      raw(body, block: false)
    )
  }
)

// ── Block quotes ────────────────────────────────────────────────────────
#show quote.where(block: true): it => block(
  above: 1em,
  below: 1em,
  inset: (left: 12pt, right: 6pt),
  stroke: (left: 2pt + luma(180)),
  it.body,
)

// ── Links ───────────────────────────────────────────────────────────────
#show link: it => text(fill: rgb("#4b69c6"), it)

// ── Definition lists ────────────────────────────────────────────────────
// Customise these values to change the definition-list appearance.
#let dl-term-font        = "PT Sans"
#let dl-term-weight      = "bold"
#let dl-term-size        = fontsize
#let dl-description-indent = 2em
#let dl-entry-above      = 1.2em    // space before each entry
#let dl-entry-below      = 0.4em    // space after each entry
#let dl-term-desc-gap    = 5pt      // gap between term and description

#show terms: it => {
  for child in it.children {
    block(above: dl-entry-above, below: dl-entry-below,
      stack(dir: ttb, spacing: dl-term-desc-gap,
        text(font: dl-term-font, weight: dl-term-weight, size: dl-term-size, child.term),
        pad(left: dl-description-indent, child.description),
      ),
    )
  }
}

// ── Tables ──────────────────────────────────────────────────────────────
#set table(
  stroke: 0.5pt + luma(200),
  inset: (x: 10pt, y: 7pt),
)

#show figure.where(kind: table): set block(breakable: true)

// Renderer wrapper for Markdown tables.  `has-header: false` is emitted for
// pipe/grid/multiline tables without a header row, so the row-zero bold and
// fill styling is suppressed (a global `where(y: 0)` selector can't tell a
// header row apart from the first body row on its own).
#let data-table(has-header: true, ..args) = {
  show table.cell.where(y: 0): it => {
    if has-header {
      set text(weight: "bold", font: "PT Sans", size: fontsize - 0.5pt)
    }
    it
  }
  show table: set table(
    fill: (_, y) => if has-header {
      if y == 0 { luma(230) }
      else if calc.rem(y, 2) == 0 { luma(248) }
      else { none }
    } else {
      if calc.rem(y, 2) == 1 { luma(248) }
      else { none }
    },
  )
  table(..args)
}

// ── Figure captions ─────────────────────────────────────────────────────
#show figure.caption: it => text(size: scale-main-font(0.9), style: "italic", it)

// ── Listing (code block) captions ───────────────────────────────────────
// Place caption above the code block, left-align everything (unlike tables
// and images which stay centered), and set the language-dependent supplement.
#show figure.where(kind: raw): set figure.caption(position: top)
#show figure.where(kind: raw): set align(left)
{% if lang == "ru" %}
#show figure.where(kind: raw): set figure(supplement: "Листинг")
{% else %}
#show figure.where(kind: raw): set figure(supplement: "Listing")
{% endif %}

// ── Outline (TOC) styling ───────────────────────────────────────────────
#show outline.entry: it => {
  if it.level == 1 {
    v(4pt)
    strong(it)
  } else {
    it
  }
}

#set document(
  title: "{{ title | default(value="") }}",
  author: "{{ author | default(value="") }}",
  {% if date %}date: auto,{% endif %}
)

// ── Title page ──────────────────────────────────────────────────────────
{% if logo %}#align(left, image("{{ asset_prefix }}{{ logo }}", height: 2cm)){% endif %}
#v(1fr)
{% if title %}
#block(width: 100%,
  stack(dir: ttb, spacing: 8pt,
    align(right, text(font: "PT Sans", size: scale-main-font(2.5), weight: "bold", "{{ title }}")),
    line(length: 100%, stroke: (thickness: 0.5pt, cap: "round", paint: hfcolor)),
    {% if subtitle %}
    align(right, text(font: "PT Sans", size: scale-main-font(1.6), fill: luma(80), "{{ subtitle }}")),
    {% endif %}
  ),
)
{% endif %}
{% if version %}
#v(2em)
#align(right, text(font: "PT Sans", size: scale-main-font(1.45), fill: luma(100), [Version {{ version }}]))
{% endif %}
#v(2fr)
{% if date %}
#align(center, text(size: scale-main-font(1.27), fill: luma(80), "{{ date }}"))
{% endif %}
{% if author %}
#v(0.3em)
#align(center, text(size: scale-main-font(1.1), "{{ author }}"))
{% endif %}
#pagebreak()

// ── Table of contents ───────────────────────────────────────────────────
#outline(indent: 1.5em, depth: 3)
#pagebreak()

{{ body | safe }}
