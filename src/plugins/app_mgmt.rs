use crate::domain::{Action, Item};

pub fn query(args: &str) -> Vec<Item> {
    let items = vec![
        Item::new_app_mgmt(
            "Open Config",
            "config",
            Action::OpenConfig,
            &["configuration", "settings", "options"],
        ),
        Item::new_app_mgmt("Restart Mist", "restart", Action::RestartApp, &["reload"]),
        Item::new_app_mgmt(
            "Exit Mist",
            "exit",
            Action::ExitApp,
            &["quit", "close", ":q"],
        ),
    ];

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
