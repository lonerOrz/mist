use super::{Plugin, PluginContext, filter_static_items, match_prefix};
use crate::domain::Item;

pub struct SysPlugin;

impl Plugin for SysPlugin {
    fn can_handle(&self, raw_input: &str) -> bool {
        match_prefix(raw_input, "/sys").is_some()
    }
    fn query(&self, raw_input: &str, _ctx: &PluginContext) -> Vec<Item> {
        match_prefix(raw_input, "/sys")
            .map(query)
            .unwrap_or_default()
    }
}

pub fn query(args: &str) -> Vec<Item> {
    let items = vec![
        Item::new_system(
            "Lock Screen",
            "lock",
            crate::domain::Action::LockScreen,
            &["lock screen", "suoping", "sp"],
        ),
        Item::new_system(
            "Shut Down",
            "shutdown",
            crate::domain::Action::ShutdownSystem,
            &["guanji", "gj"],
        ),
        Item::new_system(
            "Restart",
            "restart",
            crate::domain::Action::RestartSystem,
            &["reboot", "chongqi", "cq"],
        ),
        Item::new_system(
            "Sleep",
            "sleep",
            crate::domain::Action::SleepSystem,
            &["xiumian", "xm"],
        ),
    ];

    filter_static_items(items, args)
}
