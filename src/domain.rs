use crate::config;
use pinyin::ToPinyinMulti;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, PostQuitMessage, SW_SHOWNORMAL,
};
use windows::core::*;

/// Encodes a UTF-8 string into a null-terminated UTF-16 wide string vector.
pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Expands %VARIABLE% and ~ tokens into an absolute filesystem path.
pub fn expand_env(s: &str) -> String {
    let mut input = s.to_string();
    if input.starts_with('~')
        && let Ok(home) = std::env::var("USERPROFILE") {
            input = format!("{}{}", home, &input[1..]);
        }
    if !input.contains('%') {
        return input;
    }
    let wide = to_wide(&input);
    let mut buf = [0u16; 1024];
    unsafe {
        let n = ExpandEnvironmentStringsW(PCWSTR(wide.as_ptr()), Some(&mut buf));
        String::from_utf16_lossy(&buf[..n as usize])
            .trim_end_matches('\0')
            .to_string()
    }
}

/// Represents the executable action associated with a search item.
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
    PasteText(Arc<str>),
    PasteFiles(Arc<[PathBuf]>),
    ExitApp,
    OpenConfig,
    RestartApp,
}

impl Action {
    /// Returns true if this action supports elevated (Administrator) execution.
    pub fn supports_admin(&self) -> bool {
        matches!(self, Action::Launch { .. })
    }

    /// Executes the action under normal user privileges.
    pub fn execute(&self) {
        self.run(false);
    }

    /// Executes the action with Administrator privileges if applicable.
    pub fn execute_as_admin(&self) {
        self.run(true);
    }

    /// Internal execution dispatcher.
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
                    } else {
                        let path_lower = path_str.to_lowercase();
                        if path_lower.starts_with("shell:appsfolder\\")
                            || path_lower.ends_with(".lnk")
                            || path_lower.ends_with(".url")
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
                        }
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
            Action::PasteText(text) => {
                crate::clipboard::IS_INTERNAL_COPY.store(true, Ordering::SeqCst);
                set_clipboard(text);
                std::thread::spawn(simulate_paste);
            }
            Action::PasteFiles(paths) => {
                crate::clipboard::IS_INTERNAL_COPY.store(true, Ordering::SeqCst);
                set_clipboard_files(paths);
                std::thread::spawn(simulate_paste);
            }
        }
    }
}

/// RAII guard ensuring the Windows clipboard is closed on drop.
pub struct ClipboardGuard;

impl ClipboardGuard {
    /// Attempts to open the clipboard with retries.
    pub fn open() -> Option<Self> {
        for _ in 0..5 {
            if unsafe { OpenClipboard(None).is_ok() } {
                return Some(ClipboardGuard);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        None
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// Writes Unicode text to the Windows clipboard.
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

/// Reads Unicode text from the Windows clipboard.
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

/// Writes a list of file paths to the Windows clipboard as CF_HDROP format.
pub fn set_clipboard_files(paths: &[PathBuf]) {
    let _guard = match ClipboardGuard::open() {
        Some(g) => g,
        None => return,
    };
    unsafe {
        let _ = EmptyClipboard();
        let mut total_wchars = Vec::new();
        for p in paths {
            total_wchars.extend(to_wide(&p.to_string_lossy()));
        }
        total_wchars.push(0);

        let dropfiles_size = std::mem::size_of::<DROPFILES>();
        let total_bytes = dropfiles_size + total_wchars.len() * 2;

        if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, total_bytes) {
            let ptr = GlobalLock(hmem);
            if !ptr.is_null() {
                let df = ptr as *mut DROPFILES;
                (*df).pFiles = dropfiles_size as u32;
                (*df).fWide = BOOL(1);

                let dest = (ptr as *mut u8).add(dropfiles_size) as *mut u16;
                std::ptr::copy_nonoverlapping(total_wchars.as_ptr(), dest, total_wchars.len());
                let _ = GlobalUnlock(hmem);
                let _ = SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(hmem.0)));
            }
        }
    }
}

/// Simulates a Ctrl+V keystroke to paste into the active foreground window.
pub fn simulate_paste() {
    std::thread::sleep(std::time::Duration::from_millis(45));
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        let inputs = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_CONTROL,
                        ..Default::default()
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_V,
                        ..Default::default()
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_V,
                        dwFlags: KEYEVENTF_KEYUP,
                        ..Default::default()
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_CONTROL,
                        dwFlags: KEYEVENTF_KEYUP,
                        ..Default::default()
                    },
                },
            },
        ];
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Visual item category discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Application,
    Calculator,
    Command,
    Web,
    Path,
    System,
    AppMgmt,
    Clipboard,
}

