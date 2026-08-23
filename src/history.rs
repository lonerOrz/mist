use crate::config;
use std::collections::HashMap;
use std::fs;
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy)]
pub struct Record {
    pub count: u32,
    pub last_used: u64,
}

pub struct History {
    records: HashMap<String, Record>,
    sender: Option<Sender<String>>, // One I/O thread serializes all writes
    worker: Option<JoinHandle<()>>,
}

impl Drop for History {
    fn drop(&mut self) {
        // Closing the sender ends the thread's recv loop, then we wait for the
        // final write to flush before the process exits.
        drop(self.sender.take());
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

impl History {
    pub fn load() -> Self {
        let file_path = config::get_mist_dir().join("history.txt");

        let mut records = HashMap::new();
        if let Ok(content) = fs::read_to_string(&file_path) {
            for line in content.lines() {
                // Reverse split: keys may contain '='
                let parts: Vec<&str> = line.rsplitn(3, '=').collect();
                if parts.len() == 3 {
                    let last_used = parts[0].trim().parse::<u64>().unwrap_or(0);
                    let count = parts[1].trim().parse::<u32>().unwrap_or(0);
                    let key = parts[2].trim().to_string();
                    records.insert(key, Record { count, last_used });
                }
            }
        }

        let (tx, rx) = channel::<String>();
        let worker = std::thread::spawn(move || {
            while let Ok(content) = rx.recv() {
                let _ = fs::write(&file_path, content);
            }
        });

        Self {
            records,
            sender: Some(tx),
            worker: Some(worker),
        }
    }

    pub fn record_launch(&mut self, key: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let entry = self.records.entry(key.to_string()).or_insert(Record {
            count: 0,
            last_used: now,
        });

        entry.count = entry.count.saturating_add(1);
        entry.last_used = now;

        let mut content = String::with_capacity(1024);
        for (k, rec) in &self.records {
            content.push_str(&format!("{}={}={}\n", k, rec.count, rec.last_used));
        }
        if let Some(sender) = &self.sender {
            let _ = sender.send(content);
        }
    }

    pub fn get_score(&self, key: &str) -> i32 {
        if let Some(rec) = self.records.get(key) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let age = now.saturating_sub(rec.last_used);
            let multiplier = if age < 3600 {
                150
            } else if age < 86400 {
                100
            } else if age < 86400 * 7 {
                50
            } else {
                20
            };

            (rec.count as i32) * multiplier
        } else {
            0
        }
    }
}
