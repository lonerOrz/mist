# Mist

A lightweight Windows launcher written in Rust.

## Features

- Application indexing from Start Menu, Desktop, UWP apps, PATH, and App Paths.
- Sub-string, acronym, and Pinyin search with frecency scoring.
- Built-in math expression evaluation.
- Command execution with optional administrator privileges.
- Direct2D rendering with acrylic backdrop and spring animations.

## Shortcuts

| Key              | Action                              |
| ---------------- | ----------------------------------- |
| `Ctrl+Space`     | Toggle window                       |
| `Up` / `Down`    | Navigate items                      |
| `Enter`          | Execute selected item / copy result |
| `Shift+Enter`    | Execute as administrator            |
| `Ctrl+Backspace` | Delete previous word                |
| `Esc`            | Hide window                         |

## Build and Run

```bash
cargo build --release
```

Binary is output to `target/release/mist.exe`.

- Start in background: `mist.exe`
- Open window immediately: `mist.exe --show`

## License

This project is licensed under the [GNU General Public License v3.0](LICENSE).
