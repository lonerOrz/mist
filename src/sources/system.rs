use crate::domain::{Action, Item, ItemKind, KeyKind};
use std::sync::Arc;

pub fn builtins() -> Vec<Item> {
    vec![
        Item::new_config(),
        Item::new_exit(),
        make_sys_action(
            "Lock Screen",
            "lock suoping sp",
            "rundll32.exe user32.dll,LockWorkStation",
        ),
        make_sys_action(
            "Shut Down",
            "shutdown poweroff guanji gj",
            "shutdown /s /t 0",
        ),
        make_sys_action("Restart", "restart reboot chongqi cq", "shutdown /r /t 0"),
        make_sys_action(
            "Sleep",
            "sleep xiumian xm",
            "rundll32.exe powrprof.dll,SetSuspendState 0,1,0",
        ),
    ]
}

fn make_sys_action(name: &'static str, aliases: &'static str, cmd: &'static str) -> Item {
    let mut keys: Vec<(KeyKind, Arc<str>)> = vec![(KeyKind::Name, Arc::from(name.to_lowercase()))];
    for a in aliases.split_whitespace() {
        keys.push((KeyKind::Alias, Arc::from(a)));
    }
    Item {
        name: Arc::from(name),
        path: Arc::from(format!("System Command: {cmd}")),
        kind: ItemKind::Command {
            raw: Arc::from(cmd),
        },
        priority_penalty: 150,
        action: Action::Launch {
            path: Arc::from(cmd),
            verb: None,
        },
        keys: keys.into_boxed_slice(),
    }
}
