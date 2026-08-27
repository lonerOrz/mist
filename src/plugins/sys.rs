use crate::domain::{Action, Item};

pub fn query(args: &str) -> Vec<Item> {
    let items = vec![
        Item::new_system(
            "Lock Screen",
            "lock",
            Action::LockScreen,
            &["lock screen", "suoping", "sp"],
        ),
        Item::new_system(
            "Shut Down",
            "shutdown",
            Action::ShutdownSystem,
            &["guanji", "gj"],
        ),
        Item::new_system(
            "Restart",
            "restart",
            Action::RestartSystem,
            &["reboot", "chongqi", "cq"],
        ),
        Item::new_system("Sleep", "sleep", Action::SleepSystem, &["xiumian", "xm"]),
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
