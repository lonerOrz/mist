use crate::clipboard::ClipboardPayload;
use crate::domain::{Action, Item, ItemKind, KeyKind};
use crate::plugins::{Plugin, PluginContext, match_prefix};
use std::sync::Arc;

pub struct ClipPlugin;

impl Plugin for ClipPlugin {
    fn can_handle(&self, raw_input: &str) -> bool {
        raw_input == "/cb" || raw_input.starts_with("/cb ")
    }

    fn query(&self, raw_input: &str, ctx: &PluginContext) -> Vec<Item> {
        let filter = match_prefix(raw_input, "/cb").unwrap_or("");

        let filter_lower = filter.to_lowercase();
        let mut results = Vec::new();

        for entry in ctx.clipboard_history {
            match &entry.payload {
                ClipboardPayload::Text {
                    full_text,
                    preview_title,
                    line_count,
                    char_count,
                } => {
                    if !filter.is_empty() && !full_text.to_lowercase().contains(&filter_lower) {
                        continue;
                    }

                    let sub = if *line_count > 1 {
                        format!("{char_count} chars · {line_count} lines · Clipboard Text")
                    } else {
                        format!("{char_count} chars · Clipboard Text")
                    };

                    results.push(Item {
                        name: preview_title.clone(),
                        path: Arc::from(sub),
                        kind: ItemKind::Clipboard,
                        priority_penalty: 0,
                        action: Action::PasteText(full_text.clone()),
                        keys: Box::new([(KeyKind::Name, Arc::from(full_text.as_ref()))]),
                    });
                }
                ClipboardPayload::Files { summary, paths } => {
                    if !filter.is_empty() && !summary.to_lowercase().contains(&filter_lower) {
                        continue;
                    }

                    results.push(Item {
                        name: summary.clone(),
                        path: Arc::from(format!("{} files · Clipboard Files", paths.len())),
                        kind: ItemKind::Path,
                        priority_penalty: 0,
                        action: Action::PasteFiles(paths.clone()),
                        keys: Box::new([(KeyKind::Name, summary.clone())]),
                    });
                }
            }
        }
        results
    }
}
