//! Thin wrappers over Win32 services: clipboard, dialogs, shell, theme.

use std::cell::Cell;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::*;
use windows::Win32::Globalization::{
    GetUserDefaultLocaleName, ISpellChecker, ISpellCheckerFactory, SpellCheckerFactory,
};
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

/// The class of every QuillNotes window.
pub const WINDOW_CLASS: PCWSTR = w!("QuillNotesEditor");

thread_local! {
    /// Set while a modal dialog runs its own message loop.
    pub static IN_MODAL: Cell<bool> = const { Cell::new(false) };
}

/// Test stand-ins: under `cargo test` the clipboard is a thread-local string,
/// modifier keys are what the test sets, and dialogs, menus, the shell and new
/// processes never open, so tests never block on a dialog, touch the user's
/// clipboard or windows, or start programs. Chaos tests also get random answers
/// to message boxes and menus. Outside tests none of this is reached.
pub mod sandbox {
    use std::cell::{Cell, RefCell};

    thread_local! {
        pub static CLIPBOARD: RefCell<Option<String>> = const { RefCell::new(None) };
        /// Held modifiers: 1 Ctrl, 2 Shift, 4 Alt.
        static MODS: Cell<u8> = const { Cell::new(0) };
        /// Random state for answers to dialogs and menus; 0 outside chaos tests.
        static ANSWERS: Cell<u64> = const { Cell::new(0) };
    }

    #[cfg(test)]
    pub fn set_mods(ctrl: bool, shift: bool, alt: bool) {
        MODS.with(|m| m.set(ctrl as u8 | (shift as u8) << 1 | (alt as u8) << 2));
    }

    pub fn mods() -> (bool, bool, bool) {
        let m = MODS.with(|m| m.get());
        (m & 1 != 0, m & 2 != 0, m & 4 != 0)
    }

    #[cfg(test)]
    pub fn answer_randomly(seed: u64) {
        ANSWERS.with(|a| a.set(seed | 1));
    }

    /// A random choice below `n` in a chaos test, else None.
    pub fn pick(n: usize) -> Option<usize> {
        ANSWERS.with(|a| {
            let mut x = a.get();
            if x == 0 || n == 0 {
                return None;
            }
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            a.set(x);
            Some((x % n as u64) as usize)
        })
    }
}

/// Runs a dialog's message loop. Paints are skipped while it is open (the window
/// procedure can't reach the app then), so the window repaints once it closes.
fn modal<T>(hwnd: HWND, f: impl FnOnce() -> T) -> T {
    IN_MODAL.with(|m| m.set(true));
    let r = f();
    IN_MODAL.with(|m| m.set(false));
    if !hwnd.is_invalid() {
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false);
        }
    }
    r
}

pub fn get_clipboard(hwnd: HWND) -> Option<String> {
    if cfg!(test) {
        return sandbox::CLIPBOARD.with(|c| c.borrow().clone());
    }
    unsafe {
        OpenClipboard(Some(hwnd)).ok()?;
        let text = (|| {
            let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let hg = HGLOBAL(h.0);
            let p = GlobalLock(hg) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(hg);
            Some(s)
        })();
        let _ = CloseClipboard();
        text
    }
}

pub fn set_clipboard(hwnd: HWND, text: &str) {
    if cfg!(test) {
        return sandbox::CLIPBOARD.with(|c| *c.borrow_mut() = Some(text.to_string()));
    }
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        put_text(text);
        let _ = CloseClipboard();
    }
}

/// Puts an HTML fragment on the clipboard as "HTML Format" (what rich editors
/// paste), with `text` as the plain text.
pub fn set_clipboard_html(hwnd: HWND, html: &str, text: &str) {
    if cfg!(test) {
        return set_clipboard(hwnd, text);
    }
    unsafe {
        let format = RegisterClipboardFormatW(w!("HTML Format"));
        if format == 0 || OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        put_text(text);
        let mut data = cf_html(html).into_bytes();
        data.push(0);
        put_bytes(format, &data);
        let _ = CloseClipboard();
    }
}

