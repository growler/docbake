# Tables

Three table syntaxes are available. Pick the smallest one that fits the content.

| Need                                   | Use             |
| -------------------------------------- | --------------- |
| Short single-line cells                | Pipe table      |
| Cells that wrap over several lines     | Multiline table |
| Merged cells                           | Grid table      |

Every table takes a caption. See the caption rules in `SKILL.md`.

No table syntax holds block content in a cell. A list, a code block, or a second
paragraph in a cell renders as inline text. Use hard line breaks (see the end
of this file) to put values on separate lines, or move the block content out of
the table.

## Pipe tables

Cells are separated by `|`. The separator row sets column alignment with `:`.

```markdown
Item    | Price
:-------|------:
SRV-001 |   100
SRV-002 |   200
```

Leading and trailing `|` are optional. The `fmt` command aligns the columns, so
write them roughly aligned and let `fmt` finish the job.

A pipe table cell holds inline markup only, and the whole cell must fit on one
physical line.

## Multiline tables

A multiline table lets header cells and body cells wrap over several physical
lines. Body rows are separated by a blank line. The syntax follows Pandoc
multiline tables.

Column boundaries come from the dash groups in the separator line. DOCX uses the
width of each dash group as the relative column width. HTML and PDF size the
columns from their content.

### With a header

A solid line of dashes opens the table. Header lines follow. A separator line of
dash groups sets the column widths and alignment. A second solid line closes the
table.

```markdown
-------------------------------------------------------------------
  Centered   Default   Right Left
   column    column
------------ --------- ------- ------------------------------------
   First      row        12.0 An example row whose text runs over
                              several physical lines.

   Second     row         5.0 Another row. Note the blank line
                              between body rows.
-------------------------------------------------------------------

Table: Multiline table example
```

### Without a header

A table that opens with a separator line instead of a solid line has no header.
Every line is a body row. Alignment comes from the first body row.

```markdown
------------ ----------- ------- ------------------------------------
First        row            12.0 A multiline row with no header.

Second       row             5.0 Another row.
----------- ------------ ------- ------------------------------------

Table: Headerless table example
```

## Grid tables

A grid table draws every cell border with `+`, `-`, `=`, and `|`. Cell content
can occupy any number of physical lines, and cells can be merged. The syntax
follows Pandoc grid tables.

The top and bottom borders are required. A `+===+` line separates the header
from the body. A table with no `+===+` line is headerless, and all its rows are
body rows.

```markdown
+----------------+----------------+----------------------+
| Fruit          | Price          | Advantage            |
+================+================+======================+
| Bananas        | 100            | sold in own wrapper  |
+----------------+----------------+----------------------+
| Oranges        | 150            | cure scurvy          |
+----------------+----------------+----------------------+

Table: Simple grid table
```

### Multiline cells

Two adjacent body lines with no `+---+` line between them belong to the same
logical row.

```markdown
+--------------+---------------------------------+
| Option       | Description                     |
+==============+=================================+
| `max-retry`  | Maximum number of retries after |
|              | a temporary error. Default `3`. |
+--------------+---------------------------------+
| `timeout`    | Single request timeout in       |
|              | milliseconds.                   |
+--------------+---------------------------------+
```

### Alignment

A `:` next to a `+` in any separator line sets that column's alignment:
`+:--+` left, `+--:+` right, `+:-:+` centered, `+---+` default. The alignment is
taken from the first separator line that carries markers, which is normally the
`+===+` line.

```markdown
+----------+----------+------------+
| Left     | Right    | Centered   |
+:=========+=========:+:==========:+
| a        | b        | c          |
+----------+----------+------------+
```

### Column spans

To merge columns, omit the `|` at the boundaries you want to merge in the
content line. The outer `|` at the start and the end of the line are required.
Every physical line of one logical row must use the same separator positions.

The example merges cells in the header and in the body.

```markdown
+--------------+---------------+--------------+
| Employee     | Contact details              |
+--------------+---------------+--------------+
| Name         | Email         | Phone        |
+==============+===============+==============+
| Ivanov I.I.  | iv@example.ru | +7 495 000-1 |
+--------------+---------------+--------------+
| Petrov P.P.  | pp@example.ru | +7 495 000-2 |
+--------------+---------------+--------------+
| All data is current as of 1 January         |
+--------------+---------------+--------------+

Table: Merged cells in the header and the body
```

### Row spans

To make a cell span several rows, leave spaces instead of `-` in the separator
line under that cell. Keep the `+` characters at every column boundary. Pandoc
requires the separator line to carry `+` in the same positions as the top
border.

In the example the cell `Ivanov I.I.` spans two rows. The separator line
`+              +    +` shows that the left column continues and the right
column closes.

```markdown
+--------------+-----------------+
| Employee     | Contact         |
+==============+=================+
| Ivanov I.I.  | iv@example.ru   |
+              +-----------------+
|              | +7 495 000-1    |
+--------------+-----------------+
| Petrov P.P.  | pp@example.ru   |
+--------------+-----------------+

Table: Employee with two contact rows
```

### Grid table limits

A row span cannot cross the `+===+` boundary between the header and the body.

A blank line inside a grid table ends it. A grid table with a syntax error
renders as a plain paragraph and produces no error, so check the output.

Grid cells hold inline markup only: `**bold**`, `*em*`, `` `code` `` and the
like. A nested list, a code block, or a multi-paragraph cell renders as inline
text.

## Hard line breaks inside cells

Inside any table cell (pipe, multiline, or grid), two markers force a hard line
break. The break becomes `<br>` in HTML and `\ \n` in Typst and PDF.

Backslash at the end of a cell line
:  When the last non-space character of the line is `\`, the parser removes it
   and inserts a hard break. A double backslash `\\` stays literal, as in
   ordinary Markdown.

Inline HTML `<br>`, `<br/>`, `<br />`
:  Recognised anywhere in the cell. The match is case insensitive.

```markdown
---------------------------------------------------
 Option               Description
-------------------- ------------------------------
 `retry.backoff`      Delay between retries.\
                      Initial value 100 ms.

 `retry.max`          At most 10 retries.<br>
                      After that the client stops.
---------------------------------------------------

Table: Hard breaks inside a cell
```

Without a marker, adjacent physical lines of a cell are joined into one
paragraph. With a marker the break is visible in HTML, PDF, DOCX, and
Confluence.
