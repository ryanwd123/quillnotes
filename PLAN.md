# Pre-release bug review

Found by reading the code (no changes made) and running `cargo test` (73 passed) and `cargo clippy --all-targets`. I could not drive the Win32 window to check layout or visual behavior, so the UI items below come from the code, not from running the app.

## Data loss

- **Non-UTF-8 files are saved with replacement characters.** Opening a file that is not valid UTF-8 shows "invalid bytes replaced" in the status line, then Save writes the lossy text back over the original bytes (`src/app.rs` `read_path` ~4716, `write_to` ~4757). The bytes are gone with no warning at save time. Options: refuse to save a lossy doc without confirmation, or keep the raw bytes and only replace them after an edit.
- **Settings writes are not atomic.** `Settings::save` (`src/settings.rs:61`) calls `std::fs::write` directly, which truncates first. A crash or power loss mid-write leaves an empty `settings.ini`. Write to a temp file and rename.
- **Settings write errors are silently dropped.** `Settings::update` results are discarded with `let _ =` in many places (`src/app/folder.rs:62, 89, 99, 202, 599, 1150`; `src/app.rs:781, 1193, 4473, 5117`). If the folder is read-only, recent files, window placement and toggles just stop persisting with no message.

## Windows-specific

- **Reserved file names are not caught in Save As.** The suggested name (`src/app.rs` `save_as` ~4796) strips `\/:*?"<>|` but not Windows reserved names (`CON`, `NUL`, `COM1`, `LPT1`, ...) or trailing dots and spaces. A first heading of "CON" suggests `CON.md`, which Windows cannot create.

## Shutdown and close

- **Shutdown can block on a message box.** `WM_QUERYENDSESSION` calls `confirm_discard()`, which can show a dialog (`src/main.rs:192`). Windows gives the app a short time to answer during sign-out, so an unanswered prompt can be cut off or the session ends without a clear question. Consider a non-blocking prompt or skipping the dialog and using autosave.

## Code quality (fix before release, per CLAUDE.md)

- **Clippy warnings in test code.**
  - `src/options.rs:293`: field assigned after `Default::default()`. Use a struct literal or `..Default::default()`.
  - `src/spell.rs:141`: single-element `[0..3]` array in an assertion. Compare with a `Vec` or a plain range.
- **Dead variable in the list-continuation code.** `src/app.rs:1714-1716`: `let p = self.doc.replace(...)` followed by `let _ = p;`. The return value is not needed; drop the binding.
- **Inconsistent string formatting.** The dialogs in `confirm_overwrite`, `check_disk`, and `reload_text` (`src/app.rs` ~4845-4930) put literal line breaks inside string literals, while `write_keymap`/`write_theme` use `\n\n` escapes. Use one style.
- **Unchecked `unwrap()` on paths.** `check_disk` (`src/app.rs` ~4885) calls `self.doc.path.clone().unwrap()`. It is safe only because `disk_differs()` returns early when there is no path. A `let Some(path) = ... else { return }` would make that explicit.

## Things to confirm by running the app

- **Unsaved-change guard on every file-open path.** `read_path` replaces the document without checking for unsaved changes. The callers I read (`open_path` from the explorer, quick open, backlinks, back/forward, drag-drop, `open_at`, `open_daily`) do call `confirm_discard()` first, but I did not check every one. Worth a manual pass: edit a file, then open another from each entry point and confirm you get the prompt.
- **DPI change.** `WM_DPICHANGED` (`src/main.rs:80`) skips the suggested rectangle when `restoring_placement` is set. Check moving the window between monitors with different scaling, both on restore and during a normal drag.
- **Modal dialogs.** While a message box is open, `IN_MODAL` makes most messages go to `DefWindowProc` (`src/main.rs:63`). Check that the window repaints and does not appear frozen after closing a dialog from a keyboard shortcut.
- **Save As from an untitled document with a heading.** The suggested name strips leading `#`, `>`, `-` and spaces but not `*`, `1.` or `[ ]`, so a heading like `1. Ideas` suggests `1. Ideas.md`. Check and decide the rule.

