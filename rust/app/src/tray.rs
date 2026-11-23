//! Shell_NotifyIcon owns the tray icon (including its balloon), tray-icon owns its native menu.
use eframe::egui;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    io,
    mem::size_of,
    ptr,
};
use tray_icon::menu::{ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use windows_sys::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::w,
};

const CALLBACK: u32 = WM_APP + 1;
const ID: u32 = 1;
const HOOK: usize = 1;
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Open,
    Connect,
    Repair,
    Update,
    Disconnect,
    ToggleTelegram,
    Quit,
}

pub struct Tray {
    window: HWND,
    // Registered as the subclass context. It must stay at a fixed address until Drop.
    callbacks: Box<Callbacks>,
    strategy: MenuItem,
}
struct Callbacks {
    ctx: egui::Context,
    menu: Menu,
    menu_open: Cell<bool>,
    actions: RefCell<VecDeque<TrayAction>>,
    icon: HICON,
    tooltip: RefCell<String>,
    explorer_started: u32,
}

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ID,
        ..Default::default()
    }
}
fn wide_text<const N: usize>(destination: &mut [u16; N], text: &str) {
    let mut at = 0;
    for ch in text.chars().take_while(|c| *c != '\0') {
        if at + ch.len_utf16() >= N {
            break;
        }
        ch.encode_utf16(&mut destination[at..at + ch.len_utf16()]);
        at += ch.len_utf16();
    }
    destination[at] = 0;
}

impl Callbacks {
    fn add_icon(&self, window: HWND) -> io::Result<()> {
        let mut d = data(window);
        d.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE;
        d.hIcon = self.icon;
        d.uCallbackMessage = CALLBACK;
        wide_text(&mut d.szTip, &self.tooltip.borrow());
        if unsafe { Shell_NotifyIconW(NIM_ADD, &d) } == 0 {
            return Err(io::Error::other(
                "Не удалось добавить значок «Связи» в трей.",
            ));
        }
        Ok(())
    }
    fn queue(&self, action: TrayAction) {
        self.actions.borrow_mut().push_back(action);
        if action == TrayAction::Open {
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::Visible(true));
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.ctx.request_repaint();
    }
    fn popup(&self, hwnd: HWND) {
        if self.menu_open.replace(true) {
            return;
        }
        // A menu choice is sent to muda's receiver before the native popup returns.
        unsafe {
            SetForegroundWindow(hwnd);
            self.menu.show_context_menu_for_hwnd(hwnd as isize, None);
        }
        self.menu_open.set(false);
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let action = match event.id.0.as_str() {
                "open" => TrayAction::Open,
                "connect" => TrayAction::Connect,
                "repair" => TrayAction::Repair,
                "update" => TrayAction::Update,
                "disconnect" => TrayAction::Disconnect,
                "telegram" => TrayAction::ToggleTelegram,
                "quit" => TrayAction::Quit,
                _ => continue,
            };
            self.queue(action);
        }
    }
}

unsafe extern "system" fn window_event(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    context: usize,
) -> LRESULT {
    // SetWindowSubclass runs on the UI thread. Drop removes it before freeing the boxed callbacks.
    let cb = unsafe { &*(context as *const Callbacks) };
    if msg == CALLBACK && w == ID as usize {
        match l as u32 {
            WM_LBUTTONDBLCLK | NIN_BALLOONUSERCLICK | NIN_KEYSELECT => cb.queue(TrayAction::Open),
            WM_RBUTTONUP | WM_CONTEXTMENU => cb.popup(hwnd),
            _ => (),
        }
        return 0;
    }
    if msg == cb.explorer_started {
        let _ = cb.add_icon(hwnd);
        return 0;
    }
    unsafe { DefSubclassProc(hwnd, msg, w, l) }
}