/// The CF_HTML header and document around `fragment`. Its offsets count UTF-8 bytes.
fn cf_html(fragment: &str) -> String {
    const HEADER: &str = "Version:0.9\r\nStartHTML:{0}\r\nEndHTML:{1}\r\n\
                          StartFragment:{2}\r\nEndFragment:{3}\r\n";
    let pre = "<html><body>\r\n<!--StartFragment-->";
    let post = "<!--EndFragment-->\r\n</body></html>";
    // Each offset is written as 10 digits, the width of its `{n}` placeholder plus 7.
    let start_html = HEADER.len() + 4 * 7;
    let start_frag = start_html + pre.len();
    let end_frag = start_frag + fragment.len();
    let end_html = end_frag + post.len();
    let header = HEADER
        .replace("{0}", &format!("{start_html:010}"))
        .replace("{1}", &format!("{end_html:010}"))
        .replace("{2}", &format!("{start_frag:010}"))
        .replace("{3}", &format!("{end_frag:010}"));
    format!("{header}{pre}{fragment}{post}")
}

/// Adds `text` to the open clipboard as Unicode text.
unsafe fn put_text(text: &str) {
    let wide: Vec<u16> = text
        .replace('\n', "\r\n")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
    unsafe { put_bytes(CF_UNICODETEXT.0 as u32, bytes) };
}

/// Adds `data` to the open clipboard in `format`.
unsafe fn put_bytes(format: u32, data: &[u8]) {
    unsafe {
        if let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE, data.len()) {
            let p = GlobalLock(hg) as *mut u8;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
                let _ = GlobalUnlock(hg);
                let _ = SetClipboardData(format, Some(HANDLE(hg.0)));
            }
        }
    }
}

pub fn clipboard_has_text() -> bool {
    if cfg!(test) {
        return sandbox::CLIPBOARD.with(|c| c.borrow().is_some());
    }
    unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok() }
}

/// The paths in a list of dropped or copied files.
pub fn hdrop_paths(hdrop: HDROP) -> Vec<PathBuf> {
    unsafe {
        let n = DragQueryFileW(hdrop, u32::MAX, None);
        (0..n)
            .filter_map(|i| {
                let len = DragQueryFileW(hdrop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                let got = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
                (got > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..got])))
            })
            .collect()
    }
}

/// Files copied in File Explorer.
pub fn clipboard_files(hwnd: HWND) -> Vec<PathBuf> {
    if cfg!(test) {
        return Vec::new();
    }
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_err()
            || OpenClipboard(Some(hwnd)).is_err()
        {
            return Vec::new();
        }
        let files = GetClipboardData(CF_HDROP.0 as u32)
            .map(|h| hdrop_paths(HDROP(h.0)))
            .unwrap_or_default();
        let _ = CloseClipboard();
        files
    }
}

/// An image on the clipboard: PNG bytes (from apps that offer them), or a DIB.
pub enum ClipImage {
    Png(Vec<u8>),
    Dib(Vec<u8>),
}

/// The data of clipboard `format`; the clipboard must be open.
unsafe fn clipboard_bytes(format: u32) -> Option<Vec<u8>> {
    unsafe {
        let h = GetClipboardData(format).ok()?;
        let hg = HGLOBAL(h.0);
        let p = GlobalLock(hg) as *const u8;
        if p.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(p, GlobalSize(hg)).to_vec();
        let _ = GlobalUnlock(hg);
        Some(bytes)
    }
}

pub fn clipboard_image(hwnd: HWND) -> Option<ClipImage> {
    if cfg!(test) {
        return None;
    }
    unsafe {
        let png = RegisterClipboardFormatW(w!("PNG"));
        let has = |f: u32| f != 0 && IsClipboardFormatAvailable(f).is_ok();
        let format = [png, CF_DIB.0 as u32].into_iter().find(|&f| has(f))?;
        OpenClipboard(Some(hwnd)).ok()?;
        let bytes = clipboard_bytes(format);
        let _ = CloseClipboard();
        let bytes = bytes?;
        Some(if format == png {
            ClipImage::Png(bytes)
        } else {
            ClipImage::Dib(bytes)
        })
    }
}

