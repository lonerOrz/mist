use crate::config;
use pinyin::ToPinyinMulti;
use std::path::Path;
use std::sync::Arc;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, PostQuitMessage, SW_SHOWNORMAL,
};
use windows::core::*;

pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Launch {
        path: Arc<str>,
        verb: Option<&'static str>,
    },
    LockScreen,
    SleepSystem,
    ShutdownSystem,
    RestartSystem,
    CopyText(Arc<str>),
    ExitApp,
    OpenConfig,
    RestartApp,
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
            Action::LockScreen => unsafe {
                let _ = LockWorkStation();
            },
            Action::SleepSystem => unsafe {
                let _ = windows::Win32::System::Power::SetSuspendState(false, true, false);
            },
            Action::ShutdownSystem => {
                let _ = std::process::Command::new("shutdown.exe")
                    .args(["/s", "/t", "0"])
                    .spawn();
            }
            Action::RestartSystem => {
                let _ = std::process::Command::new("shutdown.exe")
                    .args(["/r", "/t", "0"])
                    .spawn();
            }
            Action::OpenConfig => {
                let path = config::get_config_path();
                let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
            }
            Action::RestartApp => {
                if let Ok(exe) = std::env::current_exe() {
                    let current_pid = std::process::id();
                    use std::os::windows::process::CommandExt;
                    let _ = std::process::Command::new(&exe)
                        .arg("--restarted-from")
                        .arg(current_pid.to_string())
                        .creation_flags(0x00000008 | 0x00000200)
                        .spawn();
                }
                unsafe {
                    PostQuitMessage(0);
                }
            }
            Action::Launch { path, verb } => {
                let path_str = &**path;
                unsafe {
                    let _ = AllowSetForegroundWindow(0xFFFFFFFF);
                }

                let (file, params, working_dir) =
                    if let Some(cmd) = path_str.strip_prefix("cmd.exe /k ") {
                        ("cmd.exe", format!("/k {cmd}"), String::new())
                    } else if path_str.starts_with("shell:AppsFolder\\")
                        || path_str.to_lowercase().ends_with(".lnk")
                        || path_str.to_lowercase().ends_with(".url")
                    {
                        (path_str, String::new(), String::new())
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

                let verb_holder;
                let verb = if let Some(v) = verb {
                    verb_holder = to_wide(v);
                    PCWSTR(verb_holder.as_ptr())
                } else if as_admin {
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
            Action::CopyText(text) => set_clipboard(text),
        }
    }
}

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
                let delivered = matches!(SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hmem.0))), Ok(h) if !h.0.is_null());
                if !delivered {
                    let _ = GlobalFree(Some(hmem));
                }
            } else {
                let _ = GlobalFree(Some(hmem));
            }
        }
    }
}

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
        let max_len = GlobalSize(hmem) / std::mem::size_of::<u16>();
        let mut len = 0usize;
        while len < max_len && *ptr.add(len) != 0 {
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
    Web,
    Path,
    System,
    AppMgmt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Name,
    Pinyin,
    Initials,
    Alias,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub name: Arc<str>,
    pub path: Arc<str>,
    pub kind: ItemKind,
    pub priority_penalty: i32,
    pub action: Action,
    pub keys: Box<[(KeyKind, Arc<str>)]>,
}

