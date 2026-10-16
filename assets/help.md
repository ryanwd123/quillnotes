# Penmark

Penmark edits plain Markdown files. The lines you are editing show their Markdown syntax; everything else is rendered. Nothing is converted: the file on disk is exactly what you typed.

Shortcuts follow VS Code where they don't clash with Markdown formatting (`Ctrl+I` and `Ctrl+K` stay italic and link; bold is `Ctrl+Shift+B` so `Ctrl+B` can toggle the explorer).

## Files and folders

| Shortcut | Action |
| --- | --- |
| `Ctrl+P` | Go to a file in the open folder (type part of its name or folder) |
| `Ctrl+Shift+F` | Search the text of the folder's Markdown files; `Enter` opens a result |
| `Ctrl+R` | Open a recent folder, or **Open Folder…** |
| `Ctrl+B` | Show / hide the explorer |
| `Ctrl+Shift+E` | Move focus between the explorer and the editor |
| `Ctrl+0` | Focus the explorer (`Esc` returns to the editor) |
| `Alt+Left` / `Alt+Right`, mouse back / forward | Previous / next file, where you left it |
| `Ctrl+N` | New document |
| `Ctrl+O` | Open a file (or, without a folder, drop one on the window) |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save as |
| `Ctrl+W` | Close the file |
| `Ctrl+Shift+N` / `Ctrl+Shift+W` | New window / close window |

## Explorer

| Key | Action |
| --- | --- |
| `Up` / `Down`, `Left` / `Right` | Move, collapse / expand folders |
| `Enter` or click | Open the file, or open / close the folder |
| `A` / `Shift+A` | New file / new folder |
| `F2` | Rename (type `folder/name.md` to move it) |
| `Delete` | Move to the Recycle Bin |
| `Ctrl+X` then `Ctrl+V` | Move into the selected folder |
| Drag and drop | Move onto a folder (or below the files: the top folder) |
| Right-click | New file, new folder, rename, delete, reveal in File Explorer |

The buttons at the top of the explorer make a new file or folder, and the funnel shows only Markdown files (hiding `data` folders and other files).

**Preferences: Folder Settings** in the command palette sets autosave for the open folder, and the delay in seconds after the last edit. With autosave on, Penmark also saves before switching files or closing.

## Attachments

With a folder open, drop files on a note to copy them into a `data` folder next to it and link them where the drop caret shows. Images are linked as images on a line of their own. `Ctrl+V` does the same for files copied in File Explorer, and saves a copied image (a screenshot, say) as `data/pasted-….png`.

## Formatting

| Shortcut | Action |
| --- | --- |
| `Ctrl+Shift+B` / `Ctrl+I` | **Bold** / *italic* |
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

Type `/` at the start of a line or after a space for the slash menu: **Code block**, **Table**, **Task**, **Heading 1**–**5**, **Horizontal rule**, **Today** / **Yesterday** / **Tomorrow**, and **Run**, **Shell** and **Start command** links. Keep typing to filter it, `Enter` inserts, `Esc` closes.

Type `[[` to link to another note in the folder: pick it from the list as you type.

## Tables

| Shortcut | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Next / previous cell (adds a row at the end) |
| `Up` / `Down` | Cell above / below |
| `Shift+Up` / `Shift+Down` | Select cells down the column (`Delete` empties them) |
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
| `Ctrl+C` / `Ctrl+X` | Copy / cut (the whole line when nothing is selected; in code, `Ctrl+C` copies the code block or inline code) |
| Right-click code | Copy the code block or inline code |
| `Enter` | New line, continuing lists and quotes |
| `Shift+Enter` | New line without continuing the list |

## Navigation and view

| Shortcut | Action |
| --- | --- |
| `F1` or `Ctrl+Shift+P` | Command palette (fonts, rendering options, folder settings, table actions, …) |
| `Ctrl+F` | Find (`Enter`/`F3` next, `Shift+Enter`/`Shift+F3` previous) |
| `Ctrl+G` | Go to line |
| Click a link | Open it (`Ctrl+Click` on the line you are editing; `[[Note]]` opens the note) |
| `Ctrl+Shift+O` | Show / hide the outline of headings |
| `Ctrl+Shift+V` | Toggle source mode |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+Numpad0` | Zoom in / out (5% steps) / reset; the zoom is remembered |
| `Ctrl+Shift+T` | Light / dark theme |

The palette also sets the **UI font size**, the **code font** and the **code font size**.

## Changing shortcuts

Run **Preferences: Write keymap.ini** from the palette. It writes `%APPDATA%\Penmark\keymap.ini` with every command and its keys and opens it. Edit a line such as `bold = Ctrl+Shift+B` (several keys are separated by commas; an empty value removes the shortcut), save, and switch back to Penmark. Problems in the file show in the status bar. **Preferences: Open Settings Folder** opens the folder; delete `keymap.ini` to go back to the defaults.

## Command links

A link whose target starts with `run:` runs a program; `shell:` runs the text with `cmd`. Wrap the target in `<...>` so it can hold spaces. Options go in the title, separated by `;`:

```markdown
[Run script](<run:%USERPROFILE%\venv\Scripts\python.exe my_script.py> "cwd=%USERPROFILE%\proj; window=keep")
[Build](<shell:make > build.log 2>&1> "wait")
[Open](<shell:start "" "%USERPROFILE%\report.pdf">)
```

| Option | Meaning |
| --- | --- |
| `cwd=` | Working folder (default: the note's folder) |
| `window=hidden`, `show`, `keep` | No console, a console, or one that stays open (default: `show` for `run:`, `hidden` for `shell:`) |
| `wait` / `detach` | Report the exit code when it ends / don't (default) |
| `confirm` | Always ask before running |

Penmark shows the full command and asks before running it the first time, and again whenever the link changes. Hover a command link to see the command it will run.

This page: **Help: Keyboard Shortcuts** in the command palette.
