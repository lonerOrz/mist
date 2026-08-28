# Mist

A lightweight Windows launcher written in Rust.

## Features

- Application indexing from Start Menu, Desktop, UWP apps, and App Paths.
- Sub-string, acronym, and Pinyin search with frecency scoring.
- Plugin-based architecture with explicit prefix routing.
- Clipboard history manager with move-to-front dedup and file/text support.
- Direct2D rendering with acrylic backdrop and spring animations.

## Prefixes & Plugins

| Prefix   | Plugin     | Description                   | Example                       |
| :------- | :--------- | :---------------------------- | :---------------------------- |
| _(none)_ | App Search | Search installed applications | `code`, `微信`                |
| `>`      | Command    | Execute shell commands        | `> ping baidu.com`            |
| `?`      | Calculator | Evaluate math expressions     | `? 12 * (4 + 5)`              |
| `!`      | Web Search | Search engines (Bang syntax)  | `!gh rust`, `!g ai`           |
| `/sys`   | System     | System power operations       | `/sys lock`, `/sys shutdown`  |
| `/app`   | App Mgmt   | Manage Mist itself            | `/app config`, `/app restart` |
| `/cb`    | Clipboard  | Paste from clipboard history  | `/cb`, `/cb config`           |
| _(auto)_ | Path       | Open file explorer            | `C:\Windows`                  |

The `/cb` plugin shows recent clipboard entries (text and files) with an optional filter;
pressing `Enter` pastes the selected entry back into the active window.

## Shortcuts

| Key              | Action                              |
| ---------------- | ----------------------------------- |
| `Alt+Space`      | Toggle window                       |
| `Up` / `Down`    | Navigate items                      |
| `Enter`          | Execute selected item / copy result |
| `Shift+Enter`    | Execute as administrator            |
| `Ctrl+Backspace` | Delete previous word                |
| `Esc`            | Hide window                         |

## Build

```bash
# Native
cargo build --target x86_64-pc-windows-gnu --release

# Or cross-compile cleanly with zig (recommended for reproducible builds)
cargo zigbuild --target x86_64-pc-windows-gnu --release
```

Binary is output to `target/x86_64-pc-windows-gnu/release/mist.exe`

## License

This project is licensed under the [GNU General Public License v3.0](LICENSE).
