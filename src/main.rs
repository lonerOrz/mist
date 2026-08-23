#![windows_subsystem = "windows"]

pub mod app;
pub mod config;
pub mod domain;
pub mod history;
pub mod pinyin;
pub mod query;
pub mod renderer;
pub mod search;
pub mod sources;

use app::{App, TIMER_ANIMATION};
use config::Config;
use domain::Item;
use renderer::{Renderer, metrics, window_scale};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

const HOTKEY_ID: i32 = 1001;
const HOTKEY_FALLBACK_ID: i32 = 1002;
const WM_INDEX_READY: u32 = WM_USER + 1;
const WM_CONFIG_RELOADED: u32 = WM_USER + 2;
const TIMER_CARET: usize = 1;

static SURROGATE_PAIR: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

fn parse_hotkey(hotkey_str: &str) -> (HOT_KEY_MODIFIERS, VIRTUAL_KEY) {
    let mut mods = HOT_KEY_MODIFIERS(MOD_NOREPEAT.0);
    let parts: Vec<&str> = hotkey_str.split('+').map(|s| s.trim()).collect();
    let mut vk = VK_SPACE;

    for part in parts {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => mods.0 |= MOD_CONTROL.0,
            "alt" => mods.0 |= MOD_ALT.0,
            "shift" => mods.0 |= MOD_SHIFT.0,
            "win" | "super" => mods.0 |= MOD_WIN.0,
            "space" => vk = VK_SPACE,
            "tab" => vk = VK_TAB,
            _ => {
                if let Some(c) = part.chars().next() {
                    let code = c.to_ascii_uppercase() as u16;
                    if (0x41..=0x5A).contains(&code) || (0x30..=0x39).contains(&code) {
                        vk = VIRTUAL_KEY(code);
                    }
                }
            }
        }
    }
    (mods, vk)
}

fn apply_hotkeys(hwnd: HWND, hotkey_str: &str) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
        let _ = UnregisterHotKey(Some(hwnd), HOTKEY_FALLBACK_ID);

        let (mods, vk) = parse_hotkey(hotkey_str);
        if RegisterHotKey(Some(hwnd), HOTKEY_ID, mods, vk.0 as u32).is_err() {
            let _ = RegisterHotKey(
                Some(hwnd),
                HOTKEY_FALLBACK_ID,
                HOT_KEY_MODIFIERS(MOD_ALT.0 | MOD_NOREPEAT.0),
                VK_SPACE.0 as u32,
            );
        }
    }
}

