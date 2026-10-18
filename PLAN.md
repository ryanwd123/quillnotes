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

## Plan file note

The committed `PLAN.md` had a "Color theme" item (`write-theme.ini`). The working-tree copy is empty, so that item is removed in the diff. This file is left uncommitted; decide whether to keep or restore that item before committing.
