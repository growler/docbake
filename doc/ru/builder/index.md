# Сборщик

Сборщик представляет собой исполняемый файл `docbake` или `docbake.exe`.
Его задача состоит в валидации Markdown-документации проекта и в генерации
результатов сборки. Поддерживаемые форматы вывода:

- **page** — single-page HTML документ с встроенными стилями и таблицей содержания
- **site** — multi-page HTML с навигацией между страницами
- **PDF** — документ на базе Typst
- **Typst** — промежуточный `.typ` файл
- **DOCX (docx-gdoc)** — DOCX в том же формате, что используется в `publish gdoc`
- **Markdown** — единый Markdown-файл, полученный после раскрытия всех `include`,
  переписывания ссылок и сборки сносок

Для диаграмм сборщик также опирается на внешние инструменты, генерирующие SVG из
исходников PlantUML, D2 и Draw.io.

~~~
$ docbake --help
Produce single-page HTML, multi-page HTML, or PDF from markdown

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
  -t, --theme <THEME>  Theme name or directory. Embedded names: blank,
                        blank, digitaltide. When omitted the built-in
                        blank theme is used
      --cache <CACHE>  Override the default cache directory for rendered
                        artifacts
      --no-cache       Disable artifact caching entirely
  -h, --help           Print help
~~~

~~~
$ docbake build --help
Build a document in the specified format

Usage: docbake build [OPTIONS] --format <FORMAT> --input <INPUT>
                         --output <OUTPUT>

Options:
  -f, --format <FORMAT>          Output format [possible values: page,
                                 site, pdf, typst, markdown,
                                 docx-gdoc]
  -i, --input <INPUT>            Root markdown file
  -o, --output <OUTPUT>          Output file or directory
  -b, --base <BASE>              Base directory for absolute path resolution.
                                 Defaults to the parent directory of the
                                 input file
      --broken-links <MODE>      How to handle broken links: "warn" or
                                 "error" [default: error]
      --meta <KEY> <VALUE>       Override a front-matter value. Repeatable.
                                 Key may use dot-notation
      --meta-drop <KEY>          Remove a front-matter key before applying
                                 --meta overrides. Repeatable
      --meta-file <PATH>         Merge top-level metadata keys from a YAML
                                 file. Keys with the same name replace
                                 front-matter keys
  -h, --help                     Print help
~~~

~~~
$ docbake serve --help
Serve a document with live-reload on file changes

Usage: docbake serve [OPTIONS] --format <FORMAT> --input <INPUT>

Options:
  -f, --format <FORMAT>          Output format (only page and site
                                 are supported) [possible values: page,
                                 site, pdf, typst, markdown]
  -i, --input <INPUT>            Root markdown file
  -b, --base <BASE>              Base directory for absolute path resolution.
                                 Defaults to the parent directory of the
                                 input file
  -p, --port <PORT>              Port to listen on. If not specified, an
                                 available port is chosen automatically
      --broken-links <MODE>      How to handle broken links: "warn" or
                                 "error" [default: warn]
      --meta <KEY> <VALUE>       Override a front-matter value. Repeatable.
                                 Key may use dot-notation
      --meta-drop <KEY>          Remove a front-matter key before applying
                                 --meta overrides. Repeatable
      --meta-file <PATH>         Merge top-level metadata keys from a YAML
                                 file. Keys with the same name replace
                                 front-matter keys
  -h, --help                     Print help
~~~

## Использование

Для сборки документа в формате PDF необходимо выполнить команду:

~~~
$ docbake build -f pdf -i doc/ru/index.md -o target/doc-out/builder.pdf -b doc
~~~

Для сборки документа в формате HTML:

~~~
$ docbake build -f page -i doc/ru/index.md -o target/doc-out/builder.html -b doc
~~~

Для сборки multi-page HTML:

~~~
$ docbake build -f site -i doc/ru/index.md -o target/doc-out/builder -b doc
~~~

Для сборки объединённого Markdown-файла:

~~~
$ docbake build -f markdown -i doc/ru/index.md -o target/doc-out/builder.md -b doc
~~~

Флаг `-o` определяет файл или каталог вывода. Если `-o` указывает на файл с
расширением `.html`, `.pdf` или `.md`, результат записывается в этот файл. Иначе
`-o` интерпретируется как каталог, а имя файла определяется из имени входного
файла. Для формата `site` `-o` всегда является каталогом.

