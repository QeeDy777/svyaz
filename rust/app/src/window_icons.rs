//! DPI-sized HICONs from the icon group embedded by build.rs.
use std::{
    cell::{Cell, RefCell},
    io, ptr, slice,
};
use windows_sys::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::{
            FindResourceW, GetModuleHandleW, LoadResource, LockResource, SizeofResource,
        },
        UI::{
            HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
    core::PCWSTR,
};

fn resource(id: u16, kind: PCWSTR) -> io::Result<&'static [u8]> {
    // The main executable stays loaded for the lifetime of the process. LockResource's
    // data pointer remains valid without any lock or handle for the caller to release.
    unsafe {
        let module = GetModuleHandleW(ptr::null());
        let found = FindResourceW(module, id as usize as PCWSTR, kind);
        if found.is_null() {
            return Err(io::Error::last_os_error());
        }
        let length = SizeofResource(module, found);
        let block = LoadResource(module, found);
        if block.is_null() {
            return Err(io::Error::last_os_error());
        }
        let bytes = LockResource(block);
        if bytes.is_null() || length == 0 {
            return Err(io::Error::other("Пустой ресурс ICO."));
        }
        Ok(slice::from_raw_parts(bytes.cast(), length as usize))
    }
}

// GRPICONDIR has a 6-byte header and 14-byte entries. Select the smallest frame
// containing the requested size; only enlarge if even the largest frame is too small.
fn source_id(group: &[u8], width: i32, height: i32) -> Option<u16> {
    let [0, 0, 1, 0, count_lo, count_hi, entries @ ..] = group else {
        return None;
    };
    let count = usize::from(u16::from_le_bytes([*count_lo, *count_hi]));
    let px = |n: u8| if n == 0 { 256 } else { i32::from(n) };
    let entry = entries
        .get(..count * 14)?
        .chunks_exact(14)
        .min_by_key(|e| {
            let (w, h) = (px(e[0]), px(e[1]));
            (
                w < width || h < height,
                w.abs_diff(width) + h.abs_diff(height),
            )
        })?;
    Some(u16::from_le_bytes([entry[12], entry[13]]))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct IconSize(i32, i32);
impl IconSize {
    pub(super) fn for_dpi(dpi: u32, small: bool) -> Self {
        let (x, y) = if small {
            (SM_CXSMICON, SM_CYSMICON)
        } else {
            (SM_CXICON, SM_CYICON)
        };
        let dpi = dpi.clamp(96, 768);
        unsafe {
            Self(
                GetSystemMetricsForDpi(x, dpi),
                GetSystemMetricsForDpi(y, dpi),
            )
        }
    }
}

pub(super) struct OwnedIcon {
    pub(super) handle: HICON,
    pub(super) size: IconSize,
}
impl OwnedIcon {
    pub(super) fn load(size: IconSize) -> io::Result<Self> {
        let id = source_id(resource(1, RT_GROUP_ICON)?, size.0, size.1)
            .ok_or_else(|| io::Error::other("Повреждён ресурс ICO."))?;
        let pixels = resource(id, RT_ICON)?;
        // No LR_SHARED. The selected PE resource is aligned and already contains
        // PNG-encoded alpha. Windows decodes that frame at the requested pixel size.
        let handle = unsafe {
            CreateIconFromResourceEx(
                pixels.as_ptr(),
                pixels.len() as u32,
                1,
                0x0003_0000,
                size.0,
                size.1,
                0,
            )
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle, size })
    }
}
impl Drop for OwnedIcon {
    fn drop(&mut self) {
        unsafe {
            DestroyIcon(self.handle);
        }
    }
}

struct WindowState {
    hwnd: Cell<HWND>,
    dpi: Cell<u32>,
    // WM_GETICON hands out borrowed handles. Keep each requested size until the
    // native window is gone; repeated queries and monitor moves reuse its handle.
    icons: RefCell<Vec<OwnedIcon>>,
}
impl WindowState {
    fn handle(&self, size: IconSize) -> io::Result<HICON> {
        if let Some(icon) = self.icons.borrow().iter().find(|icon| icon.size == size) {
            return Ok(icon.handle);
        }
        let icon = OwnedIcon::load(size)?;
        let handle = icon.handle;
        self.icons.borrow_mut().push(icon);
        Ok(handle)
    }

