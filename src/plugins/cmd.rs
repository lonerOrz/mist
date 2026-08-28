use crate::domain::Item;
use crate::plugins::{Plugin, PluginContext, match_prefix};

pub struct CmdPlugin;

impl Plugin for CmdPlugin {
    fn can_handle(&self, raw_input: &str) -> bool {
        match_prefix(raw_input, ">").is_some()
    }
    fn query(&self, raw_input: &str, _ctx: &PluginContext) -> Vec<Item> {
        match_prefix(raw_input, ">").map(query).unwrap_or_default()
    }
}

pub fn query(args: &str) -> Vec<Item> {
    if args.is_empty() {
        return Vec::new();
    }
    vec![Item::new_command(args)]
}