/// Saves a clipboard image as a PNG file.
pub fn save_png(image: &ClipImage, path: &Path) -> Result<()> {
    let dib = match image {
        ClipImage::Png(bytes) => return std::fs::write(path, bytes).map_err(|_| E_FAIL.into()),
        ClipImage::Dib(dib) => dib,
    };
    let bmp = crate::attach::bmp_from_dib(dib).ok_or(Error::from(E_FAIL))?;
    unsafe {
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let input = wic.CreateStream()?;
        input.InitializeFromMemory(&bmp)?;
        let dec =
            wic.CreateDecoderFromStream(&input, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        // Opaque pixels: alpha in clipboard DIBs is usually unset.
        let conv = wic.CreateFormatConverter()?;
        conv.Initialize(
            &dec.GetFrame(0)?,
            &GUID_WICPixelFormat24bppBGR,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let out = wic.CreateStream()?;
        out.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&out, WICBitmapEncoderNoCache)?;
        let (mut frame, mut props) = (None, None);
        enc.CreateNewFrame(&mut frame, &mut props)?;
        let frame = frame.ok_or(Error::from(E_FAIL))?;
        frame.Initialize(props.as_ref())?;
        frame.WriteSource(&conv, std::ptr::null())?;
        frame.Commit()?;
        enc.Commit()?;
    }
    Ok(())
}

/// `s` with `%VARS%` expanded as Windows does.
pub fn expand_env(s: &str) -> String {
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
    let src = HSTRING::from(s);
    unsafe {
        let n = ExpandEnvironmentStringsW(&src, None);
        let mut buf = vec![0u16; n as usize];
        let m = ExpandEnvironmentStringsW(&src, Some(&mut buf)) as usize;
        if m == 0 || m > buf.len() {
            return s.to_string();
        }
        String::from_utf16_lossy(&buf[..m - 1])
    }
}

/// Starts the command line `line` in `cwd`: console programs get a new console, or
/// none when `hidden`. Returns the process handle.
pub fn spawn(line: &str, cwd: &Path, hidden: bool) -> std::result::Result<HANDLE, String> {
    if cfg!(test) {
        return Err("programs are not started in tests".into());
    }
    use windows::Win32::System::Threading::*;
    let mut cmd: Vec<u16> = line.encode_utf16().chain([0]).collect();
    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    let flags = if hidden {
        CREATE_NO_WINDOW
    } else {
        CREATE_NEW_CONSOLE
    };
    unsafe {
        CreateProcessW(
            None,
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            false,
            flags,
            None,
            &HSTRING::from(cwd.as_os_str()),
            &si,
            &mut pi,
        )
        .map_err(|e| e.message())?;
        let _ = CloseHandle(pi.hThread);
    }
    Ok(pi.hProcess)
}

pub fn close_handle(h: HANDLE) {
    unsafe {
        let _ = CloseHandle(h);
    }
}

/// Asks every QuillNotes window, in every process, to close; each one saves or asks
/// about unsaved changes as it would for its own close.
pub fn close_all_windows() {
    // A test must never close the user's own QuillNotes windows.
    if cfg!(test) {
        return;
    }
    unsafe extern "system" fn close(hwnd: HWND, _: LPARAM) -> BOOL {
        let mut name = [0u16; 32];
        let n = unsafe { GetClassNameW(hwnd, &mut name) } as usize;
        if name[..n] == *unsafe { WINDOW_CLASS.as_wide() } {
            let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        true.into()
    }
    unsafe {
        let _ = EnumWindows(Some(close), LPARAM(0));
    }
}

/// Waits on another thread for `process` to end, then posts `msg` to `hwnd` with
/// the exit code as wparam and `id` as lparam.
pub fn watch_process(process: HANDLE, hwnd: HWND, msg: u32, id: isize) {
    use windows::Win32::System::Threading::*;
    // Handles are not Send; pass them as integers.
    let (p, h) = (process.0 as isize, hwnd.0 as isize);
    std::thread::spawn(move || unsafe {
        let process = HANDLE(p as _);
        WaitForSingleObject(process, INFINITE);
        let mut code = 0u32;
        let _ = GetExitCodeProcess(process, &mut code);
        let _ = CloseHandle(process);
        let _ = PostMessageW(Some(HWND(h as _)), msg, WPARAM(code as usize), LPARAM(id));
    });
}

/// The local date and time: (year, month, day, hour, minute, second).
pub fn now() -> (i32, u32, u32, u32, u32, u32) {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    (
        t.wYear as i32,
        t.wMonth as u32,
        t.wDay as u32,
        t.wHour as u32,
        t.wMinute as u32,
        t.wSecond as u32,
    )
}

const FILTERS: [COMDLG_FILTERSPEC; 3] = [
    COMDLG_FILTERSPEC {
        pszName: w!("Markdown"),
        pszSpec: w!("*.md;*.markdown;*.mdown;*.mkd"),
    },
    COMDLG_FILTERSPEC {
        pszName: w!("Text"),
        pszSpec: w!("*.txt"),
    },
    COMDLG_FILTERSPEC {
        pszName: w!("All files"),
        pszSpec: w!("*.*"),
    },
];

unsafe fn result_path(item: &IShellItem) -> Option<PathBuf> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s.map(PathBuf::from)
    }
}

