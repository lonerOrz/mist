use crate::domain::{Item, to_wide};
use crate::search::pinyin_abbr;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use windows::Win32::System::Com::*;
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::core::*;

/// Each scan thread joins the MTA and uninitializes on exit.
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

/// App discovery: Start Menu + Desktop + Admin Tools + UWP + %PATH% + App Paths.
pub fn scan_all() -> Vec<Item> {
    // Parallelize: .lnk COM resolution dominates; registries/PATH are plain IO
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
    let mut seen_keys: HashSet<Arc<str>> = HashSet::new();
    for source in [startmenu, uwp, path_apps, app_paths] {
        for item in source {
            if seen_keys.insert(item.name_lower.clone()) {
                items.push(item);
            }
        }
    }
    items
}

/// Authoritative PATH: registry (HKLM+HKCU Environment) overrides the stale
/// process snapshot inherited from a cross-env launch, plus WindowsApps fallback.
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

/// Expand %VAR% references using the system, since registry PATH strings keep them literal.
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

/// Read a registry string value (default value when value_name is empty).
fn read_reg_string(hkey: HKEY, subkey: PCWSTR, value_name: PCWSTR) -> Result<String> {
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(hkey, subkey, 0, KEY_READ, &mut key).ok()?;
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

/// Scan executables/scripts in PATH dirs (mirrors Win+R). Single-level only.
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
            // CUI .exe spawns a terminal host (the original stray-terminal bug); GUI only
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
            items.push(Item::new_application(stem, &path_str, &pinyin_abbr(stem)));
        }
    }
    items
}

/// Scan the App Paths registry (Win+R's source for wt/chrome/etc.).
fn scan_app_paths() -> Vec<Item> {
    let mut items = Vec::new();
    for hkey in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        if let Some(names) =
            enum_reg_keys(hkey, r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths")
        {
            for name in names {
                let subkey = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{name}");
                let subkey_w = to_wide(&subkey);
                if let Ok(exe) = read_reg_string(hkey, PCWSTR(subkey_w.as_ptr()), w!("")) {
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
                    items.push(Item::new_application(&stem, &exe, &pinyin_abbr(&stem)));
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
        if RegOpenKeyExW(hkey, PCWSTR(sub_w.as_ptr()), 0, KEY_READ, &mut key)
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
                PWSTR(name_buf.as_mut_ptr()),
                &mut len,
                None,
                PWSTR::null(),
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
    // Win32 KnownFolder lookup: no hard-coded drive letters or user-dir paths.
    // AdminTools surfaces Services/Device Manager/Computer Management via native .lnk.
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

/// SHGetKnownFolderPath wrapper (caller supplies FOLDERID_*).
fn known_folder_path(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let path = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = path.to_string().unwrap_or_default();
        CoTaskMemFree(Some(path.0 as *const _));
        (!s.is_empty()).then_some(PathBuf::from(s))
    }
}

/// True if launching this entry forces a console host (Win11 spawns a terminal).
fn targets_console_window(entry: &Path, ext: &str) -> bool {
    match ext {
        "exe" => is_cui_image(entry),
        "lnk" | "appref-ms" => matches!(
            unsafe { lnk_target(entry) },
            Some(target)
            if target.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
                && is_cui_image(&target)
        ),
        _ => false,
    }
}

fn is_cui_image(path: &Path) -> bool {
    read_pe_subsystem(path) == Some(3) // 3 = IMAGE_SUBSYSTEM_WINDOWS_CUI
}

fn read_pe_subsystem(path: &Path) -> Option<u16> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 4096];
    let n = std::io::Read::read(&mut file, &mut buf).ok()?;
    pe_subsystem(&buf[..n])
}

/// Read the PE Subsystem field (same offset in PE32 and PE32+).
/// None means inconclusive; caller conservatively keeps the item.
fn pe_subsystem(buf: &[u8]) -> Option<u16> {
    if buf.len() < 0x40 || &buf[0..2] != b"MZ" {
        return None;
    }
    let e_lfanew = u32::from_le_bytes(buf[0x3C..0x40].try_into().ok()?) as usize;
    let pe = buf.get(e_lfanew..e_lfanew + 4)?;
    if pe != b"PE\0\0" {
        return None;
    }
    // PE sig (4) + COFF header (20), then Optional Header
    let off = e_lfanew + 24 + 68;
    Some(u16::from_le_bytes(buf.get(off..off + 2)?.try_into().ok()?))
}

/// Resolve a .lnk's real target (expands environment variables).
unsafe fn lnk_target(path: &Path) -> Option<PathBuf> {
    unsafe {
        let wide = to_wide(&path.to_string_lossy());

        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        persist.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;

        let mut raw = [0u16; 1024];
        link.GetPath(&mut raw, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
            .ok()?;

        let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
        let raw_str = String::from_utf16_lossy(&raw[..end]);
        if raw_str.is_empty() {
            return None;
        }

        let raw_w = to_wide(&raw_str);
        let mut expanded = [0u16; 1024];
        let n = ExpandEnvironmentStringsW(PCWSTR(raw_w.as_ptr()), Some(&mut expanded)) as usize;

        let expanded = String::from_utf16_lossy(&expanded[..n.min(expanded.len())])
            .trim_end_matches('\0')
            .to_string();
        (!expanded.is_empty()).then_some(PathBuf::from(expanded))
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

                if is_blacklisted(&name_lower, &path_lower)
                    || targets_console_window(&path, &ext_str)
                {
                    continue;
                }

                out.push(Item::new_application(stem, &path_str, &pinyin_abbr(stem)));
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

                        items.push(Item::new_application(
                            &display_name,
                            &aumid_path,
                            &pinyin_abbr(&display_name),
                        ));
                    }
                }
            }
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid PE header buffer
    fn fake_pe(subsystem: u16) -> Vec<u8> {
        let mut buf = vec![0u8; 0x100];
        buf[0..2].copy_from_slice(b"MZ");
        buf[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        buf[0x80..0x84].copy_from_slice(b"PE\0\0");
        let opt = 0x80 + 24;
        buf[opt..opt + 2].copy_from_slice(&0x20Bu16.to_le_bytes()); // PE32+ Magic
        let off = opt + 68;
        buf[off..off + 2].copy_from_slice(&subsystem.to_le_bytes());
        buf
    }

    #[test]
    fn test_pe_subsystem_detection() {
        assert_eq!(pe_subsystem(&fake_pe(2)), Some(2)); // GUI: launch directly
        assert_eq!(pe_subsystem(&fake_pe(3)), Some(3)); // CUI: filtered out
        assert_eq!(pe_subsystem(b"MZ tiny"), None); // invalid: conservatively allowed
    }
}
