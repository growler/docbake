# docbake

CLI-утилита для сборки многофайловой Markdown-документации в различные
выходные форматы: single-page HTML, multi-page HTML, PDF (через Typst) и
Confluence.

Gitlab CI сервер для сборки и публикации документации.

Поддерживаемый синтаксис приведен в [документации](doc/ru/index.md).

## Возможности

- **Форматы вывода:** `html`, `html-multi`, `pdf`, `typst`, `markdown`
- **Диаграммы:** PlantUML, Mermaid, D2, Typst, DrawIO — рендерятся в SVG, результат кешируется
- **Темы:** встроенная тема по умолчанию + поддержка пользовательских тем
- **Live-reload:** команда `serve` с автоматической пересборкой при изменении файлов
- **Валидация:** команда `check` — проверка ссылок и рендеринг всех диаграмм
- **Публикация:** прямая публикация в Confluence Server через REST API

## Использование

```bash
# Single-page HTML
docbake build -f html -i doc/ru/index.md -o target/doc/output.html

# Multi-page HTML
docbake build -f html-multi -i doc/ru/index.md -o target/doc/html

# PDF
docbake build -f pdf -i doc/ru/index.md -o target/doc/output.pdf

# Объединённый Markdown
docbake build -f markdown -i doc/ru/index.md -o target/doc/output.md

# Live-reload сервер
docbake serve -f html -i doc/ru/index.md

# Live-rebuild сервер (удобнее для PDF)
docbake watch -f pdf -i doc/ru/index.md -o output.pdf

[В](#в)алидация документа (ссылки + диаграммы)
docbake check -i doc/ru/index.md
```

Флаг `-b/--base` задаёт корневой каталог для разрешения абсолютных путей
в include и ссылках. По умолчанию — каталог входного файла.

## Темы

Экспорт встроенной темы для кастомизации:

```bash
docbake init-theme my-theme
```

Структура каталога темы:

```
my-theme/
  template.html              # Tera-шаблон для single-page HTML
  template-multipage.html    # Tera-шаблон для multi-page HTML
  template.typ               # Tera-шаблон для Typst/PDF
  style.css                  # CSS (подставляется как {{ style }})
  theme.puml                 # Тема PlantUML-диаграмм
  theme.mmd                  # Тема Mermaid-диаграмм
  theme.d2                   # Тема D2-диаграмм
  assets/                    # Статические файлы, доступные из шаблонов
```

Подключение: `docbake -t my-theme build ...`

## Публикация в Confluence

Команда `publish confluence` публикует документ в Confluence Server.
Параметры подключения указываются в front matter входного файла или через
CLI-флаги:

```yaml
---
title: Название документа
confluence:
  server: https://confluence.example.com
  space: PROJ
  path:
    - Parent Page            # поиск по заголовку
    - id:12345678            # или по content ID страницы
    - Название страницы
---
```

Каждый элемент `path` (и каждый `--parent` в CLI) — это либо заголовок
страницы, либо content ID с префиксом `id:`. Последний элемент — имя
целевой страницы, остальные — цепочка родителей.

Использование `id:` удобно, когда заголовок родительской страницы
слишком длинный или может измениться.

Для аутентификации используется переменная окружения `CONFLUENCE_TOKEN`
(Personal Access Token).

По умолчанию в CI/CD присутствует переменная `CONFLUENCE_BOT_TOKEN`, если 
ее значение дать переменной `CONFLUENCE_TOKEN`, то публикация будет выполняется
от имени пользователя **documentabot** — ему необходимо предоставить права
на запись в целевом пространстве Confluence.

## CI/CD

Примеры GitLab CI/CD job для генерации документации.

### Прямой вызов docbake

```yaml
build:docs:html:
  stage: build
  script:
    - docbake build -f html -i doc/ru/index.md -o public/index.html
  artifacts:
    paths:
      - public/

build:docs:html-multi:
  stage: build
  script:
    - docbake build -f html-multi -i doc/ru/index.md -o public
  artifacts:
    paths:
      - public/

build:docs:pdf:
  stage: build
  script:
    - docbake build -f pdf -i doc/ru/index.md -o public/document.pdf
  artifacts:
    paths:
      - public/
```

### Обёртка publish-docs

В проекте используется обёртка `publish-docs`, которая вызывает
`docbake` с нужными параметрами и сразу публикуте документ на сервере 
[http://doc.blank](http://doc.blank) с путем `<giltab-group>/<gitlab-project>/<branch|tag>/html` 
для HTML single page и `<giltab-group>/<gitlab-project>/<branch|tag>/pdf/<gitlab-group>-<gitlab-project>.pdf`
для PDF:

```yaml
build:docs:
  stage: build
  tags:
    - docs
  rules:
    - if: '$CI_COMMIT_BRANCH == "main"'
  script:
    - publish-docs -i doc/ru/index.md -b doc
```

