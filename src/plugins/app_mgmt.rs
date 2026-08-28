use super::{Plugin, PluginContext, filter_static_items, match_prefix};
use crate::domain::Item;

pub struct AppMgmtPlugin;

impl Plugin for AppMgmtPlugin {
    fn can_handle(&self, raw_input: &str) -> bool {
        match_prefix(raw_input, "/app").is_some()
    }
    fn query(&self, raw_input: &str, _ctx: &PluginContext) -> Vec<Item> {
        match_prefix(raw_input, "/app")
            .map(query)
            .unwrap_or_default()
    }
}

pub fn query(args: &str) -> Vec<Item> {
    let items = vec![
        Item::new_app_mgmt(
            "Open Config",
            "config",
            crate::domain::Action::OpenConfig,
            &["configuration", "settings", "options"],
        ),
        Item::new_app_mgmt(
            "Restart Mist",
            "restart",
            crate::domain::Action::RestartApp,
            &["reload"],
        ),
        Item::new_app_mgmt(
            "Exit Mist",
            "exit",
            crate::domain::Action::ExitApp,
            &["quit", "close", ":q"],
        ),
    ];

    filter_static_items(items, args)
}
