//! Let Windows move the borderless window by its application-drawn caption.
use eframe::egui;
use std::{cell::Cell, io};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::ScreenToClient,
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
    moving: Cell<bool>,
    ctx: egui::Context,
}

impl WindowChrome {
    pub fn attach(window: HWND, ctx: &egui::Context) -> io::Result<Self> {
        let state = Box::new(Caption {
            bounds: Cell::new(RECT::default()),
            moving: Cell::new(false),
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
        WM_ENTERSIZEMOVE => state.moving.set(true),
        WM_EXITSIZEMOVE => {
            state.moving.set(false);
            state.ctx.request_repaint();
        }
        WM_WINDOWPOSCHANGED if state.moving.get() && l != 0 => {
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
            RemoveWindowSubclass(hwnd, Some(caption_proc), id);
        },
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, msg, w, l) }
}
