# Markdown

Markdown - это простой язык текстовой разметки, который позволяет, 
с одной стороны, писать текст в простом читаемом формате, и, при этом,
дает достаточно информации для разбора текста и его преобразования
в абстрактное синтаксическое дерево с последующей трансляцией в
целевой формат.

Существует много диалектов Markdown. Основой синтаксиса для системы 
подготовки документации является спецификация
[CommonMark](https://spec.commonmark.org/). Помимо базового синтаксиса,
поддерживаются следующие расширения:

- зачёркнутый текст (`~~текст~~`)
- [таблицы](markdown/tables-lists.md#таблицы) (pipe tables)
- [списки определений](markdown/tables-lists.md#списки-определений-definition-lists)
- [списки с чекбоксами](markdown/tables-lists.md#списки-с-чекбоксами-task-lists) (task lists)
- [сноски](markdown/links.md#сноски) (footnotes)
- [подсветка синтаксиса](markdown/text.md#блоки-кода) в блоках кода
- [метаданные документа](document.md#параметры-документа) в формате YAML (front matter)
- [атрибуты элементов](markdown/links.md#метки) (`{#id .class attr=value}`)
- [инлайн-спаны с атрибутами](markdown/links.md#метки) (`[текст]{#id .class}`)
- [огороженные блоки](markdown/diagrams.md#блоки-для-использования-в-конкретном-формате) (`:::block`)
- [включение файлов](document.md#include) (`:::include`)
- [подписи таблиц и диаграмм](markdown/tables-lists.md#подпись-таблицы) (`Table:`, `Figure:`, `:`)
- [диаграммы PlantUML](markdown/diagrams.md#диаграммы-plantuml), [Mermaid](markdown/diagrams.md#диаграммы-mermaid), [D2](markdown/diagrams.md#диаграммы-d2) и [Draw.io](markdown/diagrams.md#диаграммы-drawio)

:::include
## [Текстовая разметка](markdown/text.md)
## [Ссылки и сноски](markdown/links.md)
## [Таблицы и списки](markdown/tables-lists.md)
## [Иллюстрации](markdown/illustrations.md)
## [Диаграммы](markdown/diagrams.md)
:::