impl Tray {
    pub fn new(ctx: &egui::Context) -> io::Result<Self> {
        let menu = Menu::new();
        for (id, title) in [
            ("open", "Открыть"),
            ("connect", "Подключить выбранные сервисы"),
            ("repair", "Исправить подключение"),
            ("update", "Проверить обновление «Связи»"),
        ] {
            menu.append(&MenuItem::with_id(id, title, true, None))
                .map_err(io::Error::other)?;
        }
        let strategy = MenuItem::new("Стратегия: —", false, None);
        menu.append(&strategy).map_err(io::Error::other)?;
        for (id, title) in [
            ("disconnect", "Отключить"),
            ("telegram", "Запустить / остановить Telegram"),
        ] {
            menu.append(&MenuItem::with_id(id, title, true, None))
                .map_err(io::Error::other)?;
        }
        menu.append(&PredefinedMenuItem::separator())
            .map_err(io::Error::other)?;
        menu.append(&MenuItem::with_id(
            "quit",
            "Выход (остановить всё)",
            true,
            None,
        ))
        .map_err(io::Error::other)?;
        unsafe {
            let module = GetModuleHandleW(ptr::null());
            // Resource 1 is the app.ico embedded by winresource in build.rs. LR_SHARED is process-owned.
            let icon = LoadImageW(
                module,
                ID as usize as *const u16,
                IMAGE_ICON,
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
                LR_SHARED,
            );
            if icon.is_null() {
                return Err(io::Error::last_os_error());
            }
            let explorer_started = RegisterWindowMessageW(w!("TaskbarCreated"));
            if explorer_started == 0 {
                return Err(io::Error::last_os_error());
            }
            let callbacks = Box::new(Callbacks {
                ctx: ctx.clone(),
                menu,
                menu_open: Cell::new(false),
                actions: RefCell::new(VecDeque::new()),
                icon,
                tooltip: RefCell::new("Связь".into()),
                explorer_started,
            });
            // A hidden top-level window receives Explorer's broadcast as well as tray callbacks.
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("Связь"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                module,
                ptr::null(),
            );
            if window.is_null() {
                return Err(io::Error::last_os_error());
            }
            if SetWindowSubclass(
                window,
                Some(window_event),
                HOOK,
                callbacks.as_ref() as *const Callbacks as usize,
            ) == 0
            {
                let e = io::Error::last_os_error();
                DestroyWindow(window);
                return Err(e);
            }
            let result = Self {
                window,
                callbacks,
                strategy,
            };
            result.callbacks.add_icon(window)?;
            Ok(result)
        }
    }
    pub fn try_next(&self) -> Option<TrayAction> {
        self.callbacks.actions.borrow_mut().pop_front()
    }
    pub fn status(&mut self, strategy: &str, discord: bool, telegram: bool) {
        let label = format!(
            "Стратегия: {}",
            if strategy.is_empty() { "—" } else { strategy }
        );
        if self.strategy.text() != label {
            self.strategy.set_text(label);
        }
        let state = |value| if value { "вкл" } else { "выкл" };
        let tooltip = format!(
            "Связь · Discord: {} · Telegram: {}",
            state(discord),
            state(telegram)
        );
        if *self.callbacks.tooltip.borrow() == tooltip {
            return;
        }
        let mut d = data(self.window);
        d.uFlags = NIF_TIP;
        wide_text(&mut d.szTip, &tooltip);
        if unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) } != 0 {
            *self.callbacks.tooltip.borrow_mut() = tooltip;
        }
    }
    pub fn notify(&self, title: &str, message: &str) {
        let mut d = data(self.window);
        d.uFlags = NIF_INFO;
        d.dwInfoFlags = NIIF_INFO | NIIF_RESPECT_QUIET_TIME;
        wide_text(&mut d.szInfoTitle, title);
        wide_text(&mut d.szInfo, message);
        unsafe {
            Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            RemoveWindowSubclass(self.window, Some(window_event), HOOK);
            Shell_NotifyIconW(NIM_DELETE, &data(self.window));
            DestroyWindow(self.window);
        }
    }
}
