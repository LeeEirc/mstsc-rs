use std::ffi::c_void;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, GetSysColorBrush, UpdateWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL, ES_PASSWORD, GWLP_USERDATA,
    GetClientRect, GetMessageW, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, HMENU,
    IDC_ARROW, LoadCursorW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MSG, MessageBoxW, MoveWindow,
    PostMessageW, PostQuitMessage, RegisterClassW, SW_HIDE, SW_SHOW, SetWindowLongPtrW,
    SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_CREATE, WM_DESTROY, WM_NCCREATE, WM_NCDESTROY, WM_SIZE, WNDCLASSW, WS_CAPTION,
    WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_EX_CLIENTEDGE, WS_MAXIMIZEBOX,
    WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_THICKFRAME, WS_VISIBLE,
};
use windows::core::{Error as WinError, HSTRING, PCWSTR, w};

use crate::config::{SecretString, SessionConfig};
use crate::error::{Error, Result, WindowsContext};

use super::activex::{ActiveXHost, RdpEvent, RdpEventKind};

pub(super) const WM_RDP_EVENT: u32 = WM_APP + 1;
const WM_START_SESSION: u32 = WM_APP + 2;

const ID_SERVER: isize = 1001;
const ID_USERNAME: isize = 1002;
const ID_DOMAIN: isize = 1003;
const ID_PASSWORD: isize = 1004;
const ID_CONNECT: usize = 1005;

const LABEL_WIDTH: i32 = 100;
const EDIT_WIDTH: i32 = 390;
const ROW_HEIGHT: i32 = 26;
const ROW_GAP: i32 = 14;

struct OleGuard;

impl Drop for OleGuard {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

struct WinRtGuard;

impl Drop for WinRtGuard {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

struct ConfigForm {
    all: Vec<HWND>,
    server: HWND,
    username: HWND,
    domain: HWND,
    password: HWND,
    connect: HWND,
}

impl ConfigForm {
    fn create(hwnd: HWND, config: &SessionConfig) -> windows::core::Result<Self> {
        let mut all = Vec::with_capacity(9);
        let labels = ["服务器", "用户名", "域（可选）", "密码"];
        let values = [
            config.server.as_deref().unwrap_or(""),
            config.username.as_deref().unwrap_or(""),
            config.domain.as_deref().unwrap_or(""),
            config
                .password
                .as_ref()
                .map(SecretString::expose)
                .unwrap_or(""),
        ];
        let ids = [ID_SERVER, ID_USERNAME, ID_DOMAIN, ID_PASSWORD];
        let mut edits = Vec::with_capacity(4);

        for index in 0..4 {
            let y = 48 + index as i32 * (ROW_HEIGHT + ROW_GAP);
            let label = create_control(
                hwnd,
                w!("STATIC"),
                &HSTRING::from(labels[index]),
                WINDOW_EX_STYLE::default(),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
                48,
                y + 4,
                LABEL_WIDTH,
                ROW_HEIGHT,
                0,
            )?;
            all.push(label);

            let mut style =
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32);
            if index == 3 {
                style.0 |= ES_PASSWORD as u32;
            }
            let edit = create_control(
                hwnd,
                w!("EDIT"),
                &HSTRING::from(values[index]),
                WS_EX_CLIENTEDGE,
                style,
                48 + LABEL_WIDTH,
                y,
                EDIT_WIDTH,
                ROW_HEIGHT,
                ids[index],
            )?;
            all.push(edit);
            edits.push(edit);
        }

        let connect = create_control(
            hwnd,
            w!("BUTTON"),
            &HSTRING::from("连接"),
            WINDOW_EX_STYLE::default(),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32),
            48 + LABEL_WIDTH + EDIT_WIDTH - 110,
            48 + 4 * (ROW_HEIGHT + ROW_GAP),
            110,
            32,
            ID_CONNECT as isize,
        )?;
        all.push(connect);
        unsafe {
            let _ = SetFocus(Some(if config.server.is_none() {
                edits[0]
            } else if config.username.is_none() {
                edits[1]
            } else {
                edits[3]
            }));
        }