pub fn open_dialog(hwnd: HWND, dir: Option<&Path>) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    modal(hwnd, || unsafe {
        let dlg: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dlg.SetFileTypes(&FILTERS).ok()?;
        if let Some(item) = dir.and_then(|d| {
            SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(d.as_os_str()), None)
                .ok()
        }) {
            let _ = dlg.SetFolder(&item);
        }
        dlg.Show(Some(hwnd)).ok()?;
        result_path(&dlg.GetResult().ok()?)
    })
}

pub fn save_dialog(
    hwnd: HWND,
    current: Option<&Path>,
    dir: Option<&Path>,
    suggested: &str,
) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    modal(hwnd, || unsafe {
        let dlg: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dlg.SetFileTypes(&FILTERS).ok()?;
        dlg.SetDefaultExtension(w!("md")).ok()?;
        let name = current
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| suggested.to_string());
        let _ = dlg.SetFileName(&HSTRING::from(name));
        if let Some(item) = dir.and_then(|d| {
            SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(d.as_os_str()), None)
                .ok()
        }) {
            let _ = dlg.SetFolder(&item);
        }
        dlg.Show(Some(hwnd)).ok()?;
        result_path(&dlg.GetResult().ok()?)
    })
}

pub fn folder_dialog(hwnd: HWND, start: Option<&Path>) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    modal(hwnd, || unsafe {
        let dlg: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dlg.GetOptions().ok()?;
        dlg.SetOptions(opts | FOS_PICKFOLDERS).ok()?;
        if let Some(item) = start.and_then(|d| {
            SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(d.as_os_str()), None)
                .ok()
        }) {
            let _ = dlg.SetFolder(&item);
        }
        dlg.Show(Some(hwnd)).ok()?;
        result_path(&dlg.GetResult().ok()?)
    })
}

/// Moves a file or folder to the Recycle Bin. Returns false on failure.
pub fn recycle(hwnd: HWND, path: &Path) -> bool {
    if cfg!(test) {
        return false;
    }
    // pFrom is a list of paths, each null-terminated, ending with an extra null.
    let from: Vec<u16> = path.as_os_str().encode_wide().chain([0, 0]).collect();
    let mut op = SHFILEOPSTRUCTW {
        hwnd,
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_WANTNUKEWARNING).0 as u16,
        ..Default::default()
    };
    modal(hwnd, || unsafe {
        SHFileOperationW(&mut op) == 0 && !op.fAnyOperationsAborted.as_bool()
    })
}

