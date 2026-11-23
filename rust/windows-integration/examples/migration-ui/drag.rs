//! Exercise Windows' own caption move loop before checking the four tabs.
use super::capture::check_focus;
use std::{
    io, ptr,
    thread::sleep,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::{
        ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    },
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32) -> io::Result<()> {
    let event = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    if unsafe { SendInput(1, &event, size_of::<INPUT>() as i32) } != 1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct LeftButton(bool);
impl LeftButton {
    fn release(mut self) -> io::Result<()> {
        mouse(MOUSEEVENTF_LEFTUP, 0, 0)?;
        self.0 = false;
        Ok(())
    }
}
impl Drop for LeftButton {
    fn drop(&mut self) {
        if self.0 {
            let _ = mouse(MOUSEEVENTF_LEFTUP, 0, 0);
        }
    }
}

fn move_pointer(x: i32, y: i32) -> io::Result<()> {
    let pixel = |p: i32, origin, span| -> io::Result<i32> {
        let offset = p - unsafe { GetSystemMetrics(origin) };
        let size = unsafe { GetSystemMetrics(span) };
        if !(0..size).contains(&offset) {
            return Err(io::Error::other("Drag point outside the virtual desktop"));
        }
        // Absolute input uses 0..65535 over the virtual desktop. Aim at the pixel centre.
        Ok(((i64::from(offset) * 65536 + 32768) / i64::from(size)) as i32)
    };
    mouse(
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        pixel(x, SM_XVIRTUALSCREEN, SM_CXVIRTUALSCREEN)?,
        pixel(y, SM_YVIRTUALSCREEN, SM_CYVIRTUALSCREEN)?,
    )
}

fn window_rect(hwnd: HWND) -> io::Result<RECT> {
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(rect)
}

// Let the application identify a draggable part of its custom client header.
// The central band avoids the logo and the window controls on the right.
fn caption_point(hwnd: HWND, rect: RECT, client: POINT, work: RECT) -> io::Result<POINT> {
    for dy in (16..100).step_by(8) {
        let y = client.y + dy;
        if y <= work.top + 8 || y >= work.bottom {
            continue;
        }
        for column in [3, 2, 4] {
            let x = rect.left + (rect.right - rect.left) * column / 6;
            let point = POINT { x, y };
            if x < work.left || x >= work.right || unsafe { WindowFromPoint(point) } != hwnd {
                continue;
            }
            let param = ((x as u16 as u32) | ((y as u16 as u32) << 16)) as isize;
            let mut hit = 0;
            if unsafe {
                SendMessageTimeoutW(
                    hwnd,
                    WM_NCHITTEST,
                    0,
                    param,
                    SMTO_ABORTIFHUNG,
                    2000,
                    &mut hit,
                )
            } == 0
            {
                return Err(io::Error::other(
                    "Fixture did not answer a caption hit test",
                ));
            }
            if hit == HTCAPTION as usize {
                return Ok(point);
            }
        }
    }
    Err(io::Error::other(
        "No draggable caption in the fixture's client header",
    ))
}

pub fn exercise(hwnd: HWND, pid: u32) -> io::Result<()> {
    check_focus(hwnd, pid)?;
    let before = window_rect(hwnd)?;
    let mut area = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let mut client = POINT::default();
    let mut cursor = POINT::default();
    unsafe {
        if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut area) == 0
            || ClientToScreen(hwnd, &mut client) == 0
            || GetCursorPos(&mut cursor) == 0
        {
            return Err(io::Error::last_os_error());
        }
        if GetAsyncKeyState(i32::from(VK_LBUTTON)) < 0 {
            return Err(io::Error::other("Left mouse button already held"));
        }
    }
    let work = area.rcWork;
    let title = caption_point(hwnd, before, client, work)?;
    let POINT { x, y } = title;
    // Require enough room for the entire window. Keep the pointer away from desktop snap zones.
    let right = work.right - before.right - 12;
    let left = before.left - work.left - 12;
    let distance = if right >= left {
        right.min(72)
    } else {
        -left.min(72)
    };
    if right.max(left) < 36 || before.right - before.left >= work.right - work.left {
        return Err(io::Error::other(
            "No safe horizontal caption-drag path on the fixture monitor",
        ));
    }
    unsafe {
        if SetCursorPos(x, y) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    check_focus(hwnd, pid)?;
    if unsafe { WindowFromPoint(title) } != hwnd {
        return Err(io::Error::other("Fixture no longer owns the caption point"));
    }
    mouse(MOUSEEVENTF_LEFTDOWN, 0, 0)?;
    let button = LeftButton(true);
    // Separate the first motion from the later ones using actual GUI state, not frame timing.
    let tid = unsafe { GetWindowThreadProcessId(hwnd, ptr::null_mut()) };
    let mut gui = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    let until = Instant::now() + Duration::from_secs(5);
    let mut first = true;
    loop {
        check_focus(hwnd, pid)?;
        move_pointer(x + distance / 3 + i32::from(first), y)?;
        first = !first;
        if unsafe { GetGUIThreadInfo(tid, &mut gui) } != 0
            && gui.flags & GUI_INMOVESIZE != 0
            && gui.hwndMoveSize == hwnd
        {
            break;
        }
        if Instant::now() >= until {
            return Err(io::Error::other(
                "Windows did not enter its native caption move loop",
            ));
        }
        sleep(Duration::from_millis(30));
    }
    check_focus(hwnd, pid)?;
    move_pointer(x + distance, y)?;
    button.release()?; // Drop also releases the button on every preceding error path.
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        check_focus(hwnd, pid)?;
        let after = window_rect(hwnd)?;
        if (after.left - before.left) * distance.signum() >= 12 {
            unsafe {
                SetCursorPos(cursor.x, cursor.y);
            }
            return Ok(());
        }
        if Instant::now() >= until {
            return Err(io::Error::other(
                "Native caption drag did not move the window",
            ));
        }
        sleep(Duration::from_millis(40));
    }
}

