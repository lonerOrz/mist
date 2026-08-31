use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

use crate::domain::to_wide;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
    RegSetValueExW,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const HOTKEY_ID: i32 = 1001;

/// Application configuration structure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,

    #[serde(default = "default_placeholder")]
    pub placeholder: String,

    #[serde(default = "default_max_results")]
    pub max_results: usize,

    #[serde(default = "default_font")]
    pub font_family: String,

    #[serde(default = "default_width")]
    pub width: i32,

    #[serde(default = "default_opacity")]
    pub opacity: f32,

    #[serde(default = "default_corner_radius")]
    pub corner_radius: f32,

    #[serde(default = "default_autostart")]
    pub autostart: bool,

    #[serde(default)]
    pub extra_paths: Vec<PathBuf>,
}

fn default_hotkey() -> String {
    "Alt+Space".into()
}

fn default_placeholder() -> String {
    "Search apps, commands, or calculate...".into()
}

fn default_max_results() -> usize {
    8
}

fn default_font() -> String {
    "Segoe UI Variable Display".into()
}

fn default_width() -> i32 {
    760
}

fn default_opacity() -> f32 {
    0.45
}

fn default_corner_radius() -> f32 {
    8.0
}

fn default_autostart() -> bool {
    false
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            placeholder: default_placeholder(),
            max_results: default_max_results(),
            font_family: default_font(),
            width: default_width(),
            opacity: default_opacity(),
            corner_radius: default_corner_radius(),
            autostart: default_autostart(),
            extra_paths: Vec::new(),
        }
    }
}

impl Config {
    /// Clamps and sanitizes user-provided configuration values within safe bounds.
    pub fn normalized(mut self) -> Self {
        self.width = self.width.clamp(400, 2000);
        self.max_results = self.max_results.clamp(1, 50);
        self.opacity = self.opacity.clamp(0.05, 1.0);
        self.corner_radius = self.corner_radius.clamp(0.0, 20.0);
        self.extra_paths.retain(|p| !p.as_os_str().is_empty());
        self
    }
}

/// Returns the configuration directory path under %USERPROFILE%\.config\mist.
pub fn get_mist_dir() -> PathBuf {
    let base = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    let dir = PathBuf::from(base).join(".config").join("mist");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// Returns the full path to config.toml.
pub fn get_config_path() -> PathBuf {
    get_mist_dir().join("config.toml")
}

/// Synchronizes the login autostart registry entry under HKCU\...\Run.
pub fn sync_autostart(enable: bool) {
    unsafe {
        let Ok(current_exe) = std::env::current_exe() else {
            return;
        };
        let formatted_path = format!("\"{}\"", current_exe.to_string_lossy());
        let wide_val = to_wide(&formatted_path);

        let subkey = windows::core::w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
        let app_name = windows::core::w!("Mist");

        let mut hkey = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey, Some(0), KEY_SET_VALUE, &mut hkey).is_ok() {
            if enable {
                let bytes =
                    std::slice::from_raw_parts(wide_val.as_ptr().cast::<u8>(), wide_val.len() * 2);
                let _ = RegSetValueExW(hkey, app_name, None, REG_SZ, Some(bytes));
            } else {
                let _ = RegDeleteValueW(hkey, app_name);
            }
            let _ = RegCloseKey(hkey);
        }
    }
}

const DEFAULT_CONFIG_TEMPLATE: &str = r#"# Mist Launcher Configuration

# Shortcut hotkey to toggle the launcher window (e.g. "Alt+Space", "Ctrl+Space", "Win+Space")
hotkey = "Alt+Space"

# Placeholder text shown in the search box
placeholder = "Search apps, commands, or calculate..."

# Maximum number of application search results to display
max_results = 8

# UI Font Family name
font_family = "Segoe UI Variable Display"

# Window width in DIPs (height follows the number of results)
width = 760

# Background tint opacity over the acrylic backdrop (0.05 - 1.0)
opacity = 0.45

# Corner radius in pixels (0.0 - 20.0, inner elements scale with it)
corner_radius = 8.0