        Ok(Self {
            all,
            server: edits[0],
            username: edits[1],
            domain: edits[2],
            password: edits[3],
            connect,
        })
    }

    fn show(&self, visible: bool) {
        for control in &self.all {
            unsafe {
                let _ = ShowWindow(*control, if visible { SW_SHOW } else { SW_HIDE });
            }
        }
    }

    fn layout(&self, width: i32, _height: i32) {
        let form_width = LABEL_WIDTH + EDIT_WIDTH;
        let left = ((width - form_width) / 2).max(16);
        for index in 0..4 {
            let y = 48 + index * (ROW_HEIGHT + ROW_GAP);
            let label = self.all[index as usize * 2];
            let edit = self.all[index as usize * 2 + 1];
            unsafe {
                let _ = MoveWindow(label, left, y + 4, LABEL_WIDTH, ROW_HEIGHT, true);
                let _ = MoveWindow(
                    edit,
                    left + LABEL_WIDTH,
                    y,
                    EDIT_WIDTH.min(width - left - LABEL_WIDTH - 16).max(80),
                    ROW_HEIGHT,
                    true,
                );
            }
        }
        unsafe {
            let _ = MoveWindow(
                self.connect,
                (left + form_width - 110).min(width - 126).max(16),
                48 + 4 * (ROW_HEIGHT + ROW_GAP),
                110,
                32,
                true,
            );
        }
    }
}

impl Drop for ConfigForm {
    fn drop(&mut self) {
        for control in self.all.drain(..) {
            let _ = unsafe { DestroyWindow(control) };
        }
    }
}

struct AppState {
    hwnd: HWND,
    config: SessionConfig,
    form: Option<ConfigForm>,
    active: Option<ActiveXHost>,
}

struct CreationContext {
    state: *mut AppState,
    adopted: bool,
}

impl AppState {
    fn new(config: SessionConfig) -> Self {
        Self {
            hwnd: HWND::default(),
            config,
            form: None,
            active: None,
        }
    }

    fn ensure_form(&mut self) -> windows::core::Result<()> {
        if self.form.is_none() {
            self.form = Some(ConfigForm::create(self.hwnd, &self.config)?);
        }
        if let Some(form) = &self.form {
            form.show(true);
            self.layout_form();
        }
        Ok(())
    }

    fn layout_form(&self) {
        let Some(form) = &self.form else {
            return;
        };
        let mut rect = RECT::default();
        if unsafe { GetClientRect(self.hwnd, &mut rect) }.is_ok() {
            form.layout(rect.right - rect.left, rect.bottom - rect.top);
        }
    }

    fn begin_session(&mut self) {
        if self.active.is_some() {
            return;
        }
        if let Some(form) = &self.form {
            form.show(false);
        }
        match ActiveXHost::create(self.hwnd, &self.config) {
            Ok(host) => {
                self.active = Some(host);
                // The control has copied/encrypted all credentials. Remove the
                // clear-text copies retained by our config and edit control.
                self.config.password.take();
                self.form.take();
            }
            Err(error) => {
                show_message(
                    Some(self.hwnd),
                    "无法启动远程桌面",
                    &format!("{error}\n\n请检查地址与系统 RDP 组件后重试。"),
                    true,
                );
                if self.ensure_form().is_err() {
                    unsafe {
                        let _ = DestroyWindow(self.hwnd);
                    }
                }
            }
        }
    }

    fn connect_from_form(&mut self) {
        let Some(form) = &self.form else {
            return;
        };
        let server = read_text(form.server);
        let username = read_text(form.username);
        let domain = read_text(form.domain);
        let entered_password = read_text(form.password);

        if server.trim().is_empty() || username.trim().is_empty() {
            show_message(
                Some(self.hwnd),
                "连接信息不完整",
                "请填写服务器地址和用户名。",
                false,
            );
            return;
        }
        let password = if entered_password.is_empty() {
            self.config.password.clone()
        } else {
            Some(SecretString::new(entered_password))
        };
        let Some(password) = password else {
            show_message(
                Some(self.hwnd),
                "连接信息不完整",
                "请填写密码，或通过 --password / --password-env 提供。",
                false,
            );
            return;
        };

        self.config.apply_interactive(
            server.trim().to_owned(),
            username.trim().to_owned(),
            (!domain.trim().is_empty()).then(|| domain.trim().to_owned()),
            password,
        );
        self.begin_session();
    }

