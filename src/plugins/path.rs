use crate::domain::{Action, Item, ItemKind};
use crate::plugins::{Plugin, PluginContext};
use std::sync::Arc;

pub struct PathPlugin;

impl Plugin for PathPlugin {
    fn can_handle(&self, raw_input: &str) -> bool {
        is_path(raw_input)
    }
    fn query(&self, raw_input: &str, _ctx: &PluginContext) -> Vec<Item> {
        query(raw_input)
    }
}

pub fn is_path(q: &str) -> bool {
    let b = q.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return b.len() == 2 || b[2] == b'\\' || b[2] == b'/';
    }
    q.starts_with(r"\\") || q.starts_with("//")
}

pub fn query(args: &str) -> Vec<Item> {
    if !is_path(args) {
        return Vec::new();
    }
    let path: Arc<str> = Arc::from(args);
    vec![Item {
        name: Arc::from("Open Folder"),
        path: path.clone(),
        kind: ItemKind::Path,
        priority_penalty: 0,
        action: Action::Launch {
            path,
            verb: Some("explore"),
        },
        keys: Box::new([]),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_path() {
        assert!(is_path(r"C:\Windows"));
        assert!(is_path("C:/Windows"));
        assert!(is_path("D:"));
        assert!(is_path(r"\\network\share"));
        assert!(is_path("//network/share"));

        assert!(!is_path("chrome"));
        assert!(!is_path("calc"));
        assert!(!is_path("http://github.com"));
    }
}
