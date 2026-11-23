//! Inspect the installed candidate through its real Windows desktop. This helper ships only with the tests.
use image::{Rgb, RgbImage};
use std::{
    io,
    path::Path,
    ptr,
    thread::sleep,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT},
    Graphics::Gdi::*,
    Storage::Xps::PrintWindow,
    System::Threading::{AttachThreadInput, GetCurrentThreadId},
    UI::{
        HiDpi::{
            GetSystemMetricsForDpi, SetProcessDpiAwarenessContext,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        },
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::*,
    },
};

pub fn run() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let pid = args
        .get(1)
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&p| p > 0)
        .ok_or_else(|| io::Error::other("Expected the running fixture PID"))?;
    // Use physical pixels for the client rectangle, capture and tab checks on every monitor.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    match args.as_slice() {
        [cmd, _, hwnd, dir] if cmd == "capture" => {
            let hwnd = hwnd.parse::<usize>().map_err(io::Error::other)? as HWND;
            if hwnd.is_null() || owner(hwnd) != pid || unsafe { IsWindowVisible(hwnd) } == 0 {
                return Err(io::Error::other("Fixture does not own this visible window"));
            }
            let class = window_class(hwnd)?;
            check_window_icons(hwnd)?;
            foreground(hwnd);
            super::drag::exercise(hwnd, pid)?;
            for (i, tab) in ["connection", "telegram", "settings", "journal"]
                .iter()
                .enumerate()
            {
                save_tab(
                    hwnd,
                    pid,
                    i,
                    &Path::new(dir).join(format!("rust-{tab}.png")),
                )?;
            }
            super::drag::window_buttons(hwnd, pid)?;
            println!("{class}");
            Ok(())
        }
        [cmd, _, class] if cmd == "hidden" && !class.is_empty() => {
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                let mut search = Search {
                    pid,
                    class,
                    hidden: false,
                };
                unsafe {
                    EnumWindows(Some(find_hidden), &mut search as *mut _ as LPARAM);
                }
                if search.hidden {
                    return Ok(());
                }
                if Instant::now() >= until {
                    return Err(io::Error::other(
                        "The ready runtime did not hide its desktop window",
                    ));
                }
                sleep(Duration::from_millis(100));
            }
        }
        _ => Err(io::Error::other(
            "Expected capture PID HWND DIRECTORY or hidden PID CLASS",
        )),
    }
}

fn owner(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }
    pid
}

fn check_window_icons(hwnd: HWND) -> io::Result<()> {
    for dpi in [96, 120, 144] {
        for (kind, x, y) in [
            (ICON_SMALL, SM_CXSMICON, SM_CYSMICON),
            (ICON_BIG, SM_CXICON, SM_CYICON),
        ] {
            unsafe {
                let mut handle = 0;
                if SendMessageTimeoutW(
                    hwnd,
                    WM_GETICON,
                    kind as usize,
                    dpi as LPARAM,
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    2000,
                    &mut handle,
                ) == 0
                    || handle == 0
                {
                    return Err(io::Error::other(format!(
                        "Window returned no icon for kind {kind} at {dpi} DPI"
                    )));
                }
                let mut info = ICONINFO::default();
                if GetIconInfo(handle as HICON, &mut info) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut pixels = BITMAP::default();
                let length = GetObjectW(
                    info.hbmColor,
                    size_of::<BITMAP>() as i32,
                    (&mut pixels as *mut BITMAP).cast(),
                );
                // GetIconInfo gives the caller two new bitmaps. The HICON belongs to the application.
                for bitmap in [info.hbmColor, info.hbmMask] {
                    if !bitmap.is_null() {
                        DeleteObject(bitmap);
                    }
                }
                let expected = (
                    GetSystemMetricsForDpi(x, dpi),
                    GetSystemMetricsForDpi(y, dpi),
                );
                let actual = (pixels.bmWidth, pixels.bmHeight);
                if length == 0 || actual != expected {
                    return Err(io::Error::other(format!("Window icon kind {kind} at {dpi} DPI: expected {expected:?}, got {actual:?}")));
                }
            }
        }
    }
    Ok(())
}

fn window_class(hwnd: HWND) -> io::Result<String> {
    let mut name = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    String::from_utf16(&name[..len as usize]).map_err(io::Error::other)
}

struct Search<'a> {
    pid: u32,
    class: &'a str,
    hidden: bool,
}
unsafe extern "system" fn find_hidden(hwnd: HWND, param: LPARAM) -> i32 {
    let test = &mut *(param as *mut Search);
    let mut rect = RECT::default();
    if owner(hwnd) == test.pid
        && window_class(hwnd).ok().as_deref() == Some(test.class)
        && GetClientRect(hwnd, &mut rect) != 0
        && rect.right >= 200
        && rect.bottom >= 280
        && IsWindowVisible(hwnd) == 0
    {
        test.hidden = true;
        return 0;
    }
    1
}

pub(super) fn foreground(hwnd: HWND) {
    unsafe {
        ShowWindowAsync(hwnd, SW_SHOW);
        if SetForegroundWindow(hwnd) != 0 {
            return;
        }
        // This short-lived console helper needs an input queue before AttachThreadInput.
        PeekMessageW(&mut MSG::default(), ptr::null_mut(), 0, 0, PM_NOREMOVE);
        let fg = GetWindowThreadProcessId(GetForegroundWindow(), ptr::null_mut());
        let own = GetCurrentThreadId();
        let attached = fg != 0 && fg != own && AttachThreadInput(own, fg, 1) != 0;
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        if attached {
            AttachThreadInput(own, fg, 0);
        }
    }
}