Флаг `-b` задаёт базовый каталог для разрешения абсолютных путей (по умолчанию —
каталог входного файла). Флаг `-t` позволяет указать каталог собственной темы.

### Переопределение метаданных {#переопределение-метаданных .unnumbered}

Флаг `--meta KEY VALUE` позволяет переопределить значение из YAML-преамбулы
документа при сборке. Флаг можно указывать несколько раз. Ключ поддерживает
точечную нотацию для обращения к вложенным объектам.

~~~
$ docbake build -f page -i doc.md -o out.html \
    --meta title "Новый заголовок" \
    --meta confluence.server "https://other.example.com"
~~~

Если промежуточный объект не существует, он будет создан автоматически.
Переопределение действует только на текущую сборку и не изменяет исходный файл.

Для ключа `variants` повторяющийся `--meta` формирует массив значений:

~~~
$ docbake build -f page -i doc.md -o out.html \
    --meta variants enterprise \
    --meta variants foo
~~~

Это используется условием `if-variant` в fenced div.

Флаг `--meta-file path.yml` позволяет загрузить YAML-файл и слить его с
преамбулой документа по верхнему уровню ключей. Это **не глубокое** слияние:
если в файле есть ключ `confluence`, он полностью заменяет весь объект
`confluence` из преамбулы.

Порядок применения метаданных: преамбула документа -> `--meta-file` ->
`--meta-drop` -> `--meta`.

Флаг `--meta-drop KEY` удаляет указанный ключ из метаданных **до** применения
`--meta`. Это полезно для удаления массивов и вложенных объектов, которые
невозможно переопределить через `--meta`. Ключ также поддерживает точечную
нотацию.

Например, при смене пространства Confluence необходимо удалить массив `path`,
чтобы он не указывал на несуществующие страницы в новом пространстве:

~~~
$ docbake publish confluence -i doc.md \
    --meta-drop confluence.path \
    --meta confluence.server "https://other.example.com" \
    --meta confluence.space "NEWSPACE"
~~~

Можно также удалить весь вложенный объект и задать его заново:

~~~
$ docbake publish confluence -i doc.md \
    --meta-drop confluence \
    --meta confluence.server "https://other.example.com" \
    --meta confluence.space "NEWSPACE"
~~~

Флаги `--meta`, `--meta-drop` и `--meta-file` доступны в командах `build`,
`serve`, `watch`, `check`, `publish confluence` и `publish gdoc`.

## Предварительный просмотр {.unnumbered}

Команда `serve` запускает локальный HTTP-сервер и открывает документ в браузере.
При изменении любого файла в базовом каталоге документ автоматически
пересобирается, а страница в браузере перезагружается.

Поддерживаются только HTML-форматы: `page` и `site`.

Для single-page HTML:

~~~
$ docbake serve -f page -i doc/ru/index.md -b doc
~~~

Для multi-page HTML:

~~~
$ docbake serve -f site -i doc/ru/index.md -b doc
~~~

Сервер автоматически выбирает свободный порт, печатает URL и открывает его в
браузере. Конкретный порт можно задать флагом `-p`:

~~~
$ docbake serve -f page -i doc/ru/index.md -b doc -p 3000
~~~

Сервер работает на `127.0.0.1` (loopback) и доступен только локально. Для
завершения работы нажмите `Ctrl+C`.

## Отслеживание изменений {.unnumbered}

Команда `watch` выполняет сборку документа и отслеживает изменения файлов в
базовом каталоге. При обнаружении изменений документ автоматически
пересобирается. В отличие от `serve`, `watch` не запускает HTTP-сервер, а
записывает результат в файл или каталог.

~~~
$ docbake watch -f page -i doc/ru/index.md -o target/doc-out/builder.html -b doc
~~~

Для завершения работы нажмите `Ctrl+C`.

## Проверка документа {.unnumbered}

Команда `check` загружает документ, проверяет ссылки и рендерит диаграммы для
выбранного профиля вывода. В отличие от `build`, команда не создаёт выходных
файлов — она предназначена исключительно для валидации.

