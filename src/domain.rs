use crate::config;
use std::path::Path;
use std::sync::Arc;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, PostQuitMessage, SW_SHOWNORMAL,
};
use windows::core::*;

/// UTF-16 with NUL terminator for Win32 APIs.
pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// NUL-free variant for DirectWrite text slices.
pub(crate) fn to_wide_slice(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Launch(Arc<str>),
    CopyText(Arc<str>),
    ExitApp,
}

impl Action {
    pub fn execute(&self) {
        self.run(false);
    }

    pub fn execute_as_admin(&self) {
        self.run(true);
    }

    fn run(&self, as_admin: bool) {
        match self {
            Action::ExitApp => unsafe {
                PostQuitMessage(0);
            },
            Action::Launch(path) => {
                let path_str = &**path;

                // Allow the newly launched process to steal foreground focus
                unsafe {
                    let _ = AllowSetForegroundWindow(0xFFFFFFFF);
                }

                // cmd.exe /k or /c command
                let (file, params, working_dir) =
                    if let Some(cmd) = path_str.strip_prefix("cmd.exe /k ") {
                        ("cmd.exe", format!("/k {cmd}"), String::new())
                    } else if let Some(cmd) = path_str.strip_prefix("cmd.exe /c ") {
                        ("cmd.exe", format!("/c {cmd}"), String::new())
                    // shell: / .lnk / .url — resolved natively
                    } else if path_str.starts_with("shell:AppsFolder\\")
                        || path_str.to_lowercase().ends_with(".lnk")
                        || path_str.to_lowercase().ends_with(".url")
                    {
                        (path_str, String::new(), String::new())
                    // standalone file (parent dir as CWD; UNC/empty → process CWD)
                    } else {
                        let target_path = Path::new(path_str);
                        let dir = target_path
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .filter(|d| !d.is_empty() && !d.starts_with(r"\\"))
                            .unwrap_or_default();
                        (path_str, String::new(), dir)
                    };

                let file_w = to_wide(file);
                let params_w = if params.is_empty() {
                    Vec::new()
                } else {
                    to_wide(&params)
                };
                let dir_w = to_wide(&working_dir);

                // NULL verb = default; "runas" only when elevating
                let verb = if as_admin {
                    w!("runas")
                } else {
                    PCWSTR::null()
                };

                let mut exec_info = SHELLEXECUTEINFOW {
                    cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                    fMask: SEE_MASK_NOZONECHECKS,
                    hwnd: HWND::default(),
                    lpVerb: verb,
                    lpFile: PCWSTR(file_w.as_ptr()),
                    lpParameters: if params.is_empty() {
                        PCWSTR::null()
                    } else {
                        PCWSTR(params_w.as_ptr())
                    },
                    lpDirectory: if working_dir.is_empty() {
                        PCWSTR::null()
                    } else {
                        PCWSTR(dir_w.as_ptr())
                    },
                    nShow: SW_SHOWNORMAL.0,
                    ..Default::default()
                };

                unsafe {
                    if let Err(e) = ShellExecuteExW(&mut exec_info) {
                        eprintln!("ShellExecuteExW failed: {e:?}, file: {file}");
                    }
                }
            }
            Action::CopyText(text) => {
                set_clipboard(text);
            }
        }
    }
}

/// Opens the clipboard and closes it on drop, so a failed path can't leak it open.
pub struct ClipboardGuard;

impl ClipboardGuard {
    pub fn open() -> Option<Self> {
        unsafe { OpenClipboard(None).ok().map(|_| ClipboardGuard) }
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn set_clipboard(text: &str) {
    let _guard = match ClipboardGuard::open() {
        Some(g) => g,
        None => return,
    };
    unsafe {
        let _ = EmptyClipboard();
        let wide = to_wide(text);
        let size = wide.len() * std::mem::size_of::<u16>();
        if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, size) {
            let ptr = GlobalLock(hmem);
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, ptr as *mut u8, size);
                let _ = GlobalUnlock(hmem);
                // free only if the clipboard didn't take ownership
                let delivered = matches!(
                    SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hmem.0))),
                    Ok(h) if !h.0.is_null()
                );
                if !delivered {
                    let _ = GlobalFree(Some(hmem));
                }
            } else {
                let _ = GlobalFree(Some(hmem));
            }
        }
    }
}

