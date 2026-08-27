use crate::domain::{Item, to_wide};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use windows::Win32::System::Com::*;
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::core::*;

fn scoped_com<T>(f: impl FnOnce() -> T) -> T {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let r = f();
    unsafe {
        CoUninitialize();
    }
    r
}

pub fn scan_all() -> Vec<Item> {
    let (startmenu, uwp, app_paths) = std::thread::scope(|s| {
        let b = s.spawn(|| scoped_com(scan_start_menu_and_desktop));
        let c = s.spawn(|| scoped_com(scan_uwp_apps));
        let d = s.spawn(scan_app_paths);
        (
            b.join().unwrap_or_default(),
            c.join().unwrap_or_default(),
            d.join().unwrap_or_default(),
        )
    });

    let mut items = Vec::with_capacity(startmenu.len() + uwp.len() + app_paths.len());
    let mut seen_keys: HashSet<Box<str>> = HashSet::new();

    for source in [startmenu, uwp, app_paths] {
        for item in source {
            if seen_keys.insert(item.name.to_lowercase().into_boxed_str()) {
                items.push(item);
            }
        }
    }
    items
}

fn expand_env(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let wide = to_wide(s);
    let mut buf = [0u16; 1024];
    unsafe {
        let n = ExpandEnvironmentStringsW(PCWSTR(wide.as_ptr()), Some(&mut buf));
        String::from_utf16_lossy(&buf[..n as usize])
            .trim_end_matches('\0')
            .to_string()
    }
}

fn read_reg_string(hkey: HKEY, subkey: PCWSTR, value_name: PCWSTR) -> Result<String> {
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(hkey, subkey, Some(0), KEY_READ, &mut key).ok()?;
        let result = (|| -> Result<String> {
            let mut buf = [0u16; 2048];
            let mut size = (buf.len() * 2) as u32;
            RegQueryValueExW(
                key,
                value_name,
                None,
                None,
                Some(buf.as_mut_ptr() as *mut _),
                Some(&mut size),
            )
            .ok()?;
            let len = (size as usize / 2).min(buf.len());
            let raw = String::from_utf16_lossy(&buf[..len]);
            Ok(raw.trim_end_matches('\0').trim_matches('"').to_string())
        })();
        let _ = RegCloseKey(key);
        result
    }
}

fn scan_app_paths() -> Vec<Item> {
    let mut items = Vec::new();
    for hkey in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let subkeys: &[&str] = if hkey == HKEY_LOCAL_MACHINE {
            &[
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths",
                r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths",
            ]
        } else {
            &[r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths"]
        };
        for subkey in subkeys {
            if let Some(names) = enum_reg_keys(hkey, subkey) {
                for name in names {
                    let val_path = format!(r"{subkey}\{name}");
                    let val_w = to_wide(&val_path);
                    if let Ok(exe) = read_reg_string(hkey, PCWSTR(val_w.as_ptr()), w!("")) {
                        let expanded_exe = expand_env(&exe);
                        if expanded_exe.is_empty() {
                            continue;
                        }
                        let stem = Path::new(&name)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or(&name)
                            .to_lowercase();
                        if is_valid_executable(Path::new(&expanded_exe)) {
                            items.push(Item::new_application(&stem, &expanded_exe));
                        }
                    }
                }
            }
        }
    }
    items
}

fn enum_reg_keys(hkey: HKEY, subkey: &str) -> Option<Vec<String>> {
    unsafe {
        let sub_w = to_wide(subkey);
        let mut key = HKEY::default();
        if RegOpenKeyExW(hkey, PCWSTR(sub_w.as_ptr()), Some(0), KEY_READ, &mut key)
            .ok()
            .is_err()
        {
            return None;
        }
        let mut out = Vec::new();
        let mut i = 0u32;
        loop {
            let mut name_buf = [0u16; 256];
            let mut len = name_buf.len() as u32;
            if RegEnumKeyExW(
                key,
                i,
                Some(PWSTR(name_buf.as_mut_ptr())),
                &mut len,
                None,
                Some(PWSTR::null()),
                None,
                None,
            )
            .ok()
            .is_err()
            {
                break;
            }
            out.push(String::from_utf16_lossy(&name_buf[..len as usize]));
            i += 1;
        }
        let _ = RegCloseKey(key);
        Some(out)
    }
}

/// Lightweight extension check — no disk I/O or COM resolution
fn is_valid_executable(path: &Path) -> bool {
    const VALID_EXTS: &[&str] = &["lnk", "exe", "bat", "cmd", "msc", "cpl", "appref-ms", "url"];

    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    VALID_EXTS.iter().any(|&v| v.eq_ignore_ascii_case(ext))
}

fn scan_start_menu_and_desktop() -> Vec<Item> {
    let mut items = Vec::new();
    for id in [
        &FOLDERID_CommonStartMenu,
        &FOLDERID_StartMenu,
        &FOLDERID_PublicDesktop,
        &FOLDERID_Desktop,
        &FOLDERID_CommonAdminTools,
        &FOLDERID_AdminTools,
    ] {
        if let Some(dir) = known_folder_path(id) {
            walk_directory(&dir, &mut items, 0);
        }
    }
    items
}

fn known_folder_path(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let path = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = path.to_string().unwrap_or_default();
        CoTaskMemFree(Some(path.0 as *const _));
        (!s.is_empty()).then_some(PathBuf::from(s))
    }
}

fn walk_directory(dir: &Path, out: &mut Vec<Item>, depth: usize) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_directory(&path, out, depth + 1);
        } else if is_valid_executable(&path)
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            out.push(Item::new_application(stem, path.to_string_lossy().as_ref()));
        }
    }
}

fn scan_uwp_apps() -> Vec<Item> {
    let mut items = Vec::new();
    unsafe {
        let apps_folder: IShellItem =
            match SHCreateItemFromParsingName(w!("shell:AppsFolder"), None) {
                Ok(item) => item,
                Err(_) => return items,
            };

        let enum_items: IEnumShellItems = match apps_folder.BindToHandler(None, &BHID_EnumItems) {
            Ok(e) => e,
            Err(_) => return items,
        };

        let mut fetched_count = 0u32;
        let mut fetched = [None::<IShellItem>; 1];

        while enum_items
            .Next(&mut fetched, Some(&mut fetched_count))
            .is_ok()
        {
            if fetched_count == 0 {
                break;
            }
            if let Some(item) = fetched[0].take()
                && let Ok(name_ptr) = item.GetDisplayName(SIGDN_NORMALDISPLAY)
            {
                let display_name = name_ptr.to_string().unwrap_or_default();
                CoTaskMemFree(Some(name_ptr.0 as *const _));

                if display_name.is_empty() {
                    continue;
                }

                if let Ok(path_ptr) = item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING) {
                    let parsing_path = path_ptr.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(path_ptr.0 as *const _));

                    let path_lower = parsing_path.to_lowercase();

                    if !parsing_path.is_empty() {
                        let aumid_path = if path_lower.starts_with("shell:appsfolder\\") {
                            parsing_path
                        } else {
                            format!(r"shell:AppsFolder\{parsing_path}")
                        };

                        items.push(Item::new_application(&display_name, &aumid_path));
                    }
                }
            }
        }
    }
    items
}
