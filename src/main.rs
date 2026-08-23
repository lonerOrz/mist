#![windows_subsystem = "windows"]

pub mod app;
pub mod calc;
pub mod domain;
pub mod history;
pub mod indexer;
pub mod renderer;
pub mod search;

use app::{App, TIMER_ANIMATION};
use domain::Item;
use renderer::{Renderer, metrics};
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
const TIMER_CARET: usize = 1;

static SURROGATE_PAIR: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let _guard = CreateMutexW(None, false, w!("SeliLauncherMutex"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(existing) = FindWindowW(w!("SeliLauncherClass"), None) {
                let _ = PostMessageW(existing, WM_HOTKEY, WPARAM(HOTKEY_ID as usize), LPARAM(0));
            }
            return Ok(());
        }
    }

    if let Ok(home) = std::env::var("USERPROFILE") {
        let _ = std::env::set_current_dir(home);
    }

    let args: Vec<String> = std::env::args().collect();
    let force_show = args
        .iter()
        .any(|arg| arg == "--show" || arg == "-s" || arg == "--test");

    let hwnd = unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;

        let instance = GetModuleHandleW(None)?;
        let class_name = w!("SeliLauncherClass");

        let wnd_class = WNDCLASSW {
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            ..Default::default()
        };

        RegisterClassW(&wnd_class);

        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let width = metrics::WINDOW_WIDTH;
        let height = metrics::HEADER_HEIGHT;
        let x = (screen_w - width) / 2;
        let y = (screen_h - height) / 3;

        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class_name,
            w!("Seli Launcher"),
            WS_POPUP,
            x,
            y,
            width,
            height,
            None,
            None,
            instance,
            None,
        )?;

        let use_dark_mode: BOOL = TRUE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &use_dark_mode as *const _ as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );

        let corner_pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner_pref as *const _ as *const _,
            4,
        );

        // DWMSBT_TRANSIENTWINDOW = 3 (Acrylic 材质，搜索框专属磨砂玻璃效果)
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

        // 修复 unused_mut 警告
        let reg = RegisterHotKey(
            hwnd,
            HOTKEY_ID,
            HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_NOREPEAT.0),
            VK_SPACE.0 as u32,
        );
        if reg.is_err() {
            let _ = RegisterHotKey(
                hwnd,
                HOTKEY_FALLBACK_ID,
                HOT_KEY_MODIFIERS(MOD_ALT.0 | MOD_NOREPEAT.0),
                VK_SPACE.0 as u32,
            );
        }

        hwnd
    };

    let renderer = Renderer::new()?;
    let app = Box::new(App::new(renderer));

    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize);
    }

    let hwnd_raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        let items = indexer::scan_all();
        let boxed_items = Box::new(items);
        let target_hwnd = HWND(hwnd_raw as *mut _);
        let ptr = LPARAM(Box::into_raw(boxed_items) as isize);
        unsafe {
            let _ = PostMessageW(target_hwnd, WM_INDEX_READY, WPARAM(0), ptr);
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

        WM_NCCALCSIZE => {
            if wparam.0 != 0 {
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) } // 修复 E0133
            }
        }

        WM_CREATE => {
            unsafe {
                let _ = SetTimer(hwnd, TIMER_CARET, 1500, None);
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
                            let cut = trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1);
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
                    let rect = RECT {
                        left: 40,
                        top: 10,
                        right: metrics::WINDOW_WIDTH - 20,
                        bottom: 48,
                    };
                    unsafe {
                        let _ = InvalidateRect(hwnd, Some(&rect), false);
                    }
                }
            } else if wparam.0 == TIMER_ANIMATION
                && let Some(app) = app_opt
            {
                if app.spring_animating {
                    unsafe {
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                } else {
                    unsafe {
                        let _ = KillTimer(hwnd, TIMER_ANIMATION);
                    }
                }
            }
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            if let Some(app) = app_opt {
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
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
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }

        WM_LBUTTONDOWN => {
            if let Some(app) = app_opt {
                let x = (lparam.0 & 0xffff) as i16 as f32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
                app.on_click(hwnd, x, y);
            }
            LRESULT(0)
        }

        WM_PAINT => {
            if let Some(app) = app_opt {
                app.render_current_frame(hwnd);
            }
            unsafe {
                let _ = ValidateRect(hwnd, None);
            }
            LRESULT(0)
        }

        WM_DESTROY => {
            unsafe {
                let _ = UnregisterHotKey(hwnd, HOTKEY_ID);
                let _ = UnregisterHotKey(hwnd, HOTKEY_FALLBACK_ID);
                let _ = KillTimer(hwnd, TIMER_CARET);
                let _ = KillTimer(hwnd, TIMER_ANIMATION);
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
                if !ptr.is_null() {
                    drop(Box::from_raw(ptr));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
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
                app.height_spring.reset(metrics::HEADER_HEIGHT as f32);
                app.pill.reset(metrics::LIST_TOP);

                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    metrics::WINDOW_WIDTH,
                    metrics::HEADER_HEIGHT,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
                );

                let items: Vec<&Item> = app.results.iter().collect();
                let _ = app.renderer.render(
                    hwnd,
                    &app.query,
                    &items,
                    app.selected,
                    app.caret_visible,
                    app.hovered,
                    app.pill.current,
                );
            }
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(hwnd);
        }
    }
}
