use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::*;
use windows::Win32::UI::Shell::*;
use windows::core::*;

pub static IS_INTERNAL_COPY: AtomicBool = AtomicBool::new(false);
static ENTRY_ID_GEN: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardPayload {
    Text {
        full_text: Arc<str>,
        preview_title: Arc<str>,
        line_count: usize,
        char_count: usize,
    },
    Files {
        paths: Arc<[PathBuf]>,
        summary: Arc<str>,
    },
}

#[derive(Debug, Clone)]
pub struct ClipboardEntry {
    pub id: u64,
    pub timestamp: u64,
    pub payload: ClipboardPayload,
}

pub struct ClipboardListener {
    tx: Sender<()>,
    pub rx: Receiver<ClipboardEntry>,
}

impl ClipboardListener {
    pub fn new() -> Self {
        let (ping_tx, ping_rx) = channel::<()>();
        let (data_tx, data_rx) = channel::<ClipboardEntry>();

        thread::spawn(move || {
            let mut last_hash: u64 = 0;

            // capture current clipboard once at startup
            if let Some(entry) = try_capture_clipboard_with_retry() {
                last_hash = calculate_entry_hash(&entry);
                let _ = data_tx.send(entry);
            }

            while ping_rx.recv().is_ok() {
                // debounce: wait 30ms to gather all clipboard changes from one burst
                let start = std::time::Instant::now();
                while ping_rx.recv_timeout(Duration::from_millis(30)).is_ok() {
                    if start.elapsed() > Duration::from_millis(150) {
                        break;
                    }
                }

                // skip if the app itself wrote to the clipboard (avoid echo loop)
                if IS_INTERNAL_COPY.swap(false, Ordering::SeqCst) {
                    continue;
                }

                if let Some(entry) = try_capture_clipboard_with_retry() {
                    let hash = calculate_entry_hash(&entry);
                    if hash != last_hash {
                        last_hash = hash;
                        let _ = data_tx.send(entry);
                    }
                }
            }
        });

        Self {
            tx: ping_tx,
            rx: data_rx,
        }
    }

    pub fn notify_update(&self) {
        let _ = self.tx.send(());
    }
}

impl Default for ClipboardListener {
    fn default() -> Self {
        Self::new()
    }
}

pub fn try_capture_clipboard_now() -> Option<ClipboardEntry> {
    capture_clipboard()
}

fn try_capture_clipboard_with_retry() -> Option<ClipboardEntry> {
    for _ in 0..5 {
        if let Some(entry) = capture_clipboard() {
            return Some(entry);
        }
        thread::sleep(Duration::from_millis(10));
    }
    None
}

fn capture_clipboard() -> Option<ClipboardEntry> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let id = ENTRY_ID_GEN.fetch_add(1, Ordering::Relaxed);

    unsafe {
        if OpenClipboard(None).is_err() {
            return None;
        }

        struct ClipGuard;
        impl Drop for ClipGuard {
            fn drop(&mut self) {
                unsafe {
                    let _ = CloseClipboard();
                }
            }
        }
        let _guard = ClipGuard;

        // password-manager privacy marker (1Password / Bitwarden / KeePass)
        let ignore_format = RegisterClipboardFormatW(w!("Clipboard Viewer Ignore"));
        if ignore_format != 0 && IsClipboardFormatAvailable(ignore_format).is_ok() {
            return None;
        }

        // extract files (CF_HDROP)
        if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok()
            && let Ok(handle) = GetClipboardData(CF_HDROP.0 as u32)
        {
            let hdrop = HDROP(handle.0 as _);
            let count = DragQueryFileW(hdrop, 0xFFFFFFFF, None);
            let mut paths = Vec::with_capacity(count as usize);

            for i in 0..count {
                let len = DragQueryFileW(hdrop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(hdrop, i, Some(&mut buf));
                let path_str = String::from_utf16_lossy(&buf[..len]);
                paths.push(PathBuf::from(path_str));
            }

            if !paths.is_empty() {
                let summary = if paths.len() == 1 {
                    paths[0]
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                } else {
                    format!(
                        "{} files (e.g. {})",
                        paths.len(),
                        paths[0].file_name().unwrap_or_default().to_string_lossy()
                    )
                };

                return Some(ClipboardEntry {
                    id,
                    timestamp: now,
                    payload: ClipboardPayload::Files {
                        paths: paths.into(),
                        summary: summary.into(),
                    },
                });
            }
        }

        // extract text (CF_UNICODETEXT) (same unsafe block, ClipGuard still holds clipboard)
        if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok()
            && let Ok(handle) = GetClipboardData(CF_UNICODETEXT.0 as u32)
            && !handle.0.is_null()
        {
            let hmem = HGLOBAL(handle.0 as _);
            let ptr = GlobalLock(hmem) as *const u16;
            if !ptr.is_null() {
                let max_len = (GlobalSize(hmem) / 2).min(512 * 1024);
                let mut len = 0usize;
                while len < max_len && *ptr.add(len) != 0 {
                    len += 1;
                }
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                let _ = GlobalUnlock(hmem);

                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    let line_count = text.lines().count();
                    let char_count = text.chars().count();

                    let first_line = text.lines().next().unwrap_or("").trim();
                    let preview_title = if first_line.chars().count() > 60 {
                        format!("{}...", first_line.chars().take(60).collect::<String>())
                    } else {
                        first_line.to_string()
                    };

                    return Some(ClipboardEntry {
                        id,
                        timestamp: now,
                        payload: ClipboardPayload::Text {
                            full_text: Arc::from(text.as_str()),
                            preview_title: Arc::from(preview_title.as_str()),
                            line_count,
                            char_count,
                        },
                    });
                }
            }
        }
    }

    None
}

pub fn calculate_entry_hash(entry: &ClipboardEntry) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match &entry.payload {
        ClipboardPayload::Text { full_text, .. } => full_text.hash(&mut hasher),
        ClipboardPayload::Files { paths, .. } => paths.hash(&mut hasher),
    }
    hasher.finish()
}