Флаг `-f/--format` обязателен и определяет профиль видимости условных fenced div
(см. [Форматы вывода](formats.md#блоки-для-использования-в-конкретном-формате)).
Поддерживаемые профили: `html`, `pdf`, `docx`, `confluence`, `markdown`.

~~~
$ docbake check -f html -i doc/ru/index.md -b doc
~~~

При успешной проверке выводится статистика:

~~~
check passed: 95 anchors, 82 links, 7 diagrams
~~~

Если обнаружена битая ссылка или ошибка рендеринга диаграммы в **видимом** для
профиля содержимом, команда завершается с ненулевым кодом возврата и выводит все
ошибки. Скрытые условные блоки не влияют на результат проверки.

Диаграммы рендерятся так же, как при сборке PDF: PlantUML, Mermaid, D2, Draw.io
и Typst-диаграммы проходят через соответствующие внешние инструменты. Это
позволяет обнаружить синтаксические ошибки в диаграммах до сборки документа.

Как и при сборке, флаги `--theme`, `--cache` и `--no-cache` позволяют управлять
темой и кешированием артефактов.

## Форматирование markdown {.unnumbered}

Команда `fmt` форматирует markdown-файл или рекурсивно все `*.md` в каталоге.
Текущие правила форматирования:

- перенос обычных параграфов на ширину 100 символов;
- выравнивание pipe-таблиц;
- удаление хвостовых пробелов (кроме hard line break);
- нормализация пустых строк (не более одной подряд);
- финальный перевод строки в конце файла.

~~~
$ docbake fmt doc/ru
~~~

Для CI и pre-commit можно использовать режим проверки:

~~~
$ docbake fmt --check doc/ru
~~~

В режиме `--check` команда печатает diff для каждого файла, который требует
форматирования, и завершается с ненулевым кодом возврата.

## Чтение метаданных {.unnumbered}

Команда `meta` извлекает значение из YAML-преамбулы документа и выводит его в
`stdout`. Команда предназначена для использования в скриптах.

~~~
$ docbake meta -i doc/ru/index.md title
Система подготовки документации
~~~

Ключ поддерживает точечную нотацию для доступа к вложенным объектам:

~~~
$ docbake meta -i doc/ru/index.md confluence.server
https://confluence.example.com
~~~

Коды возврата:

- **0** — преамбула найдена. Если ключ существует, его значение выводится в
  `stdout`; если ключ отсутствует, ничего не выводится.
- **1** — файл не содержит YAML-преамбулы (сообщение об ошибке выводится в
  `stderr`).

Для составных значений (массивы, словари) выводится компактный JSON.

## Публикация в Confluence {.unnumbered}

Команда `publish confluence` публикует документ в Confluence Server. Параметры
подключения задаются в YAML-преамбуле документа в виде словаря `confluence` (см.
[Параметры документа](../document.md#параметры-документа)):

~~~yaml
confluence:
  server: https://confluence.example.com
  space: PROJ
  path:
    - "Родительская страница"
    - "Название документа"
~~~

Значение `path` может быть строкой или массивом. Если это строка — страница
создаётся непосредственно в корне пространства. Если массив — последний элемент
используется как заголовок страницы, остальные — как цепочка страниц-предков.
Каждый элемент массива может быть заголовком страницы или литеральным ID в
формате `id:12345678`.

Для аутентификации необходимо задать переменную окружения `CONFLUENCE_TOKEN` с
персональным токеном (PAT) Confluence.

~~~
$ export CONFLUENCE_TOKEN=...
$ docbake publish confluence -i doc/ru/index.md -b doc
~~~

Любой параметр преамбулы можно переопределить флагом `--meta`. Для удаления
ключей (например, `confluence.path` при смене пространства) используется
`--meta-drop`:

Особенности Confluence-рендера (включая поведение TOC и предупреждений в начале
страницы) описаны в разделе [Форматы вывода](formats.md#confluence).

~~~
$ docbake publish confluence -i doc/ru/index.md -b doc \
    --meta-drop confluence.path \
    --meta confluence.server "https://other.example.com" \
    --meta confluence.space "NEWSPACE"
~~~

## Отладка

Для просмотра итогового потока Markdown-событий после раскрытия `include`,
переписывания ссылок и остальных внутренних преобразований необходимо выполнить:

~~~
$ docbake dump path/to/document.md
~~~

Команда `dump` печатает в `stdout` те события, которые получаются из финального
flattened Markdown текста документа. Это удобно для отладки того, как документ в
итоге видит парсер.

Для просмотра AST конкретного Markdown-файла (без раскрытия `include`):

~~~
$ docbake parse path/to/document.md
~~~

Для детальной отладки процесса парсинга можно добавить глобальный флаг
`--debug-dump`:

~~~
$ docbake --debug-dump dump path/to/document.md
~~~

В этом режиме все проходы парсера, включая чтение включаемых файлов и внутренние
повторные разборы переписанных блоков, выводятся в `stderr`.

## Инсталляция

Установить Сборщик можно:

- при наличии установленного Rust с помощью команды `cargo install`:

    ~~~
  $ cargo install --path .
    ~~~

- скачав бинарный файл из релизов и положив его в каталог, который находится в
  переменной окружения `PATH`. Для Linux и macOS также потребуется сказать `chmod
  +x docbake`.

Для создания PDF документов необходимо также установить:

[Typst](https://typst.app/)

:  Сборщик использует Typst для создания PDF документов. Скачать Typst можно [по
   ссылке](https://github.com/typst/typst/releases/latest). Допустимо скачать
   только бинарный файл `typst` или `typst.exe` и положить его в тот же каталог,
   где находится Сборщик.

    Typst также есть в каталогах [Homebrew] и [Chocolatey].

[PlantUML](https://plantuml.com/)

:  Сборщик использует PlantUML для генерации диаграмм из исходного кода. Скачать
   PlantUML можно [по ссылке](https://plantuml.com/download). Рекомендуется
   скачивать версию "Compiled Jar BSD License".

[JRE или JDK](https://ru.wikipedia.org/wiki/Java_Runtime_Environment)

:  PlantUML требует для работы JRE или JDK. Скорее всего, у вас на машине уже стоит
   JRE или JDK версии 8 или выше. Проверить это можно, выполнив в командной строке
   команду

   ~~~
   java -version
   ~~~

   Если JRE или JDK не установлен, то можно либо скачать версию 8 с [сайта
   Oracle](https://www.java.com/en/download/manual.jsp), либо установить последнюю
   версию OpenJDK. Это можно сделать или пакетным менеджером системы, или скачав
   установщик с [сайта Adoptium](https://adoptium.net/en-GB/).

[ParaType Fonts](https://fonts.google.com/?query=ParaType):

:  Встроенная тема по умолчанию использует шрифты ParaType:

   - [PT Serif](https://fonts.google.com/specimen/PT+Serif) — основной шрифт PDF
   - [PT Sans](https://fonts.google.com/specimen/PT+Sans) — основной шрифт HTML
   - [PT Mono](https://fonts.google.com/specimen/PT+Mono) — моноширинный шрифт

   Рекомендуется скачать и установить эти шрифты на свою систему. В Debian и Ubuntu
   это можно сделать, установив пакет `fonts-paratype`.

   Шрифты задаются в файлах темы: `template.typ` и `style.css` для основного
   текста, `theme.puml` и `theme.mmd` для диаграмм PlantUML и Mermaid
   соответственно. Для создания темы с другими шрифтами экспортируйте встроенную
   тему командой `docbake init-theme` и отредактируйте файлы.

[Draw.io](https://www.drawio.com/)

:  Сборщик может вставлять в документ диаграммы, созданные в Draw.io. Скачать
   Draw.io можно [по ссылке](https://get.diagrams.net/), либо установить при помощи
   пакетного менеджера.

[D2](https://d2lang.com/)

:  Сборщик использует D2 для генерации диаграмм из исходного кода на языке D2.
   Скачать D2 можно [по ссылке](https://github.com/terrastruct/d2/releases/latest),
   либо установить при помощи пакетного менеджера ([Homebrew], [Chocolatey] и
   другие).

[Chocolatey]: https://chocolatey.org/ "Chocolatey — пакетный менеджер для Windows"

[Homebrew]: https://brew.sh/ "Homebrew — пакетный менеджер для macOS"

:::include
## [Темы и настройки](config.md)
## [Форматы](formats.md)
:::
