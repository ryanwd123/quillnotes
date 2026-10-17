# Penmark

Penmark edits plain Markdown files. The lines you are editing show their Markdown syntax; everything else is rendered. Nothing is converted: the file on disk is exactly what you typed.

Shortcuts follow VS Code where they don't clash with Markdown formatting (`Ctrl+I` and `Ctrl+K` stay italic and link; bold is `Ctrl+Shift+B` so `Ctrl+B` can toggle the explorer).

## Files and folders

| Shortcut | Action |
| --- | --- |
| `Ctrl+P` | Go to a file in the open folder (type part of its name or folder) |
| `Ctrl+Shift+2`, or `@` first in `Ctrl+P` | Go to a heading in this note (`Esc` goes back) |
| `Ctrl+Shift+F` | Search the text of the folder's Markdown files; `Up`/`Down` preview a result, `Enter` opens it, `Escape` clears then closes |
| `Ctrl+R` | Open a recent folder, or **Open Folder…** |
| `Ctrl+B` | Show / hide the explorer |
| `Ctrl+Shift+E` | Move focus between the explorer and the editor |
| `Ctrl+0` | Focus the explorer (`Esc` returns to the editor) |
| `Alt+Left` / `Alt+Right`, mouse back / forward | Previous / next file, where you left it (kept when the folder is closed and opened again) |
| `Ctrl+N` | New document |
| `Ctrl+O` | Open a file (or, without a folder, drop one on the window) |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save as |
| `Ctrl+W` | Close the file |
| `Ctrl+Shift+N` / `Ctrl+Shift+W` | New window / close window |
| `Ctrl+Q` | Quit: close every Penmark window |
| `Alt+D` | Today's daily note (`journal/YYYY-MM-DD.md`, made if needed); **File: Previous / Next Daily Note** in the palette step through them |

When the open file changes on disk (a sync, `git pull`, another editor), Penmark reloads it when you come back to the window, keeping your place; `Ctrl+Z` undoes the reload. With unsaved changes it asks first, and saving over a newer file asks before overwriting it.

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

When a renamed or moved note has links to it (`[[name]]` or relative `.md` links), Penmark offers to update them, along with the relative links inside the moved notes.

**Preferences: Folder Settings** in the command palette sets autosave for the open folder, and the delay in seconds after the last edit. It also sets the daily notes folder and a template note for new daily notes, in which `{{date}}` and `{{title}}` become the date. With autosave on, Penmark also saves before switching files or closing.

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

Type `[[` to link to another note in the folder: pick it from the list as you type. A link to a note that doesn't exist yet is grey and underlined; click it to make the note. Type `#` after the name (or press it while a note is highlighted) to pick one of its headings: `[[note#Heading]]` opens the note at that heading, and `[[#Heading]]` links within this note.

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

Code blocks are syntax highlighted when the fence names the language (```` ```rust ````, `python`, `js`, `ts`, `c`, `cs`, `go`, `sh`, `ps1`, `json`, `toml`, `yaml`, `sql`, `html`, …).

Misspelled words get a red wavy underline (Windows' spell checker, in your language). Right-click one for suggestions, **Add to Dictionary** or **Ignore**. Code, links' targets, URLs, `[[note names]]` and words with digits or paths are not checked. **View: Toggle Spell Check** turns it off.

## Navigation and view

| Shortcut | Action |
| --- | --- |
| `F1` or `Ctrl+Shift+P` | Command palette (fonts, rendering options, folder settings, table actions, …) |
| `Ctrl+F` | Find (`Enter`/`F3` next, `Shift+Enter`/`Shift+F3` previous; click the document to edit with find left open, `Ctrl+F` or a status bar click to return, `Esc` to close) |
| `Ctrl+H` | Replace: `Tab` switches between find and replace, `Enter` in the replacement replaces the match and goes to the next, `Ctrl+Enter` replaces all (one undo step) |
| `Ctrl+G` | Go to line |
| Click a link | Open it (`Ctrl+Click` on the line you are editing; `[[Note]]` opens the note) |
| `Ctrl+Shift+O` | Show / hide the outline of headings, with the notes that link to this one (backlinks) below it |
| `Ctrl+Shift+[` / `Ctrl+Shift+]` | Fold / unfold the section under the heading the caret is in (click the `⋯` to unfold; **View: Unfold All Headings** in the palette) |
| `Ctrl+Shift+V` | Toggle source mode |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+Numpad0` | Zoom in / out (5% steps) / reset; the zoom is remembered |
| `Ctrl+Shift+T` | Light / dark theme |

The palette also sets the **UI font size**, the **code font** and the **code font size**.

## Changing shortcuts

Run **Preferences: Write keymap.ini** from the palette. It writes `%APPDATA%\Penmark\keymap.ini` with every command and its keys and opens it. Edit a line such as `bold = Ctrl+Shift+B` (several keys are separated by commas; an empty value removes the shortcut), or give keys to one of the palette commands listed without any, save, and switch back to Penmark. Problems in the file show in the status bar. **Preferences: Open Settings Folder** opens the folder; delete `keymap.ini` to go back to the defaults.

## Changing colors

Run **Preferences: Write theme.ini** from the palette. It writes `%APPDATA%\Penmark\theme.ini` with every color of the light and dark themes and opens it. Change a line such as `bg = #FFF8E7` under `[light]` or `[dark]` (`#AARRGGBB` adds transparency), save, and switch back to Penmark. Problems show in the status bar; delete `theme.ini` to go back to the default colors.

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