pub(super) fn check_focus(hwnd: HWND, pid: u32) -> io::Result<()> {
    if unsafe { GetForegroundWindow() } != hwnd || owner(hwnd) != pid {
        return Err(io::Error::other("Capture lost foreground ownership"));
    }
    Ok(())
}

pub(super) fn choose_tab(hwnd: HWND, pid: u32, tab: usize) -> io::Result<()> {
    let key = VK_1 + tab as u16;
    let keys = [
        (VK_CONTROL, 0),
        (key, 0),
        (key, KEYEVENTF_KEYUP),
        (VK_CONTROL, KEYEVENTF_KEYUP),
    ]
    .map(|(key, flags)| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: flags,
                ..Default::default()
            },
        },
    });
    check_focus(hwnd, pid)?;
    if unsafe { SendInput(keys.len() as u32, keys.as_ptr(), size_of::<INPUT>() as i32) }
        != keys.len() as u32
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct Surface {
    screen: HDC,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            if !self.previous.is_null() {
                SelectObject(self.dc, self.previous);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            if !self.dc.is_null() {
                DeleteDC(self.dc);
            }
            if !self.screen.is_null() {
                ReleaseDC(ptr::null_mut(), self.screen);
            }
        }
    }
}

fn capture(hwnd: HWND, pid: u32, from_screen: bool) -> io::Result<RgbImage> {
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let (w, h) = (rect.right, rect.bottom);
    if !(280..=8192).contains(&w) || !(320..=8192).contains(&h) {
        return Err(io::Error::other(
            "Fixture client rectangle is outside capture bounds",
        ));
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut surface = Surface {
        screen: ptr::null_mut(),
        dc: ptr::null_mut(),
        bitmap: ptr::null_mut(),
        previous: ptr::null_mut(),
    };
    unsafe {
        surface.screen = GetDC(ptr::null_mut());
        if surface.screen.is_null() {
            return Err(io::Error::last_os_error());
        }
        surface.dc = CreateCompatibleDC(surface.screen);
        if surface.dc.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut pixels = ptr::null_mut();
        surface.bitmap = CreateDIBSection(
            surface.screen,
            &info,
            DIB_RGB_COLORS,
            &mut pixels,
            ptr::null_mut(),
            0,
        );
        if surface.bitmap.is_null() || pixels.is_null() {
            return Err(io::Error::last_os_error());
        }
        surface.previous = SelectObject(surface.dc, surface.bitmap);
        if surface.previous.is_null() {
            return Err(io::Error::last_os_error());
        }
        let painted = if from_screen {
            check_focus(hwnd, pid)?;
            let mut origin = POINT::default();
            if ClientToScreen(hwnd, &mut origin) == 0 {
                return Err(io::Error::last_os_error());
            }
            BitBlt(
                surface.dc,
                0,
                0,
                w,
                h,
                surface.screen,
                origin.x,
                origin.y,
                SRCCOPY,
            )
        } else {
            PrintWindow(hwnd, surface.dc, 3)
        };
        if painted == 0 || GdiFlush() == 0 {
            return Err(io::Error::last_os_error());
        }
        let bytes = std::slice::from_raw_parts(pixels.cast::<u8>(), w as usize * h as usize * 4);
        // A negative bitmap height gives top-down rows; 32-bit DIB pixels are B,G,R,unused.
        Ok(RgbImage::from_fn(w as u32, h as u32, |x, y| {
            let offset = ((y * w as u32 + x) * 4) as usize;
            Rgb([bytes[offset + 2], bytes[offset + 1], bytes[offset]])
        }))
    }
}

fn active_tab(image: &RgbImage, tab: usize) -> bool {
    let (w, h) = image.dimensions();
    let mut counts = [0usize; 4];
    for y in (h.saturating_sub((w as f32 * 0.105).round() as u32)..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let [r, g, b] = image.get_pixel(x, y).0;
            if r > 226 && (151..=195).contains(&g) && b < 136 {
                counts[(4 * x / w) as usize] += 1;
            }
        }
    }
    counts[tab] >= 2
        && counts
            .iter()
            .enumerate()
            .all(|(i, n)| i == tab || *n < counts[tab])
}

fn readable_title(image: &RgbImage) -> bool {
    let w = image.width() as f32;
    let mut light = 0;
    for x in (w * 0.125).round() as u32..(w * 0.235).round() as u32 {
        for y in (w * 0.024).round() as u32..((w * 0.082).round() as u32).min(image.height()) {
            if image.get_pixel(x, y).0.iter().all(|value| *value >= 190) {
                light += 1;
            }
        }
    }
    light >= 15
}

fn save_tab(hwnd: HWND, pid: u32, tab: usize, path: &Path) -> io::Result<()> {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        choose_tab(hwnd, pid, tab)?;
        sleep(Duration::from_millis(160));
        for from_screen in [false, true] {
            check_focus(hwnd, pid)?;
            if let Ok(image) = capture(hwnd, pid, from_screen) {
                if active_tab(&image, tab) && readable_title(&image) {
                    return image.save(path).map_err(io::Error::other);
                }
            }
        }
        if Instant::now() >= until {
            return Err(io::Error::other(format!(
                "Could not verify the rendered tab {}",
                tab + 1
            )));
        }
    }
}
