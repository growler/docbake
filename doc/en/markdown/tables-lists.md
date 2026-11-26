# Tables and lists

## Tables

Tables use the pipe table syntax. The `|` character divides the cells. The `':'`
character in the delimiter row sets the column alignment:

~~~markdown
Item    | Price
:-------|----:
SRV-001 |  100
SRV-002 |  200
~~~

:::related
# Result

Item    | Price
:------ | ----:
SRV-001 |   100
SRV-002 |   200
:::

### Table caption

To refer to a table, you must give it a caption. Write the caption as a separate
paragraph before or after the table. The paragraph starts with the word `Table:`
(or `table:`). Code blocks use the same type of prefix (`Listing:`), and figures
use `Figure:`.

~~~markdown
Table: Standards in use

| RFC      | Description             |
| -------- | ----------------------- |
| RFC 2822 | Internet Message Format |

...

The [table](# "Standards in use") lists the standards.
~~~

:::related
# Result

Table: Standards in use

| RFC      | Description             |
| -------- | ----------------------- |
| RFC 2822 | Internet Message Format |

...

The [table](# "Standards in use") lists the standards.
:::

You can set the table identifier explicitly in this format:

~~~markdown
Table: Standards in use {#rfc-table}

| RFC      | Description             |
| -------- | ----------------------- |
| RFC 2822 | Internet Message Format |

...

The [table](#rfc-table) lists the standards.
~~~

### Multiline tables

In a multiline table, the header and the body rows can use more than one
physical line. An empty line divides the body rows. The syntax is compatible
with [Pandoc multiline tables].

**Table with a header.** A continuous line of dashes starts the table. The
header lines come next. A delimiter line with groups of dashes (divided by
spaces) sets the width and the alignment of the columns. One more continuous
line ends the table body:

~~~markdown
-------------------------------------------------------------------
  Centered   Default      Right Left
  column     alignment
------------ --------- ------- ------------------------------------
  First       row         12.0 An example of a row that uses
                               more than one physical line.

  Second      row          5.0 One more row. Note the empty line
                               between the rows.
-------------------------------------------------------------------

Table: Example of a multiline table
~~~

:::related
# Result

-------------------------------------------------------------------
  Centered   Default      Righ                 Left
   column    alignment
------------ --------- ------- ------------------------------------
   First     row          12.0    An example of a row that uses
                                   more than one physical line.

   Second    row           5.0  One more row. Note the empty line
                                        between the rows.
-------------------------------------------------------------------

Table: Example of a multiline table
:::

**Table without a header.** If the table starts with a delimiter line (and not
with a continuous line), the table has no header lines. All rows are body rows.
In this case, the first body row sets the alignment.

~~~markdown
------------ ----------- ------- ------------------------------------
First        row            12.0 An example of a multiline row
                                 without a header.

Second       row             5.0 One more row.
----------- ------------ ------- ------------------------------------

Table: Example of a table without a header
~~~

:::related
# Result

------------ ----------- ------- ------------------------------------
First        row            12.0 An example of a multiline row
                                 without a header.

Second       row             5.0 One more row.
---------------------------------------------------------------------

Table: Example of a table without a header
:::

The column width in the source changes the output. If the text in a column is
too wide in PDF or HTML, make the group of dashes in the delimiter line longer.

To add a caption, write a `Table:` paragraph before or after the table (see
["Table caption"](# "Table caption")).

[Pandoc multiline tables]: https://pandoc.org/demo/example33/8.9-tables.html

### Grid tables

Grid tables use the `+`, `-`, `=` and `|` characters to draw the cell borders.
Each cell has an explicit frame. Thus, the cell content can use any number of
physical lines, and columns can merge (see below). The syntax is compatible with
[Pandoc grid tables].

**Basic form.** The top and the bottom borders are mandatory. A `+===+...+===+`
line (the `=` character instead of `-`) divides the header from the body. If
this line is not there, the table has no header, and all rows are body rows.

~~~markdown
+----------------+----------------+----------------------+
| Fruit          | Price          | Advantages           |
+================+================+======================+
| Bananas        | $1.00          | in a branded package |
+----------------+----------------+----------------------+
| Oranges        | $1.50          | prevent scurvy       |
+----------------+----------------+----------------------+

Table: Simple grid table
~~~

:::related
# Result

+----------------+----------------+----------------------+
| Fruit          | Price          | Advantages           |
+================+================+======================+
| Bananas        | $1.00          | in a branded package |
+----------------+----------------+----------------------+
| Oranges        | $1.50          | prevent scurvy       |
+----------------+----------------+----------------------+

Table: Simple grid table
:::

**Multiline cells.** If no `+---+...+---+` horizontal line divides two adjacent
body rows, the two rows merge into one logical row with multiline content.

~~~markdown
+--------------+---------------------------------+
| Parameter    | Description                     |
+==============+=================================+
| `max-retry`  | Maximum number of retries after |
|              | a temporary error. The default  |
|              | value is `3`.                   |
+--------------+---------------------------------+
| `timeout`    | Timeout of one request in       |
|              | milliseconds.                   |
+--------------+---------------------------------+
~~~

:::related
# Result

+--------------+---------------------------------+
| Parameter    | Description                     |
+==============+=================================+
| `max-retry`  | Maximum number of retries after |
|              | a temporary error. The default  |
|              | value is `3`.                   |
+--------------+---------------------------------+
| `timeout`    | Timeout of one request in       |
|              | milliseconds.                   |
+--------------+---------------------------------+
:::

**Column alignment.** A `:` marker next to a `+` in a delimiter line sets the
alignment of the related column: `+:--+` is left, `+--:+` is right, `+:-:+` is
center, and `+---+` is the default. The alignment comes from the first delimiter
line that contains markers (usually the `+===+` line).

~~~markdown
+----------+----------+------------+
| Left     | Right    | Center     |
+:=========+=========:+:==========:+
| a        | b        | c          |
+----------+----------+------------+
~~~

:::related
# Result

+----------+----------+------------+
| Left     |    Right |   Center   |
+:=========+=========:+:==========:+
| a        |        b |     c      |
+----------+----------+------------+
:::

**Merged cells (column spans).** To merge some columns into one cell, remove the
`|` character at the borders of the merged columns in the content line. The
outer `|` characters at the start and at the end of the line are mandatory. All
physical lines of one logical row must use the same delimiter positions.

The example below merges cells in the header and in the body. In the header,
`Contact details` spans columns 2-3. In the body, the last row merges all three
columns under a summary comment:

~~~markdown
+--------------+---------------+--------------+
| Employee     | Contact details              |
+--------------+---------------+--------------+
| Name         | Email         | Phone        |
+==============+===============+==============+
| Alice Smith  | a@example.com | +1 555 000-1 |
+--------------+---------------+--------------+
| Bob Jones    | b@example.com | +1 555 000-2 |
+--------------+---------------+--------------+
| All data is valid as of January 1           |
+--------------+---------------+--------------+

Table: Merged cells in the header and in the body
~~~

:::related
# Result

+--------------+---------------+--------------+
| Employee     | Contact details              |
+--------------+---------------+--------------+
| Name         | Email         | Phone        |
+==============+===============+==============+
| Alice Smith  | a@example.com | +1 555 000-1 |
+--------------+---------------+--------------+
| Bob Jones    | b@example.com | +1 555 000-2 |
+--------------+---------------+--------------+
| All data is valid as of January 1           |
+--------------+---------------+--------------+

Table: Merged cells in the header and in the body
:::

**Merged rows (row spans).** To make a cell span more than one row vertically,
put spaces instead of the `-` characters in the delimiter line below the cell.
Keep the `+` characters at the column borders. Pandoc requires that the
delimiter line has `+` at the same positions as the top border of the table.

In the example below, the `Alice Smith` cell spans two rows: the main row and
the row with an additional phone number. The `+ + +` delimiter line shows that
the left column continues (spaces instead of dashes) and the right column
closes.

~~~markdown
+--------------+-----------------+
| Employee     | Contact         |
+==============+=================+
| Alice Smith  | a@example.com   |
+              +-----------------+
|              | +1 555 000-1    |
+--------------+-----------------+
| Bob Jones    | b@example.com   |
+--------------+-----------------+

Table: Employee with two contact rows
~~~

:::related
# Result

+--------------+-----------------+
| Employee     | Contact         |
+==============+=================+
| Alice Smith  | a@example.com   |
+              +-----------------+
|              | +1 555 000-1    |
+--------------+-----------------+
| Bob Jones    | b@example.com   |
+--------------+-----------------+

Table: Employee with two contact rows
:::

::: note
A row span cannot cross the `+===+` border between the header and the body of
the table. Cells accept only inline formatting (`**bold**`, `*em*`, `` `code` ``
and more). Nested lists, code blocks and cells with more than one paragraph
render as inline text.
:::

[Pandoc grid tables]: https://pandoc.org/demo/example33/8.9-tables.html

### Hard line breaks in cells

In a cell of a multiline table or a grid table, you can insert an explicit hard
line break (`<br>` in the HTML output, `\ \n` in Typst/PDF). There are two
methods:

* **A backslash `\` at the end of a cell line.** If the last non-space character
  of the line is `\`, the parser removes it and inserts a hard line break. A
  double backslash `\\` stays a literal (as in usual Markdown).
* **Inline HTML `<br>`, `<br/>`, `<br />`** at any position in the cell. The match
  is not case-sensitive.

~~~markdown
---------------------------------------------------
 Parameter            Description
-------------------- ------------------------------
 `retry.backoff`      Delay between retries.\
                      The start value is 100 ms.

 `retry.max`          At most 10 retries.<br>
                      Then the client stops.
---------------------------------------------------

Table: Hard line breaks in a cell
~~~

:::related
# Result

---------------------------------------------------
     Parameter                Description
-------------------- ------------------------------
  `retry.backoff`       Delay between retries.\
                       The start value is 100 ms.

    `retry.max`           At most 10 retries.\

                         Then the client stops.
---------------------------------------------------

Table: Hard line breaks in a cell
:::

::: note
A hard line break is different from the usual merge of physical cell lines.
Without a marker, adjacent physical lines join into one paragraph. You see the
break only in the `--wrap=none` output. With a marker, the break is always
visible in HTML, PDF, DOCX and Confluence.
:::

## Lists

The format supports some types of lists.

### Bullet lists

A bullet list item starts with the `*`, `+` or `-` character:

If each list item starts on a new line, the list is compact:

~~~markdown
* Item 1
* Item 2
* Item 3
~~~

:::related
# Result

* Item 1
* Item 2
* Item 3
:::

If an empty line divides the list items, each item is a separate paragraph.
Items of this type of list can also contain other block elements.

~~~markdown
* Item 1

* Item 2

  ```
  code
  ```

* Item 3
~~~

:::related
# Result

* Item 1

* Item 2

  ```
  code
  ```

  -------------- ---------------
  A multiline             `true`
  table
  ------------------------------

* Item 3
:::

The format also supports nested lists:

~~~markdown
* Item 1
  + Item 1.1
  + Item 1.2
* Item 2
~~~

:::related
# Result

* Item 1
  + Item 1.1
  + Item 1.2
* Item 2
:::

In this case, an empty line between the list items is not necessary.

### Ordered lists

An ordered list item starts with a number and a period or a closing parenthesis.
The number of the first item sets the start value of the numbering.

~~~markdown
1. Item 1
   1. Item 1.1
   2. Item 1.2
2. Item 2
~~~

:::related
# Result

1. Item 1
   1. Item 1.1
   2. Item 1.2
2. Item 2
:::

You do not have to number the items in the document. You can always use the same
number.

~~~markdown
1) Item 1
   1) Item 1.1
   1) Item 1.2
1) Item 2
~~~

:::related
# Result

<!-- The formatter keeps renumbering so leave it be -->

1) Item 1
   1) Item 1.1
   2) Item 1.2
2) Item 2
:::

<!-- TODO: Add support for the `#` character instead of a number for automatic
     numbering (for example, `#. Item`). -->

### Definition lists

A definition list contains terms and their definitions. A term uses a separate
line. The next line contains the definition and starts with a colon `:`. An
indent of up to three spaces is permitted before the colon. At least one space
or tab must come after the colon.

An empty line must divide a term from the previous definition. One term can have
more than one definition. Also, one definition can apply to more than one
consecutive term.

If no empty line divides the term and the definition, the definition is compact
and is not a separate paragraph. Use this form for short definitions.

~~~markdown
Apple
:   The fruit of plants of the genus Malus.

Orange
:   The fruit of evergreen trees of the genus Citrus.
~~~

:::related
# Result

Apple
:   The fruit of plants of the genus Malus.

Orange
:   The fruit of evergreen trees of the genus Citrus.
:::

A definition can use more than one line. You can indent the continuation lines,
but this is not mandatory.

~~~markdown
Apple
:   The fruit of plants of the genus Malus, family Rosaceae.

Orange
:    The fruit of evergreen trees of the genus Citrus,
that usually has an orange peel.
~~~

:::related
# Result

Apple
:   The fruit of plants of the genus Malus, family Rosaceae.

Orange
:   The fruit of evergreen trees of the genus Citrus, that usually has an orange
    peel and a round shape.
:::

If an empty line comes before the definition, its content is a separate
paragraph. This type of definition can contain more than one paragraph and other
block elements.

~~~markdown
Term

:   This definition starts with a separate paragraph.

    This is the second paragraph of the same definition.

    ```
    code example
    ```

    | Header | Value |
    |--------|-------|
    | Table  | `nil` |
~~~

:::related
# Result

Term

:   This definition starts with a separate paragraph.

    This is the second paragraph of the same definition.

    ```
    code example
    ```

    | Header | Value |
    | ------ | ----- |
    | Table  | `nil` |
:::

One term can have more than one definition:

~~~markdown
Apple
:   The fruit of plants of the genus Malus.
:   The name of a technology company.
~~~

More than one term can also share one definition:

~~~markdown
Term 1
Term 2
:   A shared definition.
~~~

### Task lists

List items can contain checkboxes. To add a checkbox, write `[ ]` (not selected)
or `[x]` (selected) after the list marker:

~~~markdown
- [x] The task is done
- [ ] The task is not done
- [ ] One more task
~~~

:::related
# Result

- [x] The task is done
- [ ] The task is not done
- [ ] One more task
:::
