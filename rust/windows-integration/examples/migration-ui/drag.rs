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
        ClientToScreen, GetMonitorInfoW, GetUpdateRect, InvalidateRect, MonitorFromWindow,
        MONITORINFO, MONITOR_DEFAULTTONEAREST,
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

fn wait_for(description: &str, mut condition: impl FnMut() -> io::Result<bool>) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition()? {
        if Instant::now() >= deadline {
            return Err(io::Error::other(description.to_owned()));
        }
        sleep(Duration::from_millis(15));
    }
    Ok(())
}

fn frame(hwnd: HWND) -> io::Result<usize> {
    let value = unsafe { GetPropW(hwnd, windows_sys::w!("Svyaz.Test.Frame")) } as usize;
    if value == 0 {
        return Err(io::Error::other(
            "The installed candidate did not expose a nonzero egui pass counter",
        ));
    }
    Ok(value)
}

fn full_window_drag() -> io::Result<bool> {
    let mut enabled = 0i32;
    if unsafe {
        SystemParametersInfoW(
            SPI_GETDRAGFULLWINDOWS,
            0,
            (&mut enabled as *mut i32).cast(),
            0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(enabled != 0)
}

// Hosted Windows defaults to moving an outline. Exercise the live DWM path and
// restore the temporary runner preference even when the fixture assertion fails.
struct FullWindowDrag(bool);

impl FullWindowDrag {
    fn on_runner() -> io::Result<Self> {
        if full_window_drag()? {
            return Ok(Self(false));
        }
        if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
            || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
        {
            return Err(io::Error::other(
                "Enable full-window dragging to run this desktop test",
            ));
        }
        if unsafe { SystemParametersInfoW(SPI_SETDRAGFULLWINDOWS, 1, ptr::null_mut(), 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let restore = Self(true);
        if !full_window_drag()? {
            return Err(io::Error::other(
                "Runner did not enable full-window dragging",
            ));
        }
        Ok(restore)
    }
}

impl Drop for FullWindowDrag {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                SystemParametersInfoW(SPI_SETDRAGFULLWINDOWS, 0, ptr::null_mut(), 0);
            }
        }
    }
}

fn invalidate(hwnd: HWND) -> io::Result<()> {
    // Give Windows a real update region. Never send a fake WM_PAINT to the application.
    if unsafe { InvalidateRect(hwnd, ptr::null(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    wait_for("The fixture did not validate its dirty client area", || {
        Ok(unsafe { GetUpdateRect(hwnd, ptr::null_mut(), 0) } == 0)
    })
}

fn trace_motion(
    hwnd: HWND,
    pid: u32,
    tid: u32,
    x: i32,
    y: i32,
    distance: i32,
) -> io::Result<usize> {
    // Allow initial hover and caption transitions to finish before counting pass requests.
    let mut last = frame(hwnd)?;
    let mut quiet_since = Instant::now();
    wait_for(
        "The frame counter did not settle after entering the move loop",
        || {
            let current = frame(hwnd)?;
            if current != last {
                last = current;
                quiet_since = Instant::now();
            }
            Ok(quiet_since.elapsed() >= Duration::from_millis(180))
        },
    )?;
    let baseline = last;
    let full = full_window_drag()?;
    let mut prior_pointer = POINT::default();
    if unsafe { GetCursorPos(&mut prior_pointer) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let steps = [7, 2, 6, 3, 8, 2, 7, 1, 6, 2, 5, 8];
    let mut positions = Vec::new();
    let mut motion_ms = Vec::new();
    for eighths in steps {
        check_focus(hwnd, pid)?;
        let mut gui = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetGUIThreadInfo(tid, &mut gui) } == 0
            || gui.hwndMoveSize != hwnd
            || gui.flags & GUI_INMOVESIZE == 0
        {
            return Err(io::Error::other(
                "The native GUI move loop ended before the input sequence",
            ));
        }
        let from = window_rect(hwnd)?.left;
        let target = x + distance * eighths / 8;
        let direction = (target - prior_pointer.x).signum();
        let sent = Instant::now();
        move_pointer(target, y)?;
        wait_for(
            "Drag input did not move the cursor and the fixture in the requested direction",
            || {
                check_focus(hwnd, pid)?;
                let mut cursor = POINT::default();
                if unsafe { GetCursorPos(&mut cursor) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                let geometry_followed = !full || (window_rect(hwnd)?.left - from) * direction >= 3;
                Ok(
                    (cursor.x - target).abs() <= 1
                        && (cursor.y - y).abs() <= 1
                        && geometry_followed,
                )
            },
        )?;
        motion_ms.push(sent.elapsed().as_millis());
        prior_pointer.x = target;
        positions.push(window_rect(hwnd)?.left);
        invalidate(hwnd)?;
        // Separate stimuli so that coalescing a whole burst into one paint cannot pass the test.
        sleep(Duration::from_millis(80));
    }
    let at_release = frame(hwnd)?;
    let count = at_release
        .checked_sub(baseline)
        .ok_or_else(|| io::Error::other("The egui counter went backwards"))?;
    eprintln!("Caption drag: full_window={full}, positions={positions:?}, invalidations={}, egui_passes={count} ({} -> {})", steps.len(), baseline, at_release);
    if full {
        motion_ms.sort_unstable();
        eprintln!(
            "Input to observed window position (ms): median={}, max={}",
            motion_ms[motion_ms.len() / 2],
            motion_ms.last().unwrap()
        );
        // Allow an isolated scheduling delay on a shared Windows runner.
        if motion_ms.iter().filter(|&&ms| ms >= 250).count() >= 3 {
            return Err(io::Error::other(
                "The window repeatedly stalled while following the mouse",
            ));
        }
    }
    if count > 1 {
        return Err(io::Error::other(
            "Repeated egui passes during a pure window move",
        ));
    }
    Ok(at_release)
}

pub fn exercise(hwnd: HWND, pid: u32) -> io::Result<()> {
    let _full_drag = FullWindowDrag::on_runner()?;
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
    let at_release = trace_motion(hwnd, pid, tid, x, y, distance)?;
    button.release()?; // Drop also releases the button on all preceding error paths.
    wait_for(
        "Native caption drag did not commit its final position",
        || {
            check_focus(hwnd, pid)?;
            Ok((window_rect(hwnd)?.left - before.left) * distance.signum() >= 12)
        },
    )?;
    wait_for(
        "The fixture did not repaint after releasing the caption",
        || Ok(frame(hwnd)? > at_release),
    )?;
    let after_exit = frame(hwnd)?;
    // A regular user action must advance the frame count once movement stops.
    super::capture::choose_tab(hwnd, pid, 1)?;
    wait_for(
        "The interface did not redraw on input after the drag",
        || Ok(frame(hwnd)? > after_exit),
    )?;
    eprintln!("Caption released: egui pass at release={at_release}, after exit={after_exit}, after tab input={}", frame(hwnd)?);
    unsafe {
        SetCursorPos(cursor.x, cursor.y);
    }
    Ok(())
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
        let pack = |p: POINT| ((p.x as u16 as u32) | ((p.y as u16 as u32) << 16)) as isize;
        let client = pack(point);
        if unsafe { ClientToScreen(hwnd, &mut point) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // Only target the installed window. During restore, DWM can temporarily
        // put a transition surface over its buttons on a hosted Windows desktop.
        if unsafe { SendMessageW(hwnd, WM_NCHITTEST, 0, pack(point)) } != HTCLIENT as isize {
            return Err(io::Error::other(format!(
                "Button {from_right} is inside the drag region"
            )));
        }
        for (message, buttons) in [(WM_MOUSEMOVE, 0), (WM_LBUTTONDOWN, 1), (WM_LBUTTONUP, 0)] {
            if unsafe { PostMessageW(hwnd, message, buttons, client) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    };
    let wait = |action: &str, ready: &dyn Fn() -> bool| -> io::Result<()> {
        let until = Instant::now() + Duration::from_secs(5);
        while !ready() {
            if Instant::now() >= until {
                return Err(io::Error::other(format!("Caption failed to {action}")));
            }
            sleep(Duration::from_millis(30));
        }
        Ok(())
    };
    click(65.)?;
    wait("minimize", &|| unsafe { IsIconic(hwnd) != 0 })?;
    unsafe {
        ShowWindowAsync(hwnd, SW_RESTORE);
    }
    wait("restore", &|| unsafe {
        IsIconic(hwnd) == 0 && IsWindowVisible(hwnd) != 0
    })?;
    super::capture::foreground(hwnd);
    click(31.)?;
    wait("hide to tray", &|| unsafe {
        IsWindow(hwnd) != 0 && IsWindowVisible(hwnd) == 0
    })
}
