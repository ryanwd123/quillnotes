//! Thin wrappers over Win32 services: clipboard, dialogs, shell, theme.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

thread_local! {
    /// Set while a modal dialog runs its own message loop.
    pub static IN_MODAL: Cell<bool> = const { Cell::new(false) };
}

fn modal<T>(f: impl FnOnce() -> T) -> T {
    IN_MODAL.with(|m| m.set(true));
    let r = f();
    IN_MODAL.with(|m| m.set(false));
    r
}

pub fn get_clipboard(hwnd: HWND) -> Option<String> {
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
    let wide: Vec<u16> = text
        .replace('\n', "\r\n")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            let p = GlobalLock(hg) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                let _ = GlobalUnlock(hg);
                let _ = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hg.0)));
            }
        }
        let _ = CloseClipboard();
    }
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
    modal(|| unsafe {
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

pub fn save_dialog(hwnd: HWND, current: Option<&Path>, suggested: &str) -> Option<PathBuf> {
    modal(|| unsafe {
        let dlg: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dlg.SetFileTypes(&FILTERS).ok()?;
        dlg.SetDefaultExtension(w!("md")).ok()?;
        let name = current
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| suggested.to_string());
        let _ = dlg.SetFileName(&HSTRING::from(name));
        if let Some(item) = current.and_then(|p| p.parent()).and_then(|d| {
            SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(d.as_os_str()), None)
                .ok()
        }) {
            let _ = dlg.SetFolder(&item);
        }
        dlg.Show(Some(hwnd)).ok()?;
        result_path(&dlg.GetResult().ok()?)
    })
}

pub fn message_box(
    hwnd: HWND,
    text: &str,
    caption: &str,
    style: MESSAGEBOX_STYLE,
) -> MESSAGEBOX_RESULT {
    modal(|| unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(text),
            &HSTRING::from(caption),
            style,
        )
    })
}

pub fn shell_open(hwnd: HWND, target: &str) {
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
