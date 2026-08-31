use crate::domain::{Item, KeyKind, to_wide};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use windows::Win32::System::Com::*;
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::core::*;

/// Executes a closure within a multithreaded COM apartment.
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

/// Resolves the destination target path of a Windows .lnk shortcut.
fn resolve_shortcut_target(lnk_path: &Path) -> Option<String> {
    unsafe {
        let shell_link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist_file: IPersistFile = shell_link.cast().ok()?;

        let wide_path = to_wide(&lnk_path.to_string_lossy());
        persist_file
            .Load(PCWSTR(wide_path.as_ptr()), STGM_READ)
            .ok()?;

        let mut path_buf = [0u16; 1024];
        shell_link
            .GetPath(&mut path_buf, std::ptr::null_mut(), 0)
            .ok()?;

        let len = path_buf
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(path_buf.len());
        let target = String::from_utf16_lossy(&path_buf[..len]);
        if target.is_empty() {
            None
        } else {
            Some(target)
        }
    }
}

/// Computes the canonical physical execution key used for deduplication.
fn get_canonical_target_key(item: &Item) -> String {
    let path_str = item.path.as_ref();
    let lower = path_str.to_lowercase();

    if lower.starts_with("shell:appsfolder\\") {
        return lower;
    }

    let p = Path::new(path_str);
    if lower.ends_with(".lnk")
        && let Some(target) = resolve_shortcut_target(p)
    {
        return target.to_lowercase();
    }

    lower
}

/// Returns execution priority score based on file extension.
fn ext_priority(path: &Path) -> u8 {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("exe") => 5,
        Some("lnk") | Some("appref-ms") => 4,
        Some("cmd") | Some("bat") => 3,
        Some("ps1") => 2,
        Some("msc") | Some("cpl") => 1,
        _ => 0,
    }
}

/// Safely extracts an executable file path without breaking spaces in directory names.
fn sanitize_exe_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(stripped) = trimmed.strip_prefix('"')
        && let Some(end) = stripped.find('"')
    {
        return stripped[..end].to_string();
    }

    let lower = trimmed.to_lowercase();
    for ext in [
        ".exe", ".bat", ".cmd", ".msc", ".cpl", ".ps1", ".vbs", ".lnk",
    ] {
        if let Some(idx) = lower.find(ext) {
            let end = idx + ext.len();
            if end == trimmed.len() || trimmed.as_bytes().get(end) == Some(&b' ') {
                return trimmed[..end].to_string();
            }
        }
    }
    trimmed.to_string()
}

/// Scans directories in system PATH environment variable, keeping highest priority per stem.
fn scan_system_path() -> Vec<Item> {
    let mut dirs_to_scan = Vec::new();

    if let Ok(path_var) = std::env::var("PATH") {
        for dir_str in path_var.split(';') {
            let trimmed = dir_str.trim().trim_matches('"');
            if !trimmed.is_empty() {
                dirs_to_scan.push(PathBuf::from(trimmed));
            }
        }
    }

    let windows_apps = expand_env(r"%LOCALAPPDATA%\Microsoft\WindowsApps");
    let win_apps_path = PathBuf::from(&windows_apps);
    if win_apps_path.is_dir() && !dirs_to_scan.contains(&win_apps_path) {
        dirs_to_scan.push(win_apps_path);
    }

    let mut best_items: HashMap<String, (u8, Item)> = HashMap::new();

    for dir in dirs_to_scan {
        if !dir.is_dir() {
            continue;
        }

        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir()
                && is_valid_executable(&path)
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                let stem_lower = stem.to_lowercase();
                let priority = ext_priority(&path);
                match best_items.get(&stem_lower) {
                    Some((existing_pri, _)) if *existing_pri >= priority => {}
                    _ => {
                        let item = Item::new_application(stem, path.to_string_lossy().as_ref());
                        best_items.insert(stem_lower, (priority, item));
                    }
                }
            }
        }
    }

    best_items.into_values().map(|(_, item)| item).collect()
}

