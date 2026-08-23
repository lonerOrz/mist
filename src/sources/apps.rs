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
    let (startmenu, uwp, path_apps, app_paths) = std::thread::scope(|s| {
        let b = s.spawn(|| scoped_com(scan_start_menu_and_desktop));
        let c = s.spawn(|| scoped_com(scan_uwp_apps));
        let d = s.spawn(scan_env_path);
        let e = s.spawn(scan_app_paths);
        (
            b.join().unwrap_or_default(),
            c.join().unwrap_or_default(),
            d.join().unwrap_or_default(),
            e.join().unwrap_or_default(),
        )
    });

    let mut items = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();
    for source in [startmenu, uwp, path_apps, app_paths] {
        for item in source {
            if seen_keys.insert(item.name.to_lowercase()) {
                items.push(item);
            }
        }
    }
    items
}

fn get_true_windows_path() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut seen = HashSet::new();
    let mut add_dir = |d: PathBuf| {
        if d.is_dir() && seen.insert(d.clone()) {
            dirs.push(d);
        }
    };

    if let Ok(s) = read_reg_string(
        HKEY_LOCAL_MACHINE,
        w!(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
        w!("Path"),
    ) {
        for p in s.split(';') {
            let p = expand_env(p.trim());
            if !p.is_empty() {
                add_dir(PathBuf::from(p));
            }
        }
    }
    if let Ok(s) = read_reg_string(HKEY_CURRENT_USER, w!(r"Environment"), w!("Path")) {
        for p in s.split(';') {
            let p = expand_env(p.trim());
            if !p.is_empty() {
                add_dir(PathBuf::from(p));
            }
        }
    }
    if let Ok(s) = std::env::var("PATH") {
        for p in s.split(';') {
            let p = expand_env(p.trim());
            if !p.is_empty() {
                add_dir(PathBuf::from(p));
            }
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        add_dir(PathBuf::from(local).join(r"Microsoft\WindowsApps"));
    }
    dirs
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

fn scan_env_path() -> Vec<Item> {
    let mut items = Vec::new();
    let pathext = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD;.PS1".into())
        .to_lowercase();
    let valid_exts: HashSet<&str> = pathext
        .split(';')
        .map(|s| s.trim().trim_start_matches('.'))
        .filter(|s| !s.is_empty())
        .collect();

    for dir in get_true_windows_path() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !valid_exts.contains(ext.to_lowercase().as_str()) {
                continue;
            }
            if ext.eq_ignore_ascii_case("exe") && is_cui_image(&path) {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let name_lower = stem.to_lowercase();
            let path_str = path.to_string_lossy().to_string();
            let path_lower = path_str.to_lowercase();
            if is_blacklisted(&name_lower, &path_lower) {
                continue;
            }
            items.push(Item::new_application(stem, &path_str));
        }
    }
    items
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
                        if exe.is_empty() {
                            continue;
                        }
                        let stem = Path::new(&name)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or(&name)
                            .to_lowercase();
                        let path_lower = exe.to_lowercase();
                        if is_blacklisted(&stem, &path_lower)
                            || (Path::new(&exe)
                                .extension()
                                .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
                                && is_cui_image(Path::new(&exe)))
                        {
                            continue;
                        }
                        items.push(Item::new_application(&stem, &exe));
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

fn is_blacklisted(name_lower: &str, path_lower: &str) -> bool {
    const BLACKLIST: &[&str] = &[
        "uninstall",
        "unins000",
        "unins001",
        "vcredist",
        "dxsetup",
        "dotnetfx",
        "installer",
        "setup",
        "updater",
        "update.exe",
        "elevate.exe",
        "crashpad",
        "crash_reporter",
        "error_report",
        "bugreport",
        "helper.exe",
        "daemon.exe",
        "qtwebengineprocess",
        "wow_helper",
        "service.exe",
        "redist",
    ];

    BLACKLIST
        .iter()
        .any(|&bad| name_lower.contains(bad) || path_lower.contains(bad))
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

fn is_cui_image(path: &Path) -> bool {
    read_pe_subsystem(path) == Some(3)
}

fn read_pe_subsystem(path: &Path) -> Option<u16> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 4096];
    let n = std::io::Read::read(&mut file, &mut buf).ok()?;
    pe_subsystem(&buf[..n])
}

fn pe_subsystem(buf: &[u8]) -> Option<u16> {
    if buf.len() < 0x40 || &buf[0..2] != b"MZ" {
        return None;
    }
    let e_lfanew = u32::from_le_bytes(buf[0x3C..0x40].try_into().ok()?) as usize;
    let pe = buf.get(e_lfanew..e_lfanew + 4)?;
    if pe != b"PE\0\0" {
        return None;
    }
    let off = e_lfanew + 24 + 68;
    Some(u16::from_le_bytes(buf.get(off..off + 2)?.try_into().ok()?))
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
        } else if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            if (ext_str == "lnk"
                || ext_str == "exe"
                || ext_str == "url"
                || ext_str == "appref-ms"
                || ext_str == "bat"
                || ext_str == "cmd")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                let name_lower = stem.to_lowercase();
                let path_str = path.to_string_lossy().to_string();
                let path_lower = path_str.to_lowercase();

                if is_blacklisted(&name_lower, &path_lower) {
                    continue;
                }

                out.push(Item::new_application(stem, &path_str));
            }
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

                    let name_lower = display_name.to_lowercase();
                    let path_lower = parsing_path.to_lowercase();

                    if !parsing_path.is_empty() && !is_blacklisted(&name_lower, &path_lower) {
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