## UI and Markdown rendering review (Live 2)

This one was done by driving the built app's Win32 window (screenshots plus real
keystrokes and posted mouse/key messages) on two documents that exercise every
supported syntax, in light and dark themes. Unlike the review above, these items
come from the running UI. Screenshots are in `%TEMP%\opencode\penmark_ui`.

### Bug: a stray colored dot under a folded heading

Folding a heading whose section contains a blockquote or callout leaves a small
square of the quote's color at the left margin, at the fold point. It is the
"Important" purple (`#8250DF`, confirmed from the pixel) when the hidden callout
is `> [!IMPORTANT]`, and grey for a plain quote. It reproduces in both test
documents and disappears when the section is unfolded.

Cause: the decorations are built in a second pass over `self.rows` in
`View::layout` that does not skip rows a fold marked hidden (`src/render.rs`
~2031-2077). A fully hidden quote run therefore has `y0 == y1`, and the bar is
painted as `rect(x, y0 + 2, x + 3z, y1 - 2)` (`src/render.rs:2406`) — a
rectangle whose top is below its bottom, which Direct2D still fills, leaving the
dot. The same pass also emits a zero-height `Decor::Block` for a hidden code
block. Fix direction: skip `row.hidden` rows when collecting `decor`, or drop
runs where `y1 <= y0`.

### Cosmetic: ordered-list numbers are dimmer than bullets

A rendered ordered-list number is drawn with `Ink::Dim` (`src/render.rs:2706`)
but an unordered bullet with `Ink::Text` (`src/render.rs:2712`), so `1.` is grey
while `•` is black at the same level. Seen in both themes.

### Limitation: four-space indented code becomes an indented paragraph

Lines indented by four spaces are not treated as a code block: they render as
paragraph text with the leading spaces shown and no code background. This is in
neither the README's supported list nor its "Not yet supported" list.

### Observation: missing `[[wiki]]` links are only marked with a folder open

`App::update_missing_links` (`src/app.rs:594`) only runs `if let Some(root) =
self.folder`. Opened as a single file, every `[[link]]` renders as a valid blue
link, so a missing note looks the same as a good one. With a folder open the
grey underline appears as documented.

### Observation: the table delimiter row shows the whole table as source

With the caret on the `| --- |` row (reachable with Go to Line), `grid_block`
returns 0 because `table::row_of_line` has no row for the delimiter, so the
table renders as Markdown. Header and data cells render and edit correctly, with
the caret's cell outlined.

### Verified rendering (no issues found)

Headings H1-H6 and heading wrapping; YAML front matter; paragraphs and wrapping
(the continuation aligns to the text column and list items hang under their
text); nested bullets and numbers (markers stay right-aligned on their dot);
task lists; blockquotes and nesting; callouts NOTE, TIP, WARNING, IMPORTANT and
an unknown type; fenced code with highlighting (Rust, Python, JSON and an
unknown fence) and long-line wrapping; tables (left/center/right alignment,
inline code, escaped `\|`, empty cells, bold/italic/link cells); rules `---`,
`***`, `___`; single- and multi-line HTML comments; escapes and entities; links,
autolinks, bare URLs and `[[wiki]]` links; very long unbroken words and URLs
wrap; unicode, emoji and RTL text; inline bold, italic, strikethrough, highlight,
code and nesting.

Live 2 specifics behaved as described: an edited line keeps its rendered height
(heading, blank line, rule, code fence — the fence source spills above the
block) and the text column does not shift (the prefix hangs into the margin); an
image under the caret stays an image with a bold caret before or after it; a
table under the caret stays rendered with the cell outlined.

Window chrome also rendered correctly: breadcrumb, status bar, outline panel
(with the caret's heading highlighted), the folder explorer, the command palette
and the dark theme.

## Plan file note

The committed `PLAN.md` had a "Color theme" item (`write-theme.ini`). The working-tree copy is empty, so that item is removed in the diff. This file is left uncommitted; decide whether to keep or restore that item before committing.
