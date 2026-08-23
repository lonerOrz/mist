use crate::domain::{Action, Item, ItemKind};
use std::sync::Arc;

pub fn evaluate(q: &str) -> Option<Item> {
    let b = q.as_bytes();
    let looks_path = b.len() >= 3
        && ((b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
            || q.starts_with(r"\\")
            || q.starts_with("//"));
    if !looks_path {
        return None;
    }

    let path: Arc<str> = Arc::from(q);
    Some(Item {
        name: Arc::from("Open Folder"),
        path: path.clone(),
        kind: ItemKind::Path,
        priority_penalty: 0,
        action: Action::Launch {
            path,
            verb: Some("explore"),
        },
        keys: Box::new([]),
    })
}