/// Read clipboard text; always closes the clipboard on exit.
pub(crate) fn get_clipboard_text() -> Option<String> {
    let _guard = ClipboardGuard::open()?;
    unsafe {
        let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
        if handle.0.is_null() {
            return None;
        }
        let hmem = HGLOBAL(handle.0 as _);
        let ptr = GlobalLock(hmem) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
        let _ = GlobalUnlock(hmem);
        Some(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemKind {
    Application,
    Calculator { result: Arc<str> },
    Command { raw: Arc<str> },
    Config,
    Exit,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub name: Arc<str>,
    pub name_lower: Arc<str>,
    /// Extra search keywords (aliases), separate from name_lower.
    pub keywords_lower: Arc<str>,
    pub pinyin_abbr: Arc<str>,
    pub path: Arc<str>,
    pub kind: ItemKind,
    pub priority_penalty: i32,
    pub action: Action,
}

impl Item {
    /// Indexed entry: derived fields set consistently.
    pub fn new_application(name: &str, path: &str, pinyin_abbr: &str) -> Self {
        let path_arc: Arc<str> = Arc::from(path);
        Self {
            name: Arc::from(name),
            name_lower: Arc::from(name.to_lowercase()),
            keywords_lower: Arc::from(""),
            pinyin_abbr: Arc::from(pinyin_abbr),
            path: path_arc.clone(),
            kind: ItemKind::Application,
            priority_penalty: 0,
            action: Action::Launch(path_arc),
        }
    }

    /// Calculator result is display-only, excluded from search.
    pub fn new_calculator(result: &str) -> Self {
        let res_arc: Arc<str> = Arc::from(result);
        Self {
            name: Arc::from(format!("= {result}")),
            name_lower: Arc::from(""),
            keywords_lower: Arc::from(""),
            pinyin_abbr: Arc::from(""),
            path: Arc::from("Result (press Enter to copy)"),
            kind: ItemKind::Calculator {
                result: res_arc.clone(),
            },
            priority_penalty: 0,
            action: Action::CopyText(res_arc),
        }
    }

    /// Command fallback: native if path exists, else via cmd (/c, "|| pause" on failure).
    pub fn new_command(raw_cmd: &str) -> Self {
        let action_str: Arc<str> = if Path::new(raw_cmd).exists() {
            Arc::from(raw_cmd)
        } else {
            Arc::from(format!("cmd.exe /c {raw_cmd} || pause"))
        };
        Self {
            name: Arc::from(format!("Run command: {raw_cmd}")),
            name_lower: Arc::from(""),
            keywords_lower: Arc::from(""),
            pinyin_abbr: Arc::from(""),
            path: Arc::from(format!("Execute in command prompt: {raw_cmd}")),
            kind: ItemKind::Command {
                raw: Arc::from(raw_cmd),
            },
            priority_penalty: 0,
            action: Action::Launch(action_str),
        }
    }

    pub fn new_config() -> Self {
        let cfg_path = config::get_config_path();
        let path_str = cfg_path.to_string_lossy().to_string();
        let path_arc: Arc<str> = Arc::from(path_str.as_str());
        Self {
            name: Arc::from("Open Config (config.toml)"),
            name_lower: Arc::from("config configuration settings preference"),
            keywords_lower: Arc::from("options"),
            pinyin_abbr: Arc::from(""),
            path: path_arc.clone(),
            kind: ItemKind::Config,
            priority_penalty: 0,
            action: Action::Launch(path_arc),
        }
    }

    pub fn new_exit() -> Self {
        Self {
            name: Arc::from("Exit Mist"),
            name_lower: Arc::from("exit mist quit"),
            keywords_lower: Arc::from(":q close"),
            pinyin_abbr: Arc::from(""),
            path: Arc::from("Quit the launcher process"),
            kind: ItemKind::Exit,
            priority_penalty: 0,
            action: Action::ExitApp,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Match<'a> {
    pub item: &'a Item,
    pub score: i32,
}
