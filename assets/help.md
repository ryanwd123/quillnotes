# Penmark

Penmark edits plain Markdown files. The lines you are editing show their Markdown syntax; everything else is rendered. Nothing is converted: the file on disk is exactly what you typed.

Shortcuts follow VS Code where they don't clash with Markdown formatting (`Ctrl+B`, `Ctrl+I` and `Ctrl+K` stay bold, italic and link).

## Files and folders

| Shortcut | Action |
| --- | --- |
| `Ctrl+P` | Go to a file in the open folder (type part of its name or folder) |
| `Ctrl+R` | Open a recent folder, or **Open Folder…** |
| `Ctrl+Shift+E` | Show / hide the explorer |
| `Ctrl+0` | Focus the explorer (`Esc` returns to the editor) |
| `Ctrl+N` | New document |
| `Ctrl+O` | Open a file (or drop one on the window) |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save as |
| `Ctrl+W` | Close the file |
| `Ctrl+Shift+N` / `Ctrl+Shift+W` | New window / close window |

## Explorer

| Key | Action |
| --- | --- |
| `Up` / `Down`, `Left` / `Right` | Move, collapse / expand folders |
| `Enter` or click | Open the file, or open / close the folder |
| `F2` | Rename (type `folder/name.md` to move it) |
| `Delete` | Move to the Recycle Bin |
| `Ctrl+X` then `Ctrl+V` | Move into the selected folder |
| Drag and drop | Move onto a folder (or below the files: the top folder) |
| Right-click | New file, new folder, rename, delete, reveal in File Explorer |

**Preferences: Folder Settings** in the command palette sets autosave for the open folder, and the delay in seconds after the last edit.

## Formatting

| Shortcut | Action |
| --- | --- |
| `Ctrl+B` / `Ctrl+I` | **Bold** / *italic* |
| `Ctrl+E` | `Inline code` |
| `Ctrl+Shift+X` | ~~Strikethrough~~ |
| `Ctrl+Shift+H` | ==Highlight== |
| `Ctrl+K` | Link (paste a URL over a selection to link it) |
| `Ctrl+1` … `Ctrl+6` | Heading level (press again to clear) |
| `Ctrl+Shift+8` / `7` / `9` | Bullet / numbered / task list |
| `Ctrl+Shift+.` | Quote |
| `Alt+C` | Toggle task done |
| `Ctrl+/` | Toggle comment (`<!-- -->`) |
| `Tab` / `Shift+Tab`, `Ctrl+]` / `Ctrl+[` | Indent / outdent |

Typing `*`, `_`, `` ` ``, `~` or `=` with text selected wraps the selection.

Type `/` at the start of a line or after a space for the slash menu: **Code block**, **Table** and **Heading 1**–**5**. Keep typing to filter it, `Enter` inserts, `Esc` closes.

## Tables

| Shortcut | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Next / previous cell (adds a row at the end) |
| `Up` / `Down` | Cell above / below |
| `Enter` / `Ctrl+Enter` | Next row / insert a row below |
| Right-click a cell | Align column, insert or delete rows and columns, delete table |

Hover a table to get **+** buttons for a new row or column. Select from outside a table into it to see and edit its Markdown.

## Editing

| Shortcut | Action |
| --- | --- |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+D` | Select the word, then the next occurrence |
| `Ctrl+L` | Select the line (again to extend) |
| `Ctrl+Shift+K` | Delete line |
| `Alt+Up` / `Alt+Down` | Move line |
| `Shift+Alt+Up` / `Shift+Alt+Down` | Copy line up / down |
| `Ctrl+Enter` / `Ctrl+Shift+Enter` | Insert a line below / above |
| `Ctrl+C` / `Ctrl+X` | Copy / cut (the whole line when nothing is selected) |
| `Enter` | New line, continuing lists and quotes |
| `Shift+Enter` | New line without continuing the list |

## Navigation and view

| Shortcut | Action |
| --- | --- |
| `F1` or `Ctrl+Shift+P` | Command palette (fonts, rendering options, folder settings, table actions, …) |
| `Ctrl+F` | Find (`Enter`/`F3` next, `Shift+Enter`/`Shift+F3` previous) |
| `Ctrl+G` | Go to line |
| `Ctrl+Click` | Open link (`[[Note]]` links open `Note.md` next to this file) |
| `Ctrl+Shift+V` | Toggle source mode |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+Numpad0` | Zoom in / out (5% steps) / reset; the zoom is remembered |
| `Ctrl+Shift+T` | Light / dark theme |

This page: **Help: Keyboard Shortcuts** in the command palette.
