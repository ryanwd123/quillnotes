//! OLE drop target, so files dragged over the window show where they will be
//! inserted (WM_DROPFILES only reports the drop itself).

use std::cell::Cell;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::HDROP;
use windows::core::*;

use crate::platform;

#[implement(IDropTarget)]
pub struct DropTarget {
    hwnd: HWND,
    /// The data being dragged holds files.
    files: Cell<bool>,
}

impl DropTarget {
    pub fn new(hwnd: HWND) -> Self {
        DropTarget {
            hwnd,
            files: Cell::new(false),
        }
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

impl DropTarget_Impl {
    /// Updates the drop position and the effect shown by the drag cursor.
    fn over(&self, pt: &POINTL, effect: *mut DROPEFFECT) -> bool {
        let mut p = POINT { x: pt.x, y: pt.y };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
        }
        let ok = self.files.get()
            && !platform::IN_MODAL.with(|m| m.get())
            && crate::app_of(self.hwnd).is_some_and(|a| a.drag_over(p.x, p.y));
        if !effect.is_null() {
            unsafe { *effect = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        }
        ok
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let files = data
            .as_ref()
            .is_some_and(|d| unsafe { d.QueryGetData(&hdrop_format()) } == S_OK);
        self.files.set(files);
        self.over(pt, effect);
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        self.over(pt, effect);
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if let Some(app) = crate::app_of(self.hwnd) {
            app.drag_leave();
        }
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let app = crate::app_of(self.hwnd);
        if !self.over(pt, effect) {
            if let Some(app) = app {
                app.drag_leave();
            }
            return Ok(());
        }
        let files = data
            .as_ref()
            .and_then(|d| unsafe { d.GetData(&hdrop_format()) }.ok())
            .map(|mut m| unsafe {
                let files = platform::hdrop_paths(HDROP(m.u.hGlobal.0));
                ReleaseStgMedium(&mut m);
                files
            })
            .unwrap_or_default();
        if let Some(app) = app {
            app.drop_files(files);
        }
        Ok(())
    }
}
