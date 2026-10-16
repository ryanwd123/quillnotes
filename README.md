# Penmark

A small, keyboard-driven, what-you-see-is-what-you-get Markdown editor for Windows.
It is a single native executable (about 410 KB) built on Win32, Direct2D and
DirectWrite through [`windows-rs`](https://github.com/microsoft/windows-rs). There is no
web view or GPU framework, and it needs no runtime.

Inspired by [Velotype](https://github.com/manyougz/velotype), but rebuilt from scratch for
Windows only and without the menu bar.

> "Penmark" is a placeholder name. To rename, change `APP_NAME` in `src/app.rs`, the
> `name` in `Cargo.toml`, and the strings in `assets/penmark.rc`.

## How it works

The file stays plain Markdown; Penmark never converts it to another format.

* Lines you are editing (the cursor line, or every line in the selection) show
  their Markdown syntax, dimmed. Block prefixes such as `#`, `-` and `>` hang into
  the margin so the text does not shift.
* All other lines are rendered: hidden delimiters, real bullets and checkboxes,
  numbered lists, quote bars, callouts, code blocks, tables, rules, and local images.
* Code blocks and tables switch to source as a whole when the cursor enters them.
* `Ctrl+/` switches the whole document to plain source mode.

Supported syntax: ATX headings; paragraphs; bold, italic, strikethrough, `==highlight==`
and inline code; links, autolinks, bare URLs, `[[wiki links]]`; images; bullet, numbered
and task lists with nesting; blockquotes and `> [!NOTE]` callouts; fenced code; YAML
front matter; GFM tables; thematic breaks; HTML comments; backslash escapes; and HTML
entities.

## Keyboard

Press **F1** in the app for the full list. It opens [`assets/help.md`](assets/help.md),
rendered by the editor itself. The highlights:

| | |
| --- | --- |
| `Ctrl+B` `Ctrl+I` `Ctrl+E` `Ctrl+K` | Bold, italic, code, link |
| `Ctrl+1`…`6`, `Ctrl+Shift+7/8/9`, `Ctrl+Shift+.` | Headings, lists, tasks, quote |
| `Enter`, `Tab`, `Shift+Tab` | Continue, indent and outdent lists |
| `Ctrl+Enter` | Toggle a task's checkbox |
| `Ctrl+F`, `F3`, `Ctrl+G` | Find, find next, go to line (in the status bar) |
| `Alt+Up/Down`, `Ctrl+D`, `Ctrl+Shift+K` | Move, duplicate, delete lines |
| `Ctrl+/`, `Ctrl+Shift+T`, `Ctrl+=`/`-`/`0` | Source mode, theme, zoom (5% steps) |
| `Ctrl+Shift+P` | Command palette |

The zoom level and the window's size, position and maximized state are remembered
between runs. A saved position on a monitor that is no longer connected is ignored.

### Command palette

`Ctrl+Shift+P` opens a searchable list of commands. The search is split into
space-separated terms, and an item matches when it contains every term in any
order. For example, `mono cas` finds *Cascadia Mono*. Use `Up`/`Down` (or the
mouse) to select, `Enter` to run, and `Esc` to close. Settings are saved to
`%APPDATA%\Penmark\settings.ini`.

* **Font: Change Editor Font** lists every installed font, with each name drawn in
  its own typeface. The document previews the highlighted font live. `Enter` keeps
  it and `Esc` reverts. Code blocks and inline code stay monospace.
* **View: Rendering Options** lists every option with its current value. Type to
  filter, `Left`/`Right` to adjust (`Shift` for bigger steps), and `Enter` to
  toggle. Changes apply and save immediately. The options are:
  * bullet style (dot or dash);
  * a line under each heading level, set separately for H1 to H6;
  * spacing around paragraphs, above and below headings, before and after
    lists, and between list items;
  * list and quote indent, blank-line height, code block padding, table cell
    padding, and horizontal rule spacing;
  * line height and content width;
  * reset to defaults.

To add a command, add a variant to `Command` and an entry to `COMMANDS` in
`src/palette.rs`, then handle it in `App::palette_accept`.

The status bar shows the file name and modified state on the left. On the right it
shows line and column, word count (selected and total), line endings and mode.

## Building

Requires the Rust MSVC toolchain and the Windows SDK. The SDK comes with the Visual
Studio C++ build tools and provides `rc.exe`, which embeds the icon and manifest.

```bash
cargo build --release
```

The executable is `target/release/penmark.exe`. To open a file, pass it as an argument
(`penmark.exe notes.md`), drop it on the window, or use `Ctrl+O`.

## Code layout

| File | Responsibility |
| --- | --- |
| `src/markdown.rs` | Line classification (block roles) and inline parsing (styles plus the delimiter ranges to hide). Pure Rust, unit-tested. |
| `src/doc.rs` | Text model as a list of lines, grouped undo/redo, grapheme and word boundaries. Unit-tested. |
| `src/render.rs` | DirectWrite layout per line with a source↔display offset map, layout caching, painting, hit-testing. |
| `src/app.rs` | Editor state, key and mouse handling, Markdown-aware editing commands, find, files. |
| `src/palette.rs` | Command palette: commands, term matching, filtering and selection. Unit-tested. |
| `src/settings.rs` | Persisted `key=value` settings in `%APPDATA%\Penmark\settings.ini`. |
| `src/options.rs` | Rendering options (bullets, heading lines, spacing): defaults, ranges, persistence. Unit-tested. |
| `src/platform.rs` | Clipboard, file dialogs, message boxes, shell, theme detection. |
| `src/main.rs` | Window class, message loop, message dispatch. |

Every line's layout is cached by its content and state. A keystroke re-lays out only
the lines that changed, which costs about 0.5 ms of CPU on a 20,000-line document.

## Not yet supported

Syntax highlighting inside code blocks, math rendering, footnote rendering, setext
headings, reference-style link resolution, remote images, HTML/PDF export, and
remembering window size and recent files.