/// Opens File Explorer with `path` selected.
pub fn reveal(hwnd: HWND, path: &Path) {
    if cfg!(test) {
        return;
    }
    let args = format!("/select,\"{}\"", path.display());
    unsafe {
        ShellExecuteW(
            Some(hwnd),
            w!("open"),
            w!("explorer.exe"),
            &HSTRING::from(args),
            None,
            SW_SHOWNORMAL,
        );
    }
}

pub fn message_box(
    hwnd: HWND,
    text: &str,
    caption: &str,
    style: MESSAGEBOX_STYLE,
) -> MESSAGEBOX_RESULT {
    // A dialog would wait for a click and hold up the test run. Chaos tests
    // answer at random; any other test must not reach one.
    if cfg!(test) {
        let answers: &[MESSAGEBOX_RESULT] = match style.0 & 0xF {
            3 => &[IDYES, IDNO, IDCANCEL],
            4 => &[IDYES, IDNO],
            _ => &[IDOK],
        };
        return match sandbox::pick(answers.len()) {
            Some(i) => answers[i],
            None => panic!("message box in a test: {text}"),
        };
    }
    modal(hwnd, || unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(text),
            &HSTRING::from(caption),
            style,
        )
    })
}

pub enum MenuItem<'a> {
    Item {
        id: u32,
        label: &'a str,
        checked: bool,
        enabled: bool,
    },
    Separator,
}

/// Shows a context menu at client pixel (x, y); returns the chosen item's id.
pub fn popup_menu(hwnd: HWND, items: &[MenuItem], x: i32, y: i32) -> Option<u32> {
    if cfg!(test) {
        // Chaos tests pick an enabled item, or dismiss the menu.
        let ids: Vec<u32> = items
            .iter()
            .filter_map(|i| match i {
                MenuItem::Item {
                    id, enabled: true, ..
                } => Some(*id),
                _ => None,
            })
            .collect();
        return sandbox::pick(ids.len() + 1).and_then(|i| ids.get(i).copied());
    }
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for item in items {
            let _ = match item {
                MenuItem::Item {
                    id,
                    label,
                    checked,
                    enabled,
                } => {
                    let mut flags = MF_STRING;
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    AppendMenuW(menu, flags, *id as usize, &HSTRING::from(*label))
                }
                MenuItem::Separator => AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()),
            };
        }
        let mut pt = POINT { x, y };
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt);
        let chosen = modal(hwnd, || {
            TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                None,
                hwnd,
                None,
            )
        });
        let _ = DestroyMenu(menu);
        (chosen.0 > 0).then_some(chosen.0 as u32)
    }
}

pub fn shell_open(hwnd: HWND, target: &str) {
    if cfg!(test) {
        return;
    }
    unsafe {
        ShellExecuteW(
            Some(hwnd),
            w!("open"),
            &HSTRING::from(target),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

/// Today's local date: (year, month, day).
pub fn today() -> (i32, u32, u32) {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    (t.wYear as i32, t.wMonth as u32, t.wDay as u32)
}

pub fn system_prefers_dark() -> bool {
    let mut value = 1u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as _),
            Some(&mut size),
        )
    };
    r == ERROR_SUCCESS && value == 0
}

