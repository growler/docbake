# docbake

CLI-утилита для сборки многофайловой Markdown-документации в различные
выходные форматы: single-page HTML, multi-page HTML, PDF (через Typst) и
Confluence.

Gitlab CI сервер для сборки и публикации документации.

Поддерживаемый синтаксис приведен в [документации](doc/ru/index.md).

## Возможности

- **Форматы вывода:** `page`, `site`, `pdf`, `typst`, `markdown`
- **Диаграммы:** PlantUML, Mermaid, D2, Typst, DrawIO — рендерятся в SVG, результат кешируется
- **Темы:** встроенные темы (`blank`, `blank`, `digitaltide`) + поддержка пользовательских тем
- **Live-reload:** команда `serve` с автоматической пересборкой при изменении файлов
- **Валидация:** команда `check` — проверка ссылок и рендеринг всех диаграмм
- **Форматирование:** команда `fmt` — выравнивание таблиц и перенос параграфов
- **Публикация:** прямая публикация в Confluence Server через REST API

## Использование

```bash
# Single-page HTML
docbake build -f page -i doc/ru/index.md -o target/doc/output.html

# Multi-page HTML
docbake build -f site -i doc/ru/index.md -o target/doc/html

# PDF
docbake build -f pdf -i doc/ru/index.md -o target/doc/output.pdf

# Объединённый Markdown
docbake build -f markdown -i doc/ru/index.md -o target/doc/output.md

# Live-reload сервер
docbake serve -f page -i doc/ru/index.md

# Live-rebuild сервер (удобнее для PDF)
docbake watch -f pdf -i doc/ru/index.md -o output.pdf

Валидация документа (ссылки + диаграммы) для конкретного профиля
docbake check -f html -i doc/ru/index.md

# Форматирование markdown-файла или каталога
docbake fmt doc/ru

# Проверка форматирования без записи (diff + код возврата)
docbake fmt --check doc/ru
```

Флаг `-b/--base` задаёт корневой каталог для разрешения абсолютных путей
в include и ссылках. По умолчанию — каталог входного файла.

## Кросс-сборка

- `make cross` — кросс-сборка в Linux-контейнере через `debles`.
- `make mac-cross` — кросс-сборка напрямую на macOS-хосте (через `cargo-zigbuild`, без контейнера).

Обе команды собирают цели:

- `x86_64-unknown-linux-musl`
- `x86_64-pc-windows-gnu`
- `aarch64-apple-darwin`
- `x86_64-apple-darwin`

## Темы

Экспорт выбранной темы для кастомизации (по умолчанию экспортируется `blank`):

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

Подключение:

- `docbake -t blank build ...` — встроенная тема по имени
- `docbake -t my-theme build ...` — локальный каталог темы

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

По умолчанию в CI/CD публикация будет выполняется от имени пользователя 
**documentabot** — ему необходимо предоставить права на запись в целевом 
пространстве Confluence. Если надо публиковать от своего имени, то надо 
в настройках CI/CD проекта задать переменную `CONFLUENCE_TOKEN` с личным 
Personal Access Token.

## CI/CD

Примеры GitLab CI/CD job для генерации документации.

### Прямой вызов docbake

```yaml
build:docs:html:
  stage: build
  script:
    - docbake build -f page -b doc -i doc/ru/index.md -o public/index.html
  artifacts:
    paths:
      - public/

build:docs:html-multi:
  stage: build
  script:
    - docbake build -f site -b doc -i doc/ru/index.md -o public
  artifacts:
    paths:
      - public/

build:docs:pdf:
  stage: build
  script:
    - docbake build -f pdf -b doc -i doc/ru/index.md -o public/document.pdf
  artifacts:
    paths:
      - public/

build:docs:kb:
  stage: build
  script:
    - |
      docbake publish confluence \
        --meta confluence.server https://confluence.example.com \
        --meta confluence.space SA \
        --meta confluence.path id:204837448 \
        --meta confluence.path "Document Publisher" \ 
        -b doc -i doc/ru/index.md
```

### Сервер docs.blank 

Есть сервис doc.blank с настроенным окружением для публикации документов. Для 
удобства, там есть обёртка `publish-docs`, которая вызывает
`docbake` с нужными параметрами и сразу публикует документ во всех форматах:

HTML Single Page: `<giltab-group>/<gitlab-project>/<branch|tag>/html`\
HTML Multi Page: `<giltab-group>/<gitlab-project>/<branch|tag>/site`\
PDF: `<giltab-group>/<gitlab-project>/<branch|tag>/pdf/<gitlab-group>-<gitlab-project>.pdf`

а также публикует в kb.blank.com, если в преамбуле есть соответствующие параметры.

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