/// Types of search indexing keys for fuzzy/prefix scoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Name,
    Pinyin,
    Initials,
    Alias,
}

/// Search result item displayed in the launcher list.
#[derive(Debug, Clone)]
pub struct Item {
    pub name: Arc<str>,
    pub path: Arc<str>,
    pub kind: ItemKind,
    pub priority_penalty: i32,
    pub action: Action,
    pub keys: Arc<[(KeyKind, Arc<str>)]>,
}

impl Item {
    /// Creates an application search item with generated Pinyin, Initials, and Alias keys.
    pub fn new_application(name: &str, path: &str) -> Self {
        let name_lower = name.to_lowercase();
        let mut keys: Vec<(KeyKind, Arc<str>)> = Vec::with_capacity(8);
        keys.push((KeyKind::Name, Arc::from(name_lower.as_str())));

        let mut py_variants: Vec<String> = vec![String::new()];
        let mut in_variants: Vec<String> = vec![String::new()];
        let mut has_cjk = false;
        let mut prev_is_alphanumeric = false;

        for c in name.chars() {
            if c.is_ascii_alphanumeric() {
                let lower = c.to_ascii_lowercase();
                for py in &mut py_variants {
                    py.push(lower);
                }
                if !prev_is_alphanumeric {
                    for init in &mut in_variants {
                        init.push(lower);
                    }
                }
                prev_is_alphanumeric = true;
            } else if let Some(multi) = c.to_pinyin_multi() {
                has_cjk = true;
                prev_is_alphanumeric = false;

                let mut py_list = Vec::new();
                let mut init_list = Vec::new();
                for p in multi {
                    let plain = p.plain();
                    if !py_list.contains(&plain) {
                        py_list.push(plain);
                    }
                    if let Some(first) = plain.chars().next()
                        && !init_list.contains(&first)
                    {
                        init_list.push(first);
                    }
                }

                let mut new_py = Vec::new();
                for base in &py_variants {
                    for py in &py_list {
                        if new_py.len() < 8 {
                            let mut s = base.clone();
                            s.push_str(py);
                            new_py.push(s);
                        }
                    }
                }
                py_variants = new_py;

                let mut new_in = Vec::new();
                for base in &in_variants {
                    for init in &init_list {
                        if new_in.len() < 8 {
                            let mut s = base.clone();
                            s.push(*init);
                            new_in.push(s);
                        }
                    }
                }
                in_variants = new_in;
            } else {
                prev_is_alphanumeric = false;
            }
        }

        if has_cjk {
            for py in py_variants {
                if !py.is_empty() && !keys.iter().any(|(_, k)| k.as_ref() == py.as_str()) {
                    keys.push((KeyKind::Pinyin, Arc::from(py.as_str())));
                }
            }
            for init in &in_variants {
                if init.len() >= 2
                    && *init != name_lower
                    && !keys.iter().any(|(_, k)| k.as_ref() == init.as_str())
                {
                    keys.push((KeyKind::Initials, Arc::from(init.as_str())));
                }
            }
        }

        // Always include initials for ASCII text (no multi-tone chars needed)
        if !has_cjk {
            let ascii_initials: String = in_variants
                .iter()
                .filter(|s| s.len() >= 2 && **s != name_lower)
                .cloned()
                .collect();
            if !ascii_initials.is_empty() {
                keys.push((KeyKind::Initials, Arc::from(ascii_initials.as_str())));
            }
        }

        if !path.starts_with("shell:")
            && let Some(stem) = Path::new(path).file_stem().and_then(|s| s.to_str())
        {
            let stem_lower = stem.to_lowercase();
            if stem_lower != name_lower
                && !keys.iter().any(|(_, k)| k.as_ref() == stem_lower.as_str())
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
            keys: keys.into(),
        }
    }

