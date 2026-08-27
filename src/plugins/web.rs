use crate::domain::{Action, Item, ItemKind};
use std::sync::Arc;

struct SearchEngine {
    prefix: &'static str,
    name: &'static str,
    url_template: &'static str,
}

const ENGINES: &[SearchEngine] = &[
    SearchEngine {
        prefix: "!dd",
        name: "DuckDuckGo",
        url_template: "https://duckduckgo.com/?q=",
    },
    SearchEngine {
        prefix: "!gh",
        name: "GitHub",
        url_template: "https://github.com/search?q=",
    },
    SearchEngine {
        prefix: "!bi",
        name: "Bing",
        url_template: "https://www.bing.com/search?q=",
    },
    SearchEngine {
        prefix: "!bd",
        name: "Baidu",
        url_template: "https://www.baidu.com/s?wd=",
    },
    SearchEngine {
        prefix: "!g",
        name: "Google",
        url_template: "https://www.google.com/search?q=",
    },
    SearchEngine {
        prefix: "!b",
        name: "Bilibili",
        url_template: "https://search.bilibili.com/all?keyword=",
    },
    SearchEngine {
        prefix: "!w",
        name: "Wikipedia",
        url_template: "https://zh.wikipedia.org/wiki/",
    },
];

pub fn query(args: &str) -> Vec<Item> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let url_arc: Arc<str> = Arc::from(trimmed);
        return vec![Item {
            name: Arc::from(format!("Open URL: {trimmed}")),
            path: Arc::from("Open in default browser"),
            kind: ItemKind::Web,
            priority_penalty: 0,
            action: Action::Launch {
                path: url_arc,
                verb: None,
            },
            keys: Box::new([]),
        }];
    }

    // Bang syntax handling (!gh rust)
    if trimmed.starts_with('!') {
        for eng in ENGINES {
            if let Some(rest) = trimmed.strip_prefix(eng.prefix) {
                // Ensure exact prefix match (e.g., "!gh" or "!gh ", not "!ghrust")
                if rest.is_empty() || rest.starts_with(' ') {
                    let query_part = rest.trim();
                    let display_query = if query_part.is_empty() {
                        "..."
                    } else {
                        query_part
                    };
                    let target_url = format!("{}{}", eng.url_template, url_encode(query_part));
                    let url_arc: Arc<str> = Arc::from(target_url.as_str());
                    return vec![Item {
                        name: Arc::from(format!("Search {}: \"{}\"", eng.name, display_query)),
                        path: Arc::from(target_url),
                        kind: ItemKind::Web,
                        priority_penalty: 0,
                        action: Action::Launch {
                            path: url_arc,
                            verb: None,
                        },
                        keys: Box::new([]),
                    }];
                }
            }
        }
    }
    Vec::new()
}

fn url_encode(input: &str) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(input.len() * 3);
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            encoded.push(b as char);
        } else {
            let _ = write!(&mut encoded, "%{b:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_bang_routing() {
        // Valid bangs
        assert!(
            query("!gh rust")
                .first()
                .unwrap()
                .path
                .contains("github.com")
        );
        assert!(
            query("!b honkai")
                .first()
                .unwrap()
                .path
                .contains("bilibili.com")
        );
        assert!(query("!g").first().unwrap().path.contains("google.com"));

        // Invalid bangs (missing space)
        assert!(query("!ghrust").is_empty());

        // Direct URL
        assert!(
            query("https://rust-lang.org")
                .first()
                .unwrap()
                .path
                .contains("rust-lang.org")
        );
    }
}
