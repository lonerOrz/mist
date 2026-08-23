use crate::domain::{Action, Item, ItemKind};
use std::sync::Arc;

struct SearchEngine {
    prefix: &'static str,
    name: &'static str,
    url_template: &'static str,
}

const ENGINES: &[SearchEngine] = &[
    SearchEngine {
        prefix: "/dd",
        name: "DuckDuckGo",
        url_template: "https://duckduckgo.com/?q=",
    },
    SearchEngine {
        prefix: "/gh",
        name: "GitHub",
        url_template: "https://github.com/search?q=",
    },
    SearchEngine {
        prefix: "/bi",
        name: "Bing",
        url_template: "https://www.bing.com/search?q=",
    },
    SearchEngine {
        prefix: "/bd",
        name: "Baidu",
        url_template: "https://www.baidu.com/s?wd=",
    },
    SearchEngine {
        prefix: "/g",
        name: "Google",
        url_template: "https://www.google.com/search?q=",
    },
    SearchEngine {
        prefix: "/b",
        name: "Bilibili",
        url_template: "https://search.bilibili.com/all?keyword=",
    },
    SearchEngine {
        prefix: "/w",
        name: "Wikipedia",
        url_template: "https://zh.wikipedia.org/wiki/",
    },
];

pub fn evaluate(q: &str) -> Option<Item> {
    let trimmed = q.trim();

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let url_arc: Arc<str> = Arc::from(trimmed);
        return Some(Item {
            name: Arc::from(format!("Open URL: {trimmed}")),
            path: Arc::from("Open in default browser"),
            kind: ItemKind::Web,
            priority_penalty: 0,
            action: Action::Launch {
                path: url_arc,
                verb: None,
            },
            keys: Box::new([]),
        });
    }

    if trimmed.starts_with('/') {
        for eng in ENGINES {
            if trimmed.starts_with(eng.prefix) {
                let query_part = trimmed
                    .strip_prefix(eng.prefix)
                    .expect("prefix matched above")
                    .trim();
                let display_query = if query_part.is_empty() {
                    "..."
                } else {
                    query_part
                };
                let target_url = format!("{}{}", eng.url_template, url_encode(query_part));
                let url_arc: Arc<str> = Arc::from(target_url.as_str());
                return Some(Item {
                    name: Arc::from(format!("Search {}: \"{}\"", eng.name, display_query)),
                    path: Arc::from(target_url),
                    kind: ItemKind::Web,
                    priority_penalty: 0,
                    action: Action::Launch {
                        path: url_arc,
                        verb: None,
                    },
                    keys: Box::new([]),
                });
            }
        }
    }

    None
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
    fn bang_and_url_routing() {
        let bing = evaluate("/bi rust").unwrap();
        assert!(bing.name.contains("Bing"));
        assert!(bing.path.contains("bing.com"));

        let bili = evaluate("/b honkai").unwrap();
        assert!(bili.name.contains("Bilibili"));

        let gh = evaluate("/gh hello world").unwrap();
        assert!(gh.path.ends_with("hello%20world"));

        let url = evaluate("https://rust-lang.org").unwrap();
        assert!(url.name.contains("rust-lang.org"));

        assert!(evaluate("calc").is_none());
        assert!(evaluate("no spaces allowed here").is_none());
    }
}
