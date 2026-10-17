# Penmark

A small, keyboard-driven, what-you-see-is-what-you-get Markdown editor for Windows.
It is a single native executable (about 700 KB) built on Win32, Direct2D and
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

Shortcuts follow VS Code's defaults where they don't clash with Markdown formatting:
`Ctrl+I` and `Ctrl+K` stay italic and link, as in other Markdown editors, and bold is
`Ctrl+Shift+B` so that `Ctrl+B` can toggle the explorer as in VS Code. The full list
is [`assets/help.md`](assets/help.md), shown rendered by **Help: Keyboard Shortcuts**
in the command palette. Shortcuts can be changed in `keymap.ini` (see
[Command palette](#command-palette)). The highlights:

| | |
| --- | --- |
| `F1` / `Ctrl+Shift+P`, `Ctrl+P`, `Ctrl+R` | Command palette, go to file, recent folders |
| `Ctrl+Shift+F` | Search the folder's Markdown files |
| `Ctrl+B`, `Ctrl+Shift+E`, `Ctrl+0` | Show / hide the explorer, switch focus to / from it, focus it |
| `Ctrl+Shift+O` | Show / hide the outline |
| `Alt+Left/Right`, mouse back/forward | Previous / next file |
| `Ctrl+Shift+B` `Ctrl+I` `Ctrl+E` `Ctrl+K` | Bold, italic, code, link |
| `Ctrl+1`…`6`, `Ctrl+Shift+7/8/9`, `Ctrl+Shift+.` | Headings, lists, tasks, quote |
| `Enter`, `Tab`, `Shift+Tab` | Continue, indent and outdent lists |
| `Alt+C` | Toggle a task's checkbox |
| `Ctrl+F`, `F3`, `Ctrl+G` | Find, find next, go to line (in the status bar) |
| `Ctrl+H` | Replace: `Tab` to the replacement, `Enter` replaces one, `Ctrl+Enter` all |
| `Ctrl+D`, `Ctrl+L` | Select word / next occurrence, select line |
| `Alt+Up/Down`, `Shift+Alt+Up/Down`, `Ctrl+Shift+K` | Move, copy, delete lines |
| `Ctrl+Enter` / `Ctrl+Shift+Enter` | Insert a line below / above |
| `Ctrl+/` | Toggle comment |
| `Ctrl+Shift+V`, `Ctrl+Shift+T`, `Ctrl+=`/`-`/`Numpad0` | Source mode, theme, zoom (5% steps) |
| `Ctrl+W`, `Ctrl+Shift+W`, `Ctrl+Q` | Close file, close window, quit (close every window) |

### Folders

A window can have one folder open, like a VS Code workspace (subfolders included, no
multi-root). Open one with **File: Open Folder…** in the palette, `Ctrl+R`, or
`penmark.exe C:\path\to\folder`. Started without arguments, Penmark reopens the last
folder and the file that was open in it.

* `Ctrl+P` lists the folder's Markdown and text files by name, with their subfolder
  shown on the right. The search matches the name and the folder, so `2024 jan` finds
  `projects/2024/jan.md`.
* `Ctrl+Shift+F` searches the text of the folder's Markdown files (not `.txt` or
  other files) and lists each matching line with its file and line number. The search
  is case-insensitive and starts with the selected text; the open file is searched as
  edited, even unsaved. The search is a panel across the top third of the editor:
  `Up` and `Down` show each result in the editor below it, centered, without opening
  it. `Enter` opens the file with the match selected, and `F3` then goes on to the
  next match in it. `Escape` clears the search, and again closes it, going back to
  where you were.
* `Ctrl+R` lists recent folders, with **Open Folder…** first.
* The **explorer** (`Ctrl+B` to show or hide, `Ctrl+Shift+E` to switch focus to or
  from it, `Ctrl+0` to focus it) shows the folder tree, skipping hidden folders such
  as `.git` and `node_modules`. Click or
  press `Enter` to open a file; non-text files open in their usual app. `Delete` moves
  to the Recycle Bin after a confirmation. `F2` renames, and a name like
  `archive/old.md` moves the file. `Ctrl+X` / `Ctrl+V` or drag and drop move files
  and folders. Right-click for New File, New Folder, Rename, Delete and Reveal in File
  Explorer. If the open file is moved or renamed, the editor follows it, and when other
  notes link to a moved note (`[[name]]` or relative `.md` links) Penmark offers to update
  those links and the relative links inside it. With the
  explorer focused, `A` starts a new file and `Shift+A` a new folder; the header has
  buttons for both, and a funnel that shows only Markdown files (hiding `data`
  folders and everything else).
* **Preferences: Folder Settings** in the palette holds per-folder settings: autosave
  on or off, and the delay in seconds after the last edit. With autosave on, switching
  files, closing the window, and signing out save instead of asking. Folder settings
  live in Penmark's settings file, not in the folder.
* `Alt+D` opens today's daily note, `YYYY-MM-DD.md` in the daily notes folder
  (`journal` unless changed in Folder Settings), and makes it if needed: from the
  template note set there, with `{{date}}` and `{{title}}` replaced by the date, or
  with a `# YYYY-MM-DD` heading. **File: Previous Daily Note** and **File: Next Daily
  Note** step through the existing ones (unbound; give them keys in `keymap.ini`).
* The window title is the folder's name. A bar above the document shows the open
  file's name, a dot when it has unsaved changes, and its subfolder.
* `Alt+Left` / `Alt+Right` and the mouse back / forward buttons move through the files
  opened in the window, returning to where you were in each.

When the open file changes on disk (a sync, `git pull`, another editor), Penmark
reloads it when the window gets focus, keeping the caret and scroll; the reload is one
undo step. With unsaved changes it asks first. Saving over a file that changed since it
was read asks before overwriting it, and autosave pauses instead.

The zoom level and the window's size, position and maximized state are remembered
between runs. A saved position on a monitor that is no longer connected is ignored.

### Links, attachments and the outline

* Rendered links are blue on a light blue background and open with a click (the hand
  cursor shows when a click will open one). On the line being edited, a click places
  the cursor and `Ctrl+Click` opens the link.
* Typing `[[` in a folder lists its notes; pick one to complete the link. A
  `[[name]]` link is looked up next to the note, then from the folder root, then by
  file name anywhere in the folder. A link to a note that doesn't exist is drawn grey
  and underlined; clicking it starts that note.
* With a folder open, dropping files on a note copies them into a `data` folder next
  to it and links them with a relative path, at the drop caret that follows the
  mouse. Images are linked as images on a line of their own, so they render. Files
  already inside the folder are linked where they are. `Ctrl+V` does the same for
  files copied in File Explorer, and saves a copied image as
  `data/pasted-<date-time>.png`. Without a folder, dropping a file opens it.
* Right-click a code block or inline code to copy it; `Ctrl+C` with nothing selected
  does the same while the cursor is in code.
* `Ctrl+Shift+O` shows the document's headings in a panel on the right. The heading
  the cursor is under is highlighted, and clicking one jumps to it. Below the
  headings, **Backlinks** lists the folder's other notes that link to this one
  (`[[wiki]]` or relative `.md` links, not those in code) with the linking line;
  click one to open it there.

### Command links

A link whose target starts with `run:` launches a program, and `shell:` runs its text
with `cmd /c`:

```markdown
[run my script](<run:%USERPROFILE%\venv\Scripts\python.exe my_script.py> "cwd=%USERPROFILE%\proj")
[build & log](<shell:cd /d %USERPROFILE%\proj && make > build.log 2>&1> "wait")
```

`run:` expands `%VARS%` itself and resolves a relative program against the working
folder; `shell:` leaves the text to `cmd`. Options go in the title as `key=value`
pairs separated by `;`: `cwd=` (default: the note's folder), `window=hidden|show|keep`
(default `show` for `run:`, `hidden` for `shell:`), `wait` (report the exit code) or
`detach` (the default), and `confirm` (always ask).

The first click shows the full command and asks before running it. Penmark then
stores a hash of the command and folder as trusted, so a link that changes asks again.
Errors and, with `wait`, non-zero exit codes show in the status bar. Command links are
drawn as black text on light green with a ▶ (`run:`) or `$` (`shell:`) glyph, and
hovering one shows the command as it will run.

CommonMark does not allow `>` inside a `<...>` link target, but commands often need
it: in a command link the target ends at the first `>` that is followed by an
optional title and `)`. Other Markdown tools will not render such a link.

### Slash menu

Type `/` at the start of a line or after a space to open a small menu at the cursor.
Keep typing to filter it (`/co`, `/h2`). `Up`/`Down` select, `Enter` or `Tab`
inserts, and `Esc` closes it and leaves what you typed. The typed `/command` is
replaced, and one `Ctrl+Z` undoes the whole insertion.

* **Code block** inserts a fenced code block with the cursor inside it. On a line
  that already has text, the block goes below it, inside the same list item or
  quote.
* **Table** inserts a table with a header row and one empty row, with the first
  header selected so you can type over it.
* **Task** turns the line into a task.
* **Heading 1** to **Heading 5** turn the current line into that heading.
* **Horizontal rule** inserts `---` (after a blank line when under a paragraph, so it
  is not read as a heading underline).
* **Today**, **Yesterday** and **Tomorrow** insert the date as `YYYY-MM-DD`.
* **Run command**, **Shell command** and **Start command** insert command link
  templates.

A `/` inside a word (`a/b`, URLs) or inside a code block does not open the menu.
Items are defined in `src/slash.rs`.

### Tables

With the cursor in a table, the table stays rendered and you edit it cell by cell.
The cell with the cursor is outlined.

| Key | In a table |
| --- | --- |
| `Tab` / `Shift+Tab` | Next / previous cell, selecting its text; `Tab` in the last cell adds a row |
| `Up` / `Down` | Same column in the row above / below; past the edge, leave the table |
| `Left` / `Right` | Move within the cell, continuing into the next cell at its edge |
| `Home` / `End` | Start / end of the cell |
| `Enter` | Next row (adds one at the bottom) |
| `Ctrl+Enter` | Insert a row below |
| `Shift+Up` / `Shift+Down` | Select cells down the column; `Delete` empties them, `Ctrl+C` copies them |

Typing `|` in a cell stores it as `\|`, and pasted text is kept to one line.

With the mouse near the table, a **+** strip on the right adds a column and one along
the bottom adds a row. A handle above the hovered column, or a right-click on any cell,
opens a menu to align the column left, center or right, insert or delete rows and
columns, or delete the table. The same actions are in the command palette as
`Table: …`. Structural changes rewrite the table's Markdown with padded columns, as
one undo step.

To work with a table as Markdown text, select from outside it into it (for example,
drag from the line above): the whole table then shows its source, so you can copy it,
or delete it with the surrounding text.

### Command palette

`Ctrl+Shift+P` opens a searchable list of commands. The search is split into
space-separated terms, and an item matches when it contains every term in any
order. For example, `mono cas` finds *Cascadia Mono*. Use `Up`/`Down` (or the
mouse) to select, `Enter` to run, and `Esc` to close. Settings are saved to
`%APPDATA%\Penmark\settings.ini`.

* **Font: Change Editor Font** lists every installed font, with each name drawn in
  its own typeface. The document previews the highlighted font live. `Enter` keeps
  it and `Esc` reverts. Code blocks and inline code stay monospace.
* **Font: Change Code Font** does the same for code blocks and inline code, listing
  monospaced fonts first.
* **Font: Code Font Size** and **Preferences: UI Font Size** open the rendering
  options at that setting: code text relative to the text around it, and the size of
  the status bar, explorer, outline, palette and menus.
* **View: Rendering Options** lists every option with its current value. Type to
  filter, `Left`/`Right` to adjust (`Shift` for bigger steps), and `Enter` to
  toggle. Changes apply and save immediately. The options are:
  * bullet style (dot or dash);
  * always show the `#` markers on headings (dimmed, in the margin), not only
    while editing the heading;
  * a line under each heading level, set separately for H1 to H6;
  * spacing around paragraphs, above and below headings, before and after
    lists, and between list items;
  * list and quote indent, blank-line height, code block padding, table cell
    padding, and horizontal rule spacing;
  * line height, content width, code font size and UI font size;
  * reset to defaults.

* **Preferences: Open Settings Folder** opens `%APPDATA%\Penmark` in Explorer.
* **Preferences: Write keymap.ini** writes `%APPDATA%\Penmark\keymap.ini` with every
  command and its current keys, then opens it in the app for `.ini` files. Each line
  is `command = keys`, such as `redo = Ctrl+Y, Ctrl+Shift+Z`; an empty value removes
  the shortcut, and commands left out keep their defaults. Palette commands without
  a default shortcut (fonts, folder settings, table actions, …) are listed with an
  empty value, ready to be given one. A key given to a command
  in the file is taken from any command that has it by default. The file is reloaded
  when Penmark gets focus (or when it is saved in Penmark), and problems in it show
  in the status bar. Caret movement, Enter, Tab, Backspace, Delete, and keys inside
  the palette, explorer and tables stay fixed. Writing it again keeps changed keys
  but not comments, so Penmark asks first.

To add a command, add a variant to `Command` and an entry to `COMMANDS` in
`src/palette.rs`, then handle it in `App::palette_accept`. To give it a shortcut, add
an `Action` and its default keys to `ACTIONS` in `src/keymap.rs`, then handle it in
`App::run_action`.

The status bar shows the file name and modified state on the left. On the right it
shows line and column, word count (selected and total), line endings and mode.

## Building

Requires the Rust MSVC toolchain and the Windows SDK. The SDK comes with the Visual
Studio C++ build tools and provides `rc.exe`, which embeds the icon and manifest.

```bash
cargo build --release
```

The executable is `target/release/penmark.exe`. To open a file or folder, pass it as an
argument (`penmark.exe notes.md`, `penmark.exe C:\notes`), drop a file on the window, or
use `Ctrl+O` / `Ctrl+R`.

## Code layout

| File | Responsibility |
| --- | --- |
| `src/markdown.rs` | Line classification (block roles) and inline parsing (styles plus the delimiter ranges to hide). Pure Rust, unit-tested. |
| `src/doc.rs` | Text model as a list of lines, grouped undo/redo, grapheme and word boundaries. Unit-tested. |
| `src/render.rs` | DirectWrite layout per line with a source↔display offset map, layout caching, painting, hit-testing. |
| `src/app.rs` | Editor state, key and mouse handling, Markdown-aware editing commands, find, files. |
| `src/app/folder.rs` | Open folder, quick open, explorer panel and its file operations, folder settings, autosave. |
| `src/workspace.rs` | Folder scanning, tree visibility, recent folders, folder settings model. Unit-tested. |
| `src/palette.rs` | Command palette: commands, term matching, filtering and selection. Unit-tested. |
| `src/settings.rs` | Persisted `key=value` settings in `%APPDATA%\Penmark\settings.ini`. |
| `src/keymap.rs` | Keyboard shortcuts: commands, default keys, parsing and writing `keymap.ini`. Unit-tested. |
| `src/table.rs` | Tables: cell lookup, row/column/alignment changes, serialization. Unit-tested. |
| `src/slash.rs` | The `/` menu: items, filtering, and the edits they make. Unit-tested. |
| `src/attach.rs` | Dropped and pasted files: copying into `data`, relative links. Unit-tested. |
| `src/command.rs` | `run:` / `shell:` links: options, command lines, trust keys. Unit-tested. |
| `src/dragdrop.rs` | OLE drop target, for the drop caret while files are dragged. |
| `src/options.rs` | Rendering options (bullets, heading lines, spacing): defaults, ranges, persistence. Unit-tested. |
| `src/platform.rs` | Clipboard (text, files, images), PNG encoding, file dialogs, message boxes, processes, shell, theme detection. |
| `src/main.rs` | Window class, message loop, message dispatch. |

Every line's layout is cached by its content and state. A keystroke re-lays out only
the lines that changed, which costs about 0.5 ms of CPU on a 20,000-line document.

## Not yet supported

Syntax highlighting inside code blocks, math rendering, footnote rendering, setext
headings, reference-style link resolution, remote images, HTML/PDF export, and
search across all files in a folder.