/// Concurrently scans all application sources and deduplicates by target path and name.
pub fn scan_all(extra_paths: &[PathBuf]) -> Vec<Item> {
    let (startmenu, uwp, app_paths, custom, sys_path) = std::thread::scope(|s| {
        let b = s.spawn(|| scoped_com(scan_start_menu_and_desktop));
        let c = s.spawn(|| scoped_com(scan_uwp_apps));
        let d = s.spawn(scan_app_paths);
        let e = s.spawn(|| scan_custom_paths(extra_paths));
        let p = s.spawn(scan_system_path);
        (
            b.join().unwrap_or_default(),
            c.join().unwrap_or_default(),
            d.join().unwrap_or_default(),
            e.join().unwrap_or_default(),
            p.join().unwrap_or_default(),
        )
    });

    let total_hint = startmenu.len() + uwp.len() + app_paths.len() + custom.len() + sys_path.len();
    let mut items: Vec<Item> = Vec::with_capacity(total_hint);
    let mut target_to_idx: HashMap<Box<str>, usize> = HashMap::new();
    let mut seen_names: HashSet<Box<str>> = HashSet::new();

    for source in [startmenu, uwp, app_paths, custom, sys_path] {
        for mut item in source {
            let name_key = item.name.to_lowercase().into_boxed_str();
            let target_key = get_canonical_target_key(&item).into_boxed_str();

            if let Some(&existing_idx) = target_to_idx.get(&target_key) {
                let existing = &mut items[existing_idx];
                let incoming_stem = Path::new(item.path.as_ref())
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&item.name)
                    .to_lowercase();

                if incoming_stem != existing.name.to_lowercase()
                    && !existing
                        .keys
                        .iter()
                        .any(|(_, k)| k.as_ref() == incoming_stem.as_str())
                {
                    let mut keys = existing.keys.as_ref().to_vec();
                    keys.push((KeyKind::Alias, Arc::from(incoming_stem.as_str())));
                    existing.keys = keys.into();
                }
                continue;
            }

            if seen_names.contains(&name_key) {
                continue;
            }

            if item.path.to_lowercase().ends_with(".lnk")
                && let Some(target) = resolve_shortcut_target(Path::new(item.path.as_ref()))
                && let Some(stem) = Path::new(&target).file_stem().and_then(|s| s.to_str())
            {
                let stem_lower = stem.to_lowercase();
                if stem_lower != item.name.to_lowercase()
                    && !item
                        .keys
                        .iter()
                        .any(|(_, k)| k.as_ref() == stem_lower.as_str())
                {
                    let mut keys = item.keys.as_ref().to_vec();
                    keys.push((KeyKind::Alias, Arc::from(stem_lower.as_str())));
                    item.keys = keys.into();
                }
            }

            target_to_idx.insert(target_key, items.len());
            seen_names.insert(name_key);
            items.push(item);
        }
    }

    items.shrink_to_fit();
    items
}

/// Expands environment variable tokens in a path string.
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

/// Scans user-configured custom paths for runnable files.
fn scan_custom_paths(paths: &[PathBuf]) -> Vec<Item> {
    let mut items = Vec::new();
    for p in paths {
        let expanded = expand_env(&p.to_string_lossy());
        let target_dir = Path::new(&expanded);
        if target_dir.is_dir() {
            walk_custom_directory(target_dir, &mut items, 0);
        }
    }
    items
}

/// Recursively scans custom directories up to a depth of 3.
fn walk_custom_directory(dir: &Path, out: &mut Vec<Item>, depth: usize) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_custom_directory(&path, out, depth + 1);
        } else if is_valid_executable(&path)
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            out.push(Item::new_application(stem, path.to_string_lossy().as_ref()));
        }
    }
}

/// Reads a string value from the Windows registry.
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

/// Enumerates registered applications under HKLM and HKCU App Paths.
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
                        let clean_path = sanitize_exe_path(&expanded_exe);
                        if clean_path.is_empty() {
                            continue;
                        }

                        let stem = Path::new(&name)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or(&name)
                            .to_lowercase();

                        if is_valid_executable(Path::new(&clean_path)) {
                            items.push(Item::new_application(&stem, &clean_path));
                        }
                    }
                }
            }
        }
    }
    items
}

/// Enumerates subkey names under a given registry key.
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

/// Validates whether a file path has an executable or runnable script extension.
fn is_valid_executable(path: &Path) -> bool {
    const VALID_EXTS: &[&str] = &[
        "lnk",
        "exe",
        "bat",
        "cmd",
        "msc",
        "cpl",
        "appref-ms",
        "url",
        "ps1",
        "vbs",
    ];

    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    VALID_EXTS.iter().any(|&v| v.eq_ignore_ascii_case(ext))
}

/// Scans user and public Start Menu and Desktop folders.
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

/// Resolves the filesystem path for a Windows Known Folder GUID.
fn known_folder_path(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let path = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = path.to_string().unwrap_or_default();
        CoTaskMemFree(Some(path.0 as *const _));
        (!s.is_empty()).then_some(PathBuf::from(s))
    }
}

/// Recursively traverses a directory up to depth 6 to collect executable files.
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

/// Enumerates packaged UWP applications via shell:AppsFolder.
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

                        let mut uwp_item = Item::new_application(&display_name, &aumid_path);

                        let mut extra_aliases = Vec::new();
                        let aumid_lower = aumid_path.to_lowercase();
                        if aumid_lower.contains("windowsterminal") {
                            extra_aliases.push("wt");
                            extra_aliases.push("terminal");
                        } else if aumid_lower.contains("screensketch")
                            || aumid_lower.contains("snippingtool")
                        {
                            extra_aliases.push("snip");
                            extra_aliases.push("snippingtool");
                        } else if aumid_lower.contains("calculator") {
                            extra_aliases.push("calc");
                        }

                        if !extra_aliases.is_empty() {
                            let mut keys = uwp_item.keys.as_ref().to_vec();
                            for alias in extra_aliases {
                                if !keys.iter().any(|(_, k)| k.as_ref() == alias) {
                                    keys.push((KeyKind::Alias, Arc::from(alias)));
                                }
                            }
                            uwp_item.keys = keys.into();
                        }

                        items.push(uwp_item);
                    }
                }
            }
        }
    }
    items
}