# Launch Mist automatically at login (managed via HKCU\...\Run)
autostart = false

# Extra directories to index for custom scripts (.bat, .cmd, .ps1) and portable apps
# e.g. extra_paths = ["D:\\Scripts", "C:\\Users\\yourname\\bin"]
extra_paths = []
"#;

impl Config {
    /// Safely loads configuration from disk, creating default file ONLY if it does not exist.
    pub fn load_or_create() -> Self {
        let path = get_config_path();

        if path.exists() {
            match Self::load_from_file(&path) {
                Ok(cfg) => return cfg,
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to parse '{path:?}': {e}. Using fallback defaults without overwriting."
                    );
                    return Config::default();
                }
            }
        }

        let _ = fs::write(&path, DEFAULT_CONFIG_TEMPLATE);
        Config::default()
    }

    /// Parses configuration from an explicit file path.
    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file '{path:?}': {e}"))?;
        toml::from_str::<Config>(&content)
            .map(Config::normalized)
            .map_err(|e| format!("Failed to parse TOML configuration: {e}"))
    }

    /// Spawns a background file watcher that posts WM_CONFIG_RELOADED on config changes.
    pub fn watch_and_notify(hwnd: HWND, msg_id: u32) {
        let dir = get_mist_dir();
        let config_path = get_config_path();
        let target_hwnd_raw = hwnd.0 as isize;

        thread::spawn(move || {
            let (tx, rx) = channel();

            let watcher_res = notify::recommended_watcher(
                move |res: std::result::Result<notify::Event, notify::Error>| {
                    if let Ok(event) = res
                        && (event.kind.is_modify() || event.kind.is_create())
                        && event
                            .paths
                            .iter()
                            .any(|p| p.file_name().is_some_and(|name| name == "config.toml"))
                    {
                        let _ = tx.send(());
                    }
                },
            );

            let mut watcher = match watcher_res {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("Failed to initialize file watcher: {e:?}");
                    return;
                }
            };

            use notify::Watcher;
            if let Err(e) = watcher.watch(&dir, notify::RecursiveMode::NonRecursive) {
                eprintln!("Failed to watch config directory '{dir:?}': {e:?}");
                return;
            }

            while rx.recv().is_ok() {
                thread::sleep(Duration::from_millis(100));
                while rx.try_recv().is_ok() {}

                if let Ok(new_config) = Self::load_from_file(&config_path) {
                    let boxed = Box::new(new_config);
                    let target_hwnd = HWND(target_hwnd_raw as *mut _);
                    let raw = Box::into_raw(boxed);
                    unsafe {
                        if PostMessageW(Some(target_hwnd), msg_id, WPARAM(0), LPARAM(raw as isize))
                            .is_err()
                        {
                            drop(Box::from_raw(raw));
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ranges_and_corner_radius() {
        let cfg = Config {
            width: 50,
            opacity: 9.9,
            corner_radius: 50.0,
            ..Config::default()
        }
        .normalized();
        assert_eq!(cfg.width, 400);
        assert_eq!(cfg.opacity, 1.0);
        assert_eq!(cfg.corner_radius, 20.0);

        let cfg = Config {
            width: 3000,
            opacity: 0.01,
            corner_radius: -3.0,
            ..Config::default()
        }
        .normalized();
        assert_eq!(cfg.width, 2000);
        assert_eq!(cfg.opacity, 0.05);
        assert_eq!(cfg.corner_radius, 0.0);

        let cfg = Config {
            width: 800,
            opacity: 0.5,
            corner_radius: 6.5,
            ..Config::default()
        }
        .normalized();
        assert_eq!(cfg.width, 800);
        assert_eq!(cfg.opacity, 0.5);
        assert_eq!(cfg.corner_radius, 6.5);
    }

    #[test]
    fn defaults_are_normalized() {
        let cfg = Config::default().normalized();
        assert_eq!(cfg.width, 760);
        assert!((cfg.opacity - 0.45).abs() < f32::EPSILON);
        assert!((cfg.corner_radius - 8.0).abs() < f32::EPSILON);
    }
}
