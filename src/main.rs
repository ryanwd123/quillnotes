#![windows_subsystem = "windows"]

mod app;
mod attach;
mod command;
mod doc;
mod dragdrop;
mod highlight;
mod keymap;
mod markdown;
mod options;
mod palette;
mod platform;
mod render;
mod settings;
mod slash;
mod table;
mod workspace;

use std::path::PathBuf;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{IDropTarget, OleInitialize, RegisterDragDrop, RevokeDragDrop};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use app::App;

/// From WinUser.h; windows-rs only exposes it with the Win32_UI_Controls feature.
const WM_MOUSELEAVE: u32 = 0x02A3;

pub(crate) fn app_of(hwnd: HWND) -> Option<&'static mut App> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut App;
    unsafe { p.as_mut() }
}

fn xy(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam.0 & 0xFFFF) as i16 as i32,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    )
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            let app = cs.lpCreateParams as *mut App;
            (*app).hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let Some(app) = app_of(hwnd) else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        if platform::IN_MODAL.with(|m| m.get()) && msg != WM_SIZE && msg != WM_DPICHANGED {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);
                app.paint();
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                let (w, h) = xy(lparam);
                app.on_size(w as u16 as u32, h as u16 as u32);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                app.view.dpi = (wparam.0 & 0xFFFF) as f32;
                let r = &*(lparam.0 as *const RECT);
                // A restored placement is already in the target monitor's pixels.
                if !app.restoring_placement {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                app.on_size(rc.right as u32, rc.bottom as u32);
                LRESULT(0)
            }
            WM_CHAR => {
                app.on_char(wparam.0 as u16);
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if app.on_key(wparam.0 as u16) {
                    LRESULT(0)
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            WM_SYSCHAR if wparam.0 == ' ' as usize => DefWindowProcW(hwnd, msg, wparam, lparam),
            WM_LBUTTONDOWN => {
                let (x, y) = xy(lparam);
                app.on_mouse_down(x, y);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = xy(lparam);
                app.on_mouse_move(x, y);
                LRESULT(0)
            }
            WM_LBUTTONUP | WM_CAPTURECHANGED => {
                app.on_mouse_up();
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                let (x, y) = xy(lparam);
                app.on_right_click(x, y);
                LRESULT(0)
            }
            // Mouse back / forward buttons.
            WM_XBUTTONUP => {
                app.navigate((wparam.0 >> 16) & 0xFFFF == 1);
                LRESULT(1)
            }
            WM_MOUSELEAVE => {
                app.on_mouse_leave();
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                app.on_wheel((wparam.0 >> 16) as i16);
                LRESULT(0)
            }
            WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                SetCursor(LoadCursorW(None, app.cursor_at(pt.x, pt.y)).ok());
                LRESULT(1)
            }
            WM_TIMER => {
                app.on_timer();
                LRESULT(0)
            }
            WM_SETFOCUS => {
                app.on_focus(true);
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                app.on_focus(false);
                LRESULT(0)
            }
            // Only when the OLE drop target could not be registered.
            WM_DROPFILES => {
                let hdrop = HDROP(wparam.0 as _);
                let files = platform::hdrop_paths(hdrop);
                DragFinish(hdrop);
                app.drop_files(files);
                LRESULT(0)
            }
            app::WM_APP_DROP => {
                app.finish_drop();
                LRESULT(0)
            }
            app::WM_APP_COMMAND_DONE => {
                app.command_done(wparam.0 as u32, lparam.0);
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                app.invalidate();
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_CLOSE => {
                if app.confirm_discard() {
                    app.save_placement();
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            // Shutdown or sign-out skips WM_CLOSE: save (autosave) or ask, like WM_CLOSE.
            WM_QUERYENDSESSION => LRESULT(app.confirm_discard() as isize),
            WM_ENDSESSION => {
                if wparam.0 != 0 {
                    app.save_placement();
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = RevokeDragDrop(hwnd);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // OLE (for drag and drop) also initializes COM, single-threaded.
        if OleInitialize(None).is_err() {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        }

        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = platform::WINDOW_CLASS;
        let wc = WNDCLASSW {
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: LoadIconW(Some(instance), PCWSTR(1 as _))
                .or_else(|_| LoadIconW(None, IDI_APPLICATION))?,
            hInstance: instance,
            lpszClassName: class,
            lpfnWndProc: Some(wndproc),
            ..Default::default()
        };
        RegisterClassW(&wc);

        let dark = platform::system_prefers_dark();
        let view = render::View::new(dark)?;
        let app = Box::into_raw(Box::new(App::new(view)));

        let scale = GetDpiForSystem() as f32 / 96.0;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            &HSTRING::from(app::APP_NAME),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            (1000.0 * scale) as i32,
            (780.0 * scale) as i32,
            None,
            None,
            Some(instance),
            Some(app as _),
        )?;

        let a = &mut *app;
        a.view.dpi = GetDpiForWindow(hwnd) as f32;
        platform::set_dark_titlebar(hwnd, dark);
        let target: IDropTarget = dragdrop::DropTarget::new(hwnd).into();
        if RegisterDragDrop(hwnd, &target).is_err() {
            DragAcceptFiles(hwnd, true);
        }
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        a.on_size(rc.right as u32, rc.bottom as u32);
        match std::env::args_os().nth(1).map(PathBuf::from) {
            Some(arg) if arg.as_os_str() == "--help" => a.show_help(),
            // A new window (Ctrl+Shift+N) starts empty.
            Some(arg) if arg.as_os_str() == "--new" => {}
            Some(dir) if dir.is_dir() => a.open_folder(&dir),
            Some(file) if file.exists() => a.open_path(&file),
            Some(file) => a.doc.path = Some(file),
            // No arguments: reopen the most recent folder and its last file.
            None => a.restore_last_folder(),
        }
        a.update_title();

        if !a.restore_placement() {
            let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        drop(Box::from_raw(app));
        Ok(())
    }
}
