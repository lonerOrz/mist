use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,

    #[serde(default = "default_placeholder")]
    pub placeholder: String,

    #[serde(default = "default_max_results")]
    pub max_results: usize,

    #[serde(default = "default_true")]
    pub enable_calc: bool,

    #[serde(default = "default_true")]
    pub enable_command: bool,

    #[serde(default = "default_font")]
    pub font_family: String,
}

fn default_hotkey() -> String {
    "Ctrl+Space".into()
}
fn default_placeholder() -> String {
    "Search apps, commands, or calculate...".into()
}
fn default_max_results() -> usize {
    8
}
fn default_true() -> bool {
    true
}
fn default_font() -> String {
    "Segoe UI Variable Display".into()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            placeholder: default_placeholder(),
            max_results: default_max_results(),
            enable_calc: true,
            enable_command: true,
            font_family: default_font(),
        }
    }
}

pub fn get_mist_dir() -> PathBuf {
    let base = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    let dir = PathBuf::from(base).join(".config").join("mist");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn get_config_path() -> PathBuf {
    get_mist_dir().join("config.toml")
}

const DEFAULT_CONFIG_TEMPLATE: &str = r#"# Mist Launcher Configuration

# Shortcut hotkey to toggle the launcher window (e.g. "Ctrl+Space", "Alt+Space", "Win+Space")
hotkey = "Ctrl+Space"

# Placeholder text shown in the search box
placeholder = "Search apps, commands, or calculate..."

# Maximum number of application search results to display
max_results = 8

# Enable inline formula calculator (e.g. 2^10, 1+2*3)
enable_calc = true

# Enable arbitrary command execution fallback
enable_command = true

# UI Font Family name
font_family = "Segoe UI Variable Display"
"#;

impl Config {
    pub fn load_or_create() -> Self {
        let path = get_config_path();

        if let Ok(content) = fs::read_to_string(&path)
            && let Ok(cfg) = toml::from_str::<Config>(&content)
        {
            return cfg;
        }

        let _ = fs::write(&path, DEFAULT_CONFIG_TEMPLATE);
        Config::default()
    }

    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file '{path:?}': {e}"))?;
        toml::from_str::<Config>(&content)
            .map_err(|e| format!("Failed to parse TOML configuration: {e}"))
    }

    /// Watches the config directory and posts `msg_id` (lparam = boxed Config)
    /// to the UI thread whenever config.toml changes. Watches the parent
    /// directory so atomic saves/replacements by editors are detected too.
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
                // Debounce: editors often write files in multiple chunks.
                thread::sleep(Duration::from_millis(100));
                while rx.try_recv().is_ok() {}

                if let Ok(new_config) = Self::load_from_file(&config_path) {
                    let boxed = Box::new(new_config);
                    let target_hwnd = HWND(target_hwnd_raw as *mut _);
                    let ptr = LPARAM(Box::into_raw(boxed) as isize);
                    unsafe {
                        let _ = PostMessageW(Some(target_hwnd), msg_id, WPARAM(0), ptr);
                    }
                }
            }
        });
    }
}