    fn set_dpi(&self, dpi: u32) -> io::Result<()> {
        if self.dpi.get() == dpi {
            return Ok(());
        }
        let small = self.handle(IconSize::for_dpi(dpi, true))?;
        let large = self.handle(IconSize::for_dpi(dpi, false))?;
        self.dpi.set(dpi);
        let window = self.hwnd.get();
        unsafe {
            // The old handles may belong to winit. Only OwnedIcon destroys handles we created.
            SendMessageW(window, WM_SETICON, ICON_SMALL as usize, small as LPARAM);
            SendMessageW(window, WM_SETICON, ICON_BIG as usize, large as LPARAM);
        }
        Ok(())
    }
}

pub(super) struct WindowIcons(Box<WindowState>);
impl WindowIcons {
    /// Call on the UI thread and retain until the main window closes.
    pub(super) fn install(hwnd: HWND) -> io::Result<Self> {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi == 0 {
            return Err(io::Error::last_os_error());
        }
        let icons = Self(Box::new(WindowState {
            hwnd: Cell::new(hwnd),
            dpi: Cell::new(0),
            icons: RefCell::new(Vec::new()),
        }));
        if unsafe {
            SetWindowSubclass(
                hwnd,
                Some(icon_message),
                1,
                &*icons.0 as *const WindowState as usize,
            )
        } == 0
        {
            icons.0.hwnd.set(ptr::null_mut());
            return Err(io::Error::last_os_error());
        }
        icons.0.set_dpi(dpi)?;
        Ok(icons)
    }
}

unsafe extern "system" fn icon_message(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    context: usize,
) -> LRESULT {
    let state = unsafe { &*(context as *const WindowState) };
    match msg {
        WM_DPICHANGED => {
            let _ = state.set_dpi((w & 0xffff) as u32);
        }
        WM_GETICON if matches!(w as u32, ICON_SMALL | ICON_SMALL2 | ICON_BIG) => {
            // Shell can ask for the icon on a different monitor than the main window.
            let dpi = if l > 0 { l as u32 } else { state.dpi.get() };
            if let Ok(icon) = state.handle(IconSize::for_dpi(dpi, w as u32 != ICON_BIG)) {
                return icon as LRESULT;
            }
        }
        WM_NCDESTROY => {
            state.hwnd.set(ptr::null_mut());
            unsafe {
                RemoveWindowSubclass(hwnd, Some(icon_message), 1);
            }
        }
        _ => (),
    }
    unsafe { DefSubclassProc(hwnd, msg, w, l) }
}

impl Drop for WindowIcons {
    fn drop(&mut self) {
        let hwnd = self.0.hwnd.get();
        if hwnd.is_null() {
            return;
        }
        unsafe {
            RemoveWindowSubclass(hwnd, Some(icon_message), 1);
            for which in [ICON_SMALL, ICON_BIG] {
                let current = SendMessageW(hwnd, WM_GETICON, which as usize, 0) as HICON;
                if self
                    .0
                    .icons
                    .borrow()
                    .iter()
                    .any(|icon| icon.handle == current)
                {
                    SendMessageW(hwnd, WM_SETICON, which as usize, 0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_group_uses_exact_frames_then_the_next_larger_frame() {
        let group = resource(1, RT_GROUP_ICON).unwrap();
        for (pixels, source) in [
            (16, 16),
            (20, 24),
            (24, 24),
            (28, 32),
            (40, 48),
            (48, 48),
            (56, 64),
            (96, 128),
            (192, 256),
        ] {
            let id = source_id(group, pixels, pixels).unwrap();
            let png = resource(id, RT_ICON).unwrap();
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), source);
            let icon = OwnedIcon::load(IconSize(pixels, pixels)).unwrap();
            assert!(!icon.handle.is_null());
        }
    }
}