fn apply_corner(hwnd: HWND, radius: f32) {
    let pref = if radius >= 6.0 {
        DWMWCP_ROUND
    } else if radius > 0.0 {
        DWMWCP_ROUNDSMALL
    } else {
        DWMWCP_DONOTROUND
    };
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            4,
        )
    };
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let _guard = CreateMutexW(None, false, w!("MistLauncherMutex"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(existing) = FindWindowW(w!("MistLauncherClass"), None) {
                let _ = PostMessageW(
                    Some(existing),
                    WM_HOTKEY,
                    WPARAM(HOTKEY_ID as usize),
                    LPARAM(0),
                );
            }
            return Ok(());
        }
    }

    if let Ok(home) = std::env::var("USERPROFILE") {
        let _ = std::env::set_current_dir(home);
    }

    let config = Config::load_or_create();

    let args: Vec<String> = std::env::args().collect();
    let force_show = args
        .iter()
        .any(|arg| arg == "--show" || arg == "-s" || arg == "--test");

    let hwnd = unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;

        let instance = GetModuleHandleW(None)?;
        let class_name = w!("MistLauncherClass");

        let default_cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();

        let wnd_class = WNDCLASSW {
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            hCursor: default_cursor,
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            ..Default::default()
        };

        RegisterClassW(&wnd_class);

        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let scale = GetDpiForSystem() as f32 / 96.0;
        let width = (config.width as f32 * scale).round() as i32;
        let height = (metrics::HEADER_HEIGHT as f32 * scale).round() as i32;
        let x = (screen_w - width) / 2;
        let y = (screen_h - height) / 3;

        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class_name,
            w!("Mist"),
            WS_POPUP,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        )?;

        let use_dark_mode: BOOL = TRUE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &use_dark_mode as *const _ as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );

        apply_corner(hwnd, config.corner_radius);

        const DWMSBT_TRANSIENTWINDOW: i32 = 3;
        let backdrop_type = DWMSBT_TRANSIENTWINDOW;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop_type as *const _ as *const _,
            4,
        );

        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

        apply_hotkeys(hwnd, &config.hotkey);

        hwnd
    };

    let renderer = Renderer::new(config.font_family.clone())?;
    let app = Box::new(App::new(renderer, config));

    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize);
    }

    Config::watch_and_notify(hwnd, WM_CONFIG_RELOADED);

    let hwnd_raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        let items = sources::apps::scan_all();
        let boxed_items = Box::new(items);
        let target_hwnd = HWND(hwnd_raw as *mut _);
        let raw = Box::into_raw(boxed_items);
        unsafe {
            if PostMessageW(
                Some(target_hwnd),
                WM_INDEX_READY,
                WPARAM(0),
                LPARAM(raw as isize),
            )
            .is_err()
            {
                drop(Box::from_raw(raw));
            }
        }
    });

    if force_show {
        unsafe {
            toggle_window(hwnd);
        }
    }

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        CoUninitialize();
    }
    Ok(())
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let app_opt = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App).as_mut() };

    match msg {
        WM_ERASEBKGND => LRESULT(1),

        WM_SETCURSOR => {
            let hit_test = (lparam.0 & 0xffff) as i32;
            if hit_test == 1 {
                let mut pt = POINT::default();
                let _ = unsafe { GetCursorPos(&mut pt) };
                let _ = unsafe { ScreenToClient(hwnd, &mut pt) };

                let s = window_scale(hwnd);
                let cursor_id = if pt.y >= 0
                    && (pt.y as f32) < metrics::HEADER_HEIGHT as f32 * s
                    && pt.x as f32 >= metrics::INPUT_X * s
                {
                    IDC_IBEAM
                } else {
                    IDC_ARROW
                };

                unsafe {
                    let cursor = LoadCursorW(None, cursor_id).unwrap_or_default();
                    SetCursor(Some(cursor));
                }
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        WM_NCCALCSIZE => {
            if wparam.0 != 0 {
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }

        WM_CREATE => {
            unsafe {
                let blink = GetCaretBlinkTime();
                let blink = if blink == 0 { 500 } else { blink };
                let _ = SetTimer(Some(hwnd), TIMER_CARET, blink, None);
            }
            LRESULT(0)
        }

        WM_INDEX_READY => {
            if lparam.0 != 0 {
                let boxed = unsafe { Box::from_raw(lparam.0 as *mut Vec<crate::domain::Item>) };
                if let Some(app) = app_opt {
                    app.set_index(*boxed);
                    app.on_query_change(hwnd);
                }
                unsafe {
                    let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
                }
            }
            LRESULT(0)
        }

        WM_CONFIG_RELOADED => {
            if lparam.0 != 0 {
                let boxed = unsafe { Box::from_raw(lparam.0 as *mut Config) };
                apply_corner(hwnd, boxed.corner_radius);
                apply_hotkeys(hwnd, &boxed.hotkey);
                if let Some(app) = app_opt {
                    app.update_config(hwnd, *boxed);
                }
            }
            LRESULT(0)
        }

        WM_DPICHANGED => {
            if let Some(app) = app_opt {
                let suggested = unsafe { *(lparam.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        suggested.left,
                        suggested.top,
                        suggested.right - suggested.left,
                        suggested.bottom - suggested.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                app.renderer.invalidate();
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
            LRESULT(0)
        }

        WM_HOTKEY => {
            let id = wparam.0 as i32;
            if id == HOTKEY_ID || id == HOTKEY_FALLBACK_ID {
                unsafe {
                    toggle_window(hwnd);
                }
            }
            LRESULT(0)
        }

        WM_CHAR => {
            if let Some(app) = app_opt {
                let code = wparam.0 as u16;
                if (0xD800..=0xDBFF).contains(&code) {
                    SURROGATE_PAIR.store(code, std::sync::atomic::Ordering::Relaxed);
                } else {
                    let full_char = if (0xDC00..=0xDFFF).contains(&code)
                        && SURROGATE_PAIR.load(std::sync::atomic::Ordering::Relaxed) != 0
                    {
                        let high = SURROGATE_PAIR.swap(0, std::sync::atomic::Ordering::Relaxed);
                        char::decode_utf16([high, code]).next().and_then(|r| r.ok())
                    } else {
                        char::from_u32(code as u32)
                    };

                    if let Some(ch) = full_char
                        && !ch.is_control()
                    {
                        app.query.push(ch);
                        app.on_query_change(hwnd);
                    }
                }
            }
            LRESULT(0)
        }

        WM_KEYDOWN => {
            if let Some(app) = app_opt {
                let is_ctrl = (unsafe { GetKeyState(VK_CONTROL.0 as i32) } as u16 & 0x8000) != 0;
                match VIRTUAL_KEY(wparam.0 as u16) {
                    VK_BACK => {
                        if is_ctrl {
                            let trimmed = app.query.trim_end();
                            let cut = trimmed
                                .char_indices()
                                .rfind(|(_, c)| c.is_whitespace())
                                .map_or(0, |(i, c)| i + c.len_utf8());
                            app.query.truncate(cut);
                        } else {
                            app.query.pop();
                        }
                        app.on_query_change(hwnd);
                    }
                    VK_V if is_ctrl => {
                        if let Some(clip) = crate::domain::get_clipboard_text() {
                            let cleaned: String =
                                clip.chars().filter(|c| !c.is_control()).collect();
                            app.query.push_str(&cleaned);
                            app.on_query_change(hwnd);
                        }
                    }
                    VK_UP => {
                        app.move_selection_up(hwnd);
                    }
                    VK_DOWN => {
                        app.move_selection_down(hwnd);
                    }
                    VK_RETURN => {
                        let is_shift =
                            (unsafe { GetKeyState(VK_SHIFT.0 as i32) } as u16 & 0x8000) != 0;
                        if is_shift {
                            app.execute_selected_admin(hwnd);
                        } else {
                            app.execute_selected(hwnd);
                        }
                    }
                    VK_ESCAPE => {
                        app.hide(hwnd);
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }

        WM_KILLFOCUS => {
            if let Some(app) = app_opt {
                app.hide(hwnd);
            }
            LRESULT(0)
        }

        WM_TIMER => {
            if wparam.0 == TIMER_CARET {
                if unsafe { IsWindowVisible(hwnd).as_bool() }
                    && let Some(app) = app_opt
                {
                    app.caret_visible = !app.caret_visible;
                    let s = window_scale(hwnd);
                    let rect = RECT {
                        left: (metrics::INPUT_X * s) as i32,
                        top: 0,
                        right: ((app.config.width - 20) as f32 * s) as i32,
                        bottom: (metrics::HEADER_HEIGHT as f32 * s) as i32,
                    };
                    unsafe {
                        let _ = InvalidateRect(Some(hwnd), Some(&rect), false);
                    }
                }
            } else if wparam.0 == TIMER_ANIMATION
                && let Some(app) = app_opt
            {
                if app.spring_animating {
                    unsafe {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                } else {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_ANIMATION);
                    }
                }
            }
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            if let Some(app) = app_opt {
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32 / window_scale(hwnd);
                app.on_mouse_move(hwnd, y);
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                unsafe {
                    let _ = TrackMouseEvent(&mut tme);
                }
            }
            LRESULT(0)
        }

        WM_MOUSELEAVE => {
            if let Some(app) = app_opt {
                app.hovered = None;
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }

        WM_LBUTTONDOWN => {
            if let Some(app) = app_opt {
                let s = window_scale(hwnd);
                let x = (lparam.0 & 0xffff) as i16 as f32 / s;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32 / s;
                app.on_click(hwnd, x, y);
            }
            LRESULT(0)
        }

        WM_PAINT => {
            if let Some(app) = app_opt {
                app.render_current_frame(hwnd);
            }
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            LRESULT(0)
        }

        WM_DESTROY => {
            unsafe {
                let mut pending = MSG::default();
                while PeekMessageW(
                    &mut pending,
                    Some(hwnd),
                    WM_INDEX_READY,
                    WM_CONFIG_RELOADED,
                    PM_REMOVE,
                )
                .into()
                {
                    if pending.lParam.0 != 0 {
                        match pending.message {
                            WM_INDEX_READY => {
                                drop(Box::from_raw(pending.lParam.0 as *mut Vec<Item>));
                            }
                            WM_CONFIG_RELOADED => {
                                drop(Box::from_raw(pending.lParam.0 as *mut Config));
                            }
                            _ => {}
                        }
                    }
                }

                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_FALLBACK_ID);
                let _ = KillTimer(Some(hwnd), TIMER_CARET);
                let _ = KillTimer(Some(hwnd), TIMER_ANIMATION);
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
                if !ptr.is_null() {
                    drop(Box::from_raw(ptr));
                    let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                PostQuitMessage(0);
            }
            LRESULT(0)
        }

        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn toggle_window(hwnd: HWND) {
    unsafe {
        let is_visible = IsWindowVisible(hwnd).as_bool();
        if is_visible {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
            if !ptr.is_null() {
                let app = &mut *ptr;
                app.hide(hwnd);
            }
        } else {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
            if !ptr.is_null() {
                let app = &mut *ptr;
                app.query.clear();
                app.results.clear();
                app.selected = 0;
                app.hovered = None;
                app.caret_visible = true;
                app.height_spring.reset(metrics::HEADER_HEIGHT as f32);
                app.pill.reset(metrics::LIST_TOP);

                let s = window_scale(hwnd);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    (app.config.width as f32 * s).round() as i32,
                    (metrics::HEADER_HEIGHT as f32 * s).round() as i32,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
                );

                let items: Vec<&Item> = app.results.iter().collect();
                let _ = app.renderer.render(
                    hwnd,
                    &app.query,
                    &app.config.placeholder,
                    &items,
                    app.selected,
                    app.caret_visible,
                    app.hovered,
                    app.pill.current,
                );
            }
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
        }
    }
}