/// Files of the fonts installed for the current user only. DirectWrite's system font
/// collection can miss these (GDI apps still see them), so the renderer adds them itself.
pub fn user_font_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(|d| PathBuf::from(d).join("Microsoft\\Windows\\Fonts"));
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows NT\\CurrentVersion\\Fonts"),
            None,
            KEY_READ,
            &mut key,
        ) != ERROR_SUCCESS
        {
            return out;
        }
        let mut name = [0u16; 512];
        let mut data = [0u16; 1024];
        for i in 0.. {
            let mut name_len = name.len() as u32;
            let mut data_len = (data.len() * 2) as u32;
            let mut kind = 0u32;
            let r = RegEnumValueW(
                key,
                i,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr() as *mut u8),
                Some(&mut data_len),
            );
            if r == ERROR_NO_MORE_ITEMS {
                break;
            }
            if r != ERROR_SUCCESS || REG_VALUE_TYPE(kind) != REG_SZ {
                continue;
            }
            let chars = &data[..data_len as usize / 2];
            let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
            let path = PathBuf::from(String::from_utf16_lossy(&chars[..end]));
            // Bare file names live in the per-user fonts folder.
            let path = match &dir {
                Some(d) if path.is_relative() => d.join(path),
                _ => path,
            };
            if path.is_file() {
                out.push(path);
            }
        }
        let _ = RegCloseKey(key);
    }
    out
}

pub fn set_dark_titlebar(hwnd: HWND, dark: bool) {
    let v = BOOL::from(dark);
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &v as *const BOOL as _,
            4,
        );
    }
}

/// Windows' spell checker for the user's language (or US English).
pub struct SpellChecker(ISpellChecker);

impl SpellChecker {
    /// None when Windows has no checker for the language.
    pub fn new() -> Option<Self> {
        unsafe {
            let factory: ISpellCheckerFactory =
                CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let mut buf = [0u16; 85];
            let n = GetUserDefaultLocaleName(&mut buf);
            let user = (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]));
            for tag in user.into_iter().chain(["en-US".to_string()]) {
                let tag = HSTRING::from(tag);
                if factory.IsSupported(&tag).is_ok_and(|b| b.as_bool()) {
                    return factory.CreateSpellChecker(&tag).ok().map(SpellChecker);
                }
            }
            None
        }
    }

    /// Misspellings in `text` (UTF-16 ending with a NUL), as (start, length) in
    /// UTF-16 units.
    pub fn check(&self, text: &[u16]) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        unsafe {
            let Ok(errors) = self.0.Check(PCWSTR(text.as_ptr())) else {
                return out;
            };
            loop {
                let mut e = None;
                if errors.Next(&mut e) != S_OK {
                    break;
                }
                let Some(e) = e else { break };
                if let (Ok(start), Ok(len)) = (e.StartIndex(), e.Length()) {
                    out.push((start, len));
                }
            }
        }
        out
    }

    /// Up to `max` suggested spellings for `word`.
    pub fn suggest(&self, word: &str, max: usize) -> Vec<String> {
        let mut out = Vec::new();
        unsafe {
            let Ok(list) = self.0.Suggest(&HSTRING::from(word)) else {
                return out;
            };
            while out.len() < max {
                let mut s = [PWSTR::null()];
                let mut got = 0;
                if list.Next(&mut s, Some(&mut got)) != S_OK || got == 0 {
                    break;
                }
                out.push(s[0].to_string().unwrap_or_default());
                CoTaskMemFree(Some(s[0].0 as _));
            }
        }
        out
    }

    /// Adds `word` to the user's dictionary (shared with other Windows apps).
    pub fn add(&self, word: &str) {
        let _ = unsafe { self.0.Add(&HSTRING::from(word)) };
    }

    /// Stops flagging `word` until QuillNotes closes.
    pub fn ignore(&self, word: &str) {
        let _ = unsafe { self.0.Ignore(&HSTRING::from(word)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cf_html_offsets() {
        let frag = "<p>caf\u{e9} \u{2611}</p>";
        let doc = cf_html(frag);
        let offset = |key: &str| -> usize {
            let at = doc.find(key).unwrap() + key.len();
            doc[at..at + 10].parse().unwrap()
        };
        assert!(doc[offset("StartHTML:")..].starts_with("<html>"));
        assert_eq!(offset("EndHTML:"), doc.len());
        assert_eq!(&doc[offset("StartFragment:")..offset("EndFragment:")], frag);
    }
}