/// Exercise the two application-drawn controls while the disposable fixture is foreground.
pub fn window_buttons(hwnd: HWND, pid: u32) -> io::Result<()> {
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.;
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) } == 0 || scale <= 0. {
        return Err(io::Error::last_os_error());
    }
    let click = |from_right: f32| -> io::Result<()> {
        check_focus(hwnd, pid)?;
        let mut point = POINT {
            x: rect.right - (from_right * scale).round() as i32,
            y: (23. * scale).round() as i32,
        };
        if unsafe { ClientToScreen(hwnd, &mut point) } == 0
            || unsafe { WindowFromPoint(point) } != hwnd
        {
            return Err(io::Error::other(
                "Caption button outside the fixture window",
            ));
        }
        move_pointer(point.x, point.y)?;
        check_focus(hwnd, pid)?;
        mouse(MOUSEEVENTF_LEFTDOWN, 0, 0)?;
        LeftButton(true).release()
    };
    let wait = |ready: &dyn Fn() -> bool| -> io::Result<()> {
        let until = Instant::now() + Duration::from_secs(5);
        while !ready() {
            if Instant::now() >= until {
                return Err(io::Error::other(
                    "Caption button did not change window state",
                ));
            }
            sleep(Duration::from_millis(30));
        }
        Ok(())
    };
    click(65.)?;
    wait(&|| unsafe { IsIconic(hwnd) != 0 })?;
    unsafe {
        ShowWindowAsync(hwnd, SW_RESTORE);
    }
    wait(&|| unsafe { IsIconic(hwnd) == 0 && IsWindowVisible(hwnd) != 0 })?;
    super::capture::foreground(hwnd);
    click(31.)?;
    wait(&|| unsafe { IsWindow(hwnd) != 0 && IsWindowVisible(hwnd) == 0 })
}
