use crate::domain::{Action, Item, ItemKind, KeyKind};
use std::sync::Arc;

pub fn builtins() -> Vec<Item> {
    vec![
        Item::new_config(),
        Item::new_exit(),
        Item {
            name: Arc::from("Lock Screen"),
            path: Arc::from("Lock the current workstation"),
            kind: ItemKind::Command {
                raw: Arc::from("lock"),
            },
            priority_penalty: 150,
            action: Action::LockScreen,
            keys: Box::new([
                (KeyKind::Name, Arc::from("lock screen")),
                (KeyKind::Alias, Arc::from("lock")),
                (KeyKind::Alias, Arc::from("suoping")),
                (KeyKind::Alias, Arc::from("sp")),
            ]),
        },
        Item {
            name: Arc::from("Shut Down"),
            path: Arc::from("Shutdown the computer"),
            kind: ItemKind::Command {
                raw: Arc::from("shutdown"),
            },
            priority_penalty: 150,
            action: Action::ShutdownSystem,
            keys: Box::new([
                (KeyKind::Name, Arc::from("shutdown")),
                (KeyKind::Alias, Arc::from("guanji")),
                (KeyKind::Alias, Arc::from("gj")),
            ]),
        },
        Item {
            name: Arc::from("Restart"),
            path: Arc::from("Restart the computer"),
            kind: ItemKind::Command {
                raw: Arc::from("restart"),
            },
            priority_penalty: 150,
            action: Action::RestartSystem,
            keys: Box::new([
                (KeyKind::Name, Arc::from("restart")),
                (KeyKind::Alias, Arc::from("reboot")),
                (KeyKind::Alias, Arc::from("chongqi")),
                (KeyKind::Alias, Arc::from("cq")),
            ]),
        },
        Item {
            name: Arc::from("Sleep"),
            path: Arc::from("Put the computer into sleep mode"),
            kind: ItemKind::Command {
                raw: Arc::from("sleep"),
            },
            priority_penalty: 150,
            action: Action::SleepSystem,
            keys: Box::new([
                (KeyKind::Name, Arc::from("sleep")),
                (KeyKind::Alias, Arc::from("xiumian")),
                (KeyKind::Alias, Arc::from("xm")),
            ]),
        },
    ]
}
