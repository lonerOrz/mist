use crate::domain::{Action, Item, ItemKind};
use std::sync::Arc;

pub fn is_path(q: &str) -> bool {
    let b = q.as_bytes();
    b.len() >= 3
        && ((b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
            || q.starts_with(r"\\")
            || q.starts_with("//"))
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