impl Item {
    pub fn new_application(name: &str, path: &str) -> Self {
        let name_lower = name.to_lowercase();
        let mut keys: Vec<(KeyKind, Arc<str>)> =
            vec![(KeyKind::Name, Arc::from(name_lower.as_str()))];

        let mut pinyin_full = String::new();
        let mut pinyin_initials = String::new();
        let mut has_cjk = false;

        for c in name.chars() {
            if c.is_ascii_alphanumeric() {
                let lower = c.to_ascii_lowercase();
                pinyin_full.push(lower);
                pinyin_initials.push(lower);
            } else if let Some(multi) = c.to_pinyin_multi() {
                has_cjk = true;
                let mut char_pinyins = Vec::new();
                let mut char_initials = Vec::new();
                for py in multi {
                    let plain = py.plain();
                    char_pinyins.push(plain.to_string());
                    if let Some(first) = plain.chars().next() {
                        char_initials.push(first);
                    }
                }
                pinyin_full.push_str(&char_pinyins.join(" "));
                pinyin_full.push(' ');
                char_initials.sort();
                char_initials.dedup();
                pinyin_initials.push_str(&char_initials.iter().collect::<String>());
                pinyin_initials.push(' ');
            }
        }

        if has_cjk {
            if !pinyin_full.is_empty() {
                keys.push((KeyKind::Pinyin, Arc::from(pinyin_full.trim_end())));
            }
            if !pinyin_initials.is_empty() {
                keys.push((KeyKind::Initials, Arc::from(pinyin_initials)));
            }
        }

        if !path.starts_with("shell:")
            && let Some(stem) = Path::new(path).file_stem().and_then(|s| s.to_str())
        {
            let stem_lower = stem.to_lowercase();
            if stem_lower != name_lower
                && !keys
                    .iter()
                    .any(|(_, key)| key.as_ref() == stem_lower.as_str())
            {
                keys.push((KeyKind::Alias, Arc::from(stem_lower)));
            }
        }
        let path_arc: Arc<str> = Arc::from(path);
        Self {
            name: Arc::from(name),
            path: path_arc.clone(),
            kind: ItemKind::Application,
            priority_penalty: 0,
            action: Action::Launch {
                path: path_arc,
                verb: None,
            },
            keys: keys.into_boxed_slice(),
        }
    }

    pub fn new_calculator(result: &str) -> Self {
        let res_arc: Arc<str> = Arc::from(result);
        Self {
            name: Arc::from(format!("= {result}")),
            keys: Box::new([]),
            path: Arc::from("Result (press Enter to copy)"),
            kind: ItemKind::Calculator {
                result: res_arc.clone(),
            },
            priority_penalty: 0,
            action: Action::CopyText(res_arc),
        }
    }

    pub fn new_command(raw_cmd: &str) -> Self {
        let action_str: Arc<str> = if Path::new(raw_cmd).is_absolute() && Path::new(raw_cmd).exists() {
            Arc::from(raw_cmd)
        } else {
            Arc::from(format!("cmd.exe /k {raw_cmd}"))
        };
        Self {
            name: Arc::from(format!("Run command: {raw_cmd}")),
            keys: Box::new([]),
            path: Arc::from(format!("Execute in command prompt: {raw_cmd}")),
            kind: ItemKind::Command {
                raw: Arc::from(raw_cmd),
            },
            priority_penalty: 0,
            action: Action::Launch {
                path: action_str,
                verb: None,
            },
        }
    }

    pub fn new_system(name: &str, cmd: &str, action: Action, aliases: &[&str]) -> Self {
        let mut keys = vec![(KeyKind::Name, Arc::from(cmd))];
        for alias in aliases {
            keys.push((KeyKind::Alias, Arc::from(*alias)));
        }
        Self {
            name: Arc::from(name),
            path: Arc::from(cmd),
            kind: ItemKind::System,
            priority_penalty: 0,
            action,
            keys: keys.into_boxed_slice(),
        }
    }

    pub fn new_app_mgmt(name: &str, cmd: &str, action: Action, aliases: &[&str]) -> Self {
        let mut keys = vec![(KeyKind::Name, Arc::from(cmd))];
        for alias in aliases {
            keys.push((KeyKind::Alias, Arc::from(*alias)));
        }
        Self {
            name: Arc::from(name),
            path: Arc::from(cmd),
            kind: ItemKind::AppMgmt,
            priority_penalty: 0,
            action,
            keys: keys.into_boxed_slice(),
        }
    }

    #[inline]
    pub fn is_name_exact(&self, q_lower: &str) -> bool {
        self.keys
            .iter()
            .any(|(kind, key)| *kind == KeyKind::Name && key.as_ref() == q_lower)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Match<'a> {
    pub item: &'a Item,
    pub score: i32,
}
