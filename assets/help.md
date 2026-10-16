# Penmark

Penmark edits plain Markdown files. The lines you are editing show their Markdown syntax; everything else is rendered. Nothing is converted: the file on disk is exactly what you typed.

## File

| Shortcut | Action |
| --- | --- |
| `Ctrl+N` | New document |
| `Ctrl+Shift+N` | New window |
| `Ctrl+O` | Open (or drop a file on the window) |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save as |
| `Ctrl+W` | Close window |

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
| `Ctrl+Enter` | Toggle task done |
| `Tab` / `Shift+Tab` | Indent / outdent |

Typing `*`, `_`, `` ` ``, `~` or `=` with text selected wraps the selection.

Type `/` at the start of a line or after a space for the slash menu: **Code block** and **Heading 1**–**5**. Keep typing to filter it, `Enter` inserts, `Esc` closes.

## Editing

| Shortcut | Action |
| --- | --- |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Ctrl+D` | Duplicate line |
| `Ctrl+Shift+K` | Delete line |
| `Alt+Up` / `Alt+Down` | Move line |
| `Ctrl+C` / `Ctrl+X` | Copy / cut (the whole line when nothing is selected) |
| `Enter` | New line, continuing lists and quotes |
| `Shift+Enter` | New line without continuing the list |

## Navigation and view

| Shortcut | Action |
| --- | --- |
| `Ctrl+Shift+P` | Command palette: *Font: Change Editor Font*, *View: Rendering Options* (←/→ adjust, Enter toggles) |
| `Ctrl+F` | Find (`Enter`/`F3` next, `Shift+Enter`/`Shift+F3` previous) |
| `Ctrl+G` | Go to line |
| `Ctrl+Click` | Open link (`[[Note]]` links open `Note.md` next to this file) |
| `Ctrl+/` | Toggle source mode |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | Zoom in / out (5% steps) / reset; the zoom is remembered |
| `Ctrl+Shift+T` | Light / dark theme |
| `F1` | This help |
