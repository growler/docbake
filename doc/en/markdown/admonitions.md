# Admonitions

An admonition is a visually separate section of a document. It shows important
information to the reader. Each admonition type has its own color and default
title.

The builder supports these admonition types:

| Type        | Purpose                                  |
| ----------- | ---------------------------------------- |
| `note`      | A note or supplementary information      |
| `tip`       | A useful recommendation                  |
| `warning`   | A warning                                |
| `caution`   | A request to be careful                  |
| `important` | Critical information                     |
| `related`   | Related information with a neutral style |

## Basic syntax

To make an admonition, write a fenced div with the type name as its class. If
the admonition has no title, the builder uses the default title of the type:

~~~markdown
:::note
This is a note without a custom title.
The builder adds the title "Note".
:::
~~~

Result:

:::note
This is a note without a custom title. The builder adds the title "Note".
:::

## Admonitions with a title

If the first element in the admonition is a heading, this heading becomes the
title of the admonition. It replaces the default title:

~~~markdown
:::tip
## Tip of the day
Use admonitions to mark important
parts of the documentation.
:::
~~~

Result:

:::tip
## Tip of the day

Use admonitions to mark important parts of the documentation.
:::

## All admonition types

:::note
Note (note) -- supplementary information.
:::

:::tip
Tip (tip) -- a useful recommendation.
:::

:::warning
Warning (warning) -- pay attention.
:::

:::caution
Caution (caution) -- unwanted results can occur.
:::

:::important
Important (important) -- critical information.
:::

:::related
Related information (related) -- a neutral style.
:::

## Identifiers and attributes

You can give an admonition an identifier and other attributes. Use the standard
attribute syntax:

~~~markdown
:::{.warning #compatibility-warning}
## Compatibility
This feature is available in version 2.0 and later.
:::

For more information, see the [warning](#compatibility-warning).
~~~

Result:

:::{.warning #compatibility-warning}
## Compatibility

This feature is available in version 2.0 and later.
:::

For more information, see the [warning](#compatibility-warning).

An admonition is a fenced div. Thus the conditional attributes `if-format` and
`if-variant` also apply to admonitions:

~~~markdown
:::{.warning if-format="pdf" if-variant="enterprise"}
## PDF and Enterprise only
This warning is visible only in the applicable profile.
:::
~~~

## Nested content

An admonition can contain any markup: text formatting, lists, code blocks, and
tables:

~~~markdown
:::note
## Example with different content

Text with **bold**, *italic*, and `code`.

- First item
- Second item

```python
def hello():
    print("Hello!")
```

| Column A | Column B |
|----------|----------|
| One      | Two      |
:::
~~~

Result:

:::note
## Example with different content

Text with **bold**, *italic*, and `code`.

- First item
- Second item

```python
def hello():
    print("Hello!")
```

| Column A | Column B |
| -------- | -------- |
| One      | Two      |
:::
