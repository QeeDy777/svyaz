//! Let Windows move the borderless window by its application-drawn caption.
use eframe::egui;
use std::{cell::Cell, io, ptr};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{InvalidateRect, ScreenToClient, ValidateRect},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

pub struct WindowChrome {
    window: HWND,
    state: Box<Caption>,
}

struct Caption {
    bounds: Cell<RECT>,
    // Last presented surface can be reused while the window translates at this client size.
    move_size: Cell<Option<(i32, i32)>>,
    count_test_frames: bool,
    ctx: egui::Context,
}

impl WindowChrome {
    pub fn attach(window: HWND, ctx: &egui::Context) -> io::Result<Self> {
        let state = Box::new(Caption {
            bounds: Cell::new(RECT::default()),
            move_size: Cell::new(None),
            count_test_frames: std::env::var_os("SVYAZ_TEST_FRAME_COUNTER").as_deref()
                == Some(std::ffi::OsStr::new("1")),
            ctx: ctx.clone(),
        });
        if unsafe {
            SetWindowSubclass(
                window,
                Some(caption_proc),
                1,
                state.as_ref() as *const _ as usize,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { window, state })
    }

    pub fn record_test_frame(&self) {
        if self.state.count_test_frames {
            let pass = self.state.ctx.cumulative_pass_nr().wrapping_add(1) as usize;
            unsafe {
                SetPropW(self.window, windows_sys::w!("Svyaz.Test.Frame"), pass as _);
            }
        }
    }

    pub fn set_drag_bounds(&self, bounds: egui::Rect, pixels_per_point: f32) {
        let pixel = |v: f32| (v * pixels_per_point).round() as i32;
        self.state.bounds.set(RECT {
            left: pixel(bounds.left()),
            top: pixel(bounds.top()),
            right: pixel(bounds.right()),
            bottom: pixel(bounds.bottom()),
        });
    }
}

impl Drop for WindowChrome {
    fn drop(&mut self) {
        unsafe {
            RemoveWindowSubclass(self.window, Some(caption_proc), 1);
        }
    }
}

unsafe extern "system" fn caption_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    // Only the UI thread calls this procedure; the boxed state outlives the hook.
    let state = unsafe { &*(data as *const Caption) };
    match msg {
        WM_NCHITTEST => {
            let result = unsafe { DefSubclassProc(hwnd, msg, w, l) };
            if result == HTCLIENT as LRESULT {
                let mut p = POINT {
                    x: l as i16 as i32,
                    y: (l >> 16) as i16 as i32,
                };
                let r = state.bounds.get();
                if unsafe { ScreenToClient(hwnd, &mut p) } != 0
                    && (r.left..r.right).contains(&p.x)
                    && (r.top..r.bottom).contains(&p.y)
                {
                    return HTCAPTION as LRESULT;
                }
            }
            return result;
        }
        // A double click on the caption must leave the compact window at its fixed size.
        WM_NCLBUTTONDBLCLK if w == HTCAPTION as usize => return 0,
        WM_ENTERSIZEMOVE => {
            let mut r = RECT::default();
            if unsafe { GetClientRect(hwnd, &mut r) } != 0 && r.right > 0 && r.bottom > 0 {
                state.move_size.set(Some((r.right, r.bottom)));
            }
        }
        WM_PAINT if state.move_size.get().is_some() => {
            // DWM can translate the last frame. A scheduled egui frame here would
            // present the same pixels on the thread running Windows' modal move loop.
            if unsafe { ValidateRect(hwnd, ptr::null()) } != 0 {
                return 0;
            }
        }
        WM_SIZE if let Some(size) = state.move_size.get() => {
            let next = (i32::from(l as u16), i32::from((l >> 16) as u16));
            if w == SIZE_RESTORED as usize && next == size {
                // eframe reconfigures the swap chain synchronously on every Resized event.
                return 0;
            }
            state.move_size.set(None);
        }
        WM_DPICHANGED | WM_SIZING | WM_DISPLAYCHANGE | WM_SHOWWINDOW => {
            state.move_size.set(None);
        }
        WM_EXITSIZEMOVE => {
            state.move_size.set(None);
            // A WM_PAINT consumed during movement no longer has an update region.
            unsafe {
                InvalidateRect(hwnd, ptr::null(), 0);
            }
            state.ctx.request_repaint();
        }
        WM_WINDOWPOSCHANGED if state.move_size.get().is_some() && l != 0 => {
            let flags = unsafe { (*(l as *const WINDOWPOS)).flags };
            let excluded = SWP_NOMOVE | SWP_FRAMECHANGED | SWP_SHOWWINDOW | SWP_HIDEWINDOW;
            if flags & (SWP_NOSIZE | SWP_NOZORDER) == SWP_NOSIZE | SWP_NOZORDER
                && flags & excluded == 0
            {
                // winit emits Moved on each step, which schedules a complete GPU frame in egui.
                // Let Windows deliver WM_MOVE and reuse the composed surface for translation.
                return unsafe { DefWindowProcW(hwnd, msg, w, l) };
            }
        }
        WM_NCDESTROY => unsafe {
            if state.count_test_frames {
                RemovePropW(hwnd, windows_sys::w!("Svyaz.Test.Frame"));
            }
            RemoveWindowSubclass(hwnd, Some(caption_proc), id);
        },
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, msg, w, l) }
}