    fn resize_active(&self, width: u32, height: u32) {
        let Some(active) = &self.active else {
            self.layout_form();
            return;
        };
        let rect = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        active.resize(rect);
        if self.config.dynamic_resolution {
            active.update_display(width, height);
        }
    }

    fn handle_rdp_event(&mut self, event: RdpEvent) {
        tracing::info!(
            event = ?event.kind,
            arguments = ?event.arguments,
            "RDP control event"
        );
        let server = self.config.server.as_deref().unwrap_or("RDP");
        match event.kind {
            RdpEventKind::Connecting => {
                set_title(self.hwnd, &format!("{server} - 正在连接 - mstsc-rs"));
            }
            RdpEventKind::Connected => {
                set_title(self.hwnd, &format!("{server} - 已连接 - mstsc-rs"));
            }
            RdpEventKind::LoginCompleted => {
                set_title(self.hwnd, &self.config.title);
            }
            RdpEventKind::Disconnected => {
                set_title(self.hwnd, &format!("{server} - 已断开 - mstsc-rs"));
                if !event.arguments.is_empty() && event.arguments.iter().any(|value| value != "0") {
                    show_message(
                        Some(self.hwnd),
                        "远程桌面连接已断开",
                        &event.arguments.join("\n"),
                        true,
                    );
                }
            }
            RdpEventKind::DialogDisplaying => {
                tracing::info!(
                    "the system RDP control is displaying a security or credential dialog"
                );
            }
            RdpEventKind::DialogDismissed
            | RdpEventKind::NetworkStatusChanged
            | RdpEventKind::RemoteDesktopSizeChanged
            | RdpEventKind::StatusChanged => {}
        }
    }
}

