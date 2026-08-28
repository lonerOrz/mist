pub mod app_mgmt;
pub mod calc;
pub mod clip;
pub mod cmd;
pub mod path;
pub mod sys;
pub mod web;

use crate::clipboard::ClipboardEntry;
use crate::config::Config;
use crate::domain::Item;
use crate::history::History;

pub struct PluginContext<'a> {
    pub index: &'a [Item],
    pub history: &'a History,
    pub config: &'a Config,
    pub clipboard_history: &'a [ClipboardEntry],
}

pub trait Plugin: Send + Sync {
    fn can_handle(&self, raw_input: &str) -> bool;
    fn query(&self, raw_input: &str, ctx: &PluginContext) -> Vec<Item>;
}

pub fn match_prefix<'a>(q: &'a str, prefix: &str) -> Option<&'a str> {
    if q == prefix {
        return Some("");
    }
    if let Some(rest) = q.strip_prefix(prefix)
        && rest.starts_with(' ')
    {
        return Some(rest.trim());
    }
    None
}

pub fn filter_static_items(items: Vec<Item>, args: &str) -> Vec<Item> {
    if args.is_empty() {
        return items;
    }
    let args_lower = args.to_lowercase();
    items
        .into_iter()
        .filter(|i| {
            i.keys.iter().any(|(_, k)| k.to_lowercase() == args_lower)
                || i.name.to_lowercase().contains(&args_lower)
        })
        .collect()
}