    /// Creates a calculator result item.
    pub fn new_calculator(result: &str) -> Self {
        let res_arc: Arc<str> = Arc::from(result);
        Self {
            name: Arc::from(format!("= {result}")),
            keys: Arc::new([]),
            path: Arc::from("Result (press Enter to copy)"),
            kind: ItemKind::Calculator,
            priority_penalty: 0,
            action: Action::CopyText(res_arc),
        }
    }

    /// Creates a command execution item.
    pub fn new_command(raw_cmd: &str) -> Self {
        let action_str: Arc<str> =
            if Path::new(raw_cmd).is_absolute() && Path::new(raw_cmd).exists() {
                Arc::from(raw_cmd)
            } else {
                Arc::from(format!("cmd.exe /k {raw_cmd}"))
            };
        Self {
            name: Arc::from(format!("Run command: {raw_cmd}")),
            keys: Arc::new([]),
            path: Arc::from(format!("Execute in command prompt: {raw_cmd}")),
            kind: ItemKind::Command,
            priority_penalty: 0,
            action: Action::Launch {
                path: action_str,
                verb: None,
            },
        }
    }

    /// Creates a web search or direct URL item.
    pub fn new_web(name: &str, url: &str) -> Self {
        let url_arc: Arc<str> = Arc::from(url);
        Self {
            name: Arc::from(name),
            path: url_arc.clone(),
            kind: ItemKind::Web,
            priority_penalty: 0,
            action: Action::Launch {
                path: url_arc,
                verb: None,
            },
            keys: Arc::new([]),
        }
    }

    /// Creates a folder / filesystem path navigation item.
    pub fn new_path(name: &str, path: &str) -> Self {
        let path_arc: Arc<str> = Arc::from(path);
        Self {
            name: Arc::from(name),
            path: path_arc.clone(),
            kind: ItemKind::Path,
            priority_penalty: 0,
            action: Action::Launch {
                path: path_arc,
                verb: Some("explore"),
            },
            keys: Arc::new([]),
        }
    }

    /// Creates a clipboard text history entry item.
    pub fn new_clipboard_text(preview_title: Arc<str>, desc: &str, full_text: Arc<str>) -> Self {
        Self {
            name: preview_title,
            path: Arc::from(desc),
            kind: ItemKind::Clipboard,
            priority_penalty: 0,
            action: Action::PasteText(full_text.clone()),
            keys: Arc::new([(KeyKind::Name, full_text)]),
        }
    }

    /// Creates a clipboard files history entry item.
    pub fn new_clipboard_files(summary: Arc<str>, desc: &str, paths: Arc<[PathBuf]>) -> Self {
        Self {
            name: summary.clone(),
            path: Arc::from(desc),
            kind: ItemKind::Clipboard,
            priority_penalty: 0,
            action: Action::PasteFiles(paths),
            keys: Arc::new([(KeyKind::Name, summary)]),
        }
    }

    /// Creates a system power management item.
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
            keys: keys.into(),
        }
    }

    /// Creates an internal Mist management item.
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
            keys: keys.into(),
        }
    }

    /// Checks if this item supports Administrator elevation.
    #[inline]
    pub fn supports_admin(&self) -> bool {
        matches!(self.kind, ItemKind::Application | ItemKind::Command)
            && self.action.supports_admin()
    }

    /// Checks if the query exactly matches the item's primary name key.
    #[inline]
    pub fn is_name_exact(&self, q_lower: &str) -> bool {
        self.keys
            .iter()
            .any(|(kind, key)| *kind == KeyKind::Name && key.as_ref() == q_lower)
    }
}

/// Scored search match pair.
#[derive(Debug, Clone, Copy)]
pub struct Match<'a> {
    pub item: &'a Item,
    pub score: i32,
}