pub(super) fn run(config: SessionConfig) -> Result<()> {
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    unsafe { RoInitialize(RO_INIT_SINGLETHREADED) }
        .windows_context("initializing the Windows Runtime apartment")?;
    let _winrt = WinRtGuard;
    unsafe { OleInitialize(None) }.windows_context("initializing OLE")?;
    let _ole = OleGuard;

    let module = unsafe { GetModuleHandleW(None) }.windows_context("getting the module handle")?;
    let instance = HINSTANCE(module.0);
    let class_name = w!("mstsc-rs-main-window");
    let cursor =
        unsafe { LoadCursorW(None, IDC_ARROW) }.windows_context("loading the default cursor")?;
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: cursor,
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        lpszClassName: class_name,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(Error::Windows {
            context: "registering the main window class",
            source: WinError::from_thread(),
        });
    }

    let width = config
        .document
        .get_integer("desktopwidth")
        .unwrap_or(1280)
        .clamp(640, 7680);
    let height = config
        .document
        .get_integer("desktopheight")
        .unwrap_or(800)
        .clamp(480, 4320);
    let title = HSTRING::from(&config.title);
    let state = Box::new(AppState::new(config));
    let state_ptr = Box::into_raw(state);
    let mut creation = CreationContext {
        state: state_ptr,
        adopted: false,
    };
    let style = WINDOW_STYLE(
        WS_OVERLAPPED.0
            | WS_CAPTION.0
            | WS_SYSMENU.0
            | WS_THICKFRAME.0
            | WS_MINIMIZEBOX.0
            | WS_MAXIMIZEBOX.0
            | WS_CLIPCHILDREN.0
            | WS_CLIPSIBLINGS.0,
    );
    let hwnd = match unsafe {
        CreateWindowExW(
            WS_EX_APPWINDOW,
            class_name,
            &title,
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            width,
            height,
            None,
            None,
            Some(instance),
            Some((&mut creation as *mut CreationContext).cast()),
        )
    } {
        Ok(hwnd) => hwnd,
        Err(source) => {
            if !creation.adopted {
                unsafe {
                    drop(Box::from_raw(state_ptr));
                }
            }
            return Err(Error::Windows {
                context: "creating the main window",
                source,
            });
        }
    };

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
    }

    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 == -1 {
            return Err(Error::Windows {
                context: "reading the Windows message queue",
                source: WinError::from_thread(),
            });
        }
        if result.0 == 0 {
            break;
        }

        let handled = unsafe {
            state_from_hwnd(hwnd)
                .and_then(|state| state.active.as_ref())
                .is_some_and(|active| active.translate_accelerator(&message))
        };
        if !handled {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        let creation = unsafe { &mut *(create.lpCreateParams as *mut CreationContext) };
        let state = creation.state;
        creation.adopted = true;
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            (*state).hwnd = hwnd;
        }
        return LRESULT(1);
    }

    let state = unsafe { state_from_hwnd(hwnd) };
    match message {
        WM_CREATE => {
            if let Some(state) = state {
                if state.config.needs_interactive_input() {
                    if let Err(error) = state.ensure_form() {
                        show_message(Some(hwnd), "无法创建连接界面", &error.to_string(), true);
                        return LRESULT(-1);
                    }
                } else {
                    let _ =
                        unsafe { PostMessageW(Some(hwnd), WM_START_SESSION, WPARAM(0), LPARAM(0)) };
                }
            }
            LRESULT(0)
        }
        WM_START_SESSION => {
            if let Some(state) = state {
                state.begin_session();
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wparam.0 & 0xffff;
            if id == ID_CONNECT {
                if let Some(state) = state {
                    state.connect_from_form();
                }
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_SIZE => {
            if let Some(state) = state {
                let width = lparam.0 as u32 & 0xffff;
                let height = (lparam.0 as u32 >> 16) & 0xffff;
                state.resize_active(width, height);
            }
            LRESULT(0)
        }
        WM_RDP_EVENT => {
            if let Some(state) = state {
                let events = state
                    .active
                    .as_ref()
                    .map(ActiveXHost::take_events)
                    .unwrap_or_default();
                for event in events {
                    state.handle_rdp_event(event);
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            if let Some(state) = state
                && let Some(active) = &state.active
            {
                active.disconnect();
            }
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(state) = state {
                state.active.take();
            }
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState };
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            if !pointer.is_null() {
                unsafe {
                    drop(Box::from_raw(pointer));
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

unsafe fn state_from_hwnd(hwnd: HWND) -> Option<&'static mut AppState> {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState };
    unsafe { pointer.as_mut() }
}

#[allow(clippy::too_many_arguments)]
fn create_control(
    parent: HWND,
    class: PCWSTR,
    text: &HSTRING,
    ex_style: WINDOW_EX_STYLE,
    style: WINDOW_STYLE,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    id: isize,
) -> windows::core::Result<HWND> {
    let module = unsafe { GetModuleHandleW(None) }?;
    unsafe {
        CreateWindowExW(
            ex_style,
            class,
            text,
            style,
            x,
            y,
            width,
            height,
            Some(parent),
            Some(HMENU(id as *mut c_void)),
            Some(HINSTANCE(module.0)),
            None,
        )
    }
}

fn read_text(hwnd: HWND) -> String {
    let length = unsafe { GetWindowTextLengthW(hwnd) }.max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..copied])
}

fn set_title(hwnd: HWND, title: &str) {
    let _ = unsafe { SetWindowTextW(hwnd, &HSTRING::from(title)) };
}

fn show_message(hwnd: Option<HWND>, title: &str, message: &str, error: bool) {
    unsafe {
        MessageBoxW(
            hwnd,
            &HSTRING::from(message),
            &HSTRING::from(title),
            MB_OK
                | if error {
                    MB_ICONERROR
                } else {
                    MB_ICONINFORMATION
                },
        );
    }
}
