use crate::clipboard::{ClipboardEntry, ClipboardPayload};
use crate::config::Config;
use crate::domain::{Action, Item};
use crate::history::History;
use crate::search;
use std::cmp::Ordering;

/// Represents a statically parsed user query intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryIntent<'a> {
    Empty,
    Calculator(&'a str),
    Command(&'a str),
    WebSearch {
        engine: &'static SearchEngine,
        query: &'a str,
    },
    DirectUrl(&'a str),
    Clipboard {
        filter: &'a str,
    },
    System {
        subcmd: &'a str,
    },
    AppMgmt {
        subcmd: &'a str,
    },
    PathNavigation(&'a str),
    AppSearch(&'a str),
}

impl<'a> QueryIntent<'a> {
    /// Parses the raw input query into a strongly typed intent without heap allocations.
    pub fn parse(raw: &'a str) -> Self {
        let q = raw.trim();
        if q.is_empty() {
            return Self::Empty;
        }

        if let Some(cmd) = match_prefix(q, ">") {
            return Self::Command(cmd);
        }
        if let Some(expr) = q.strip_prefix('?') {
            return Self::Calculator(expr.trim());
        }
        if q.starts_with("http://") || q.starts_with("https://") {
            return Self::DirectUrl(q);
        }
        if q.starts_with('!') {
            for eng in ENGINES {
                if let Some(rest) = q.strip_prefix(eng.prefix)
                    && (rest.is_empty() || rest.starts_with(' '))
                {
                    return Self::WebSearch {
                        engine: eng,
                        query: rest.trim(),
                    };
                }
            }
        }
        if let Some(filter) = match_prefix(q, "/cb") {
            return Self::Clipboard { filter };
        }
        if let Some(subcmd) = match_prefix(q, "/sys") {
            return Self::System { subcmd };
        }
        if let Some(subcmd) = match_prefix(q, "/app") {
            return Self::AppMgmt { subcmd };
        }
        if is_filesystem_path(q) {
            return Self::PathNavigation(q);
        }
        Self::AppSearch(q)
    }
}

/// Routes the parsed query intent to its corresponding handler and returns matched items.
pub fn route_query(
    raw_query: &str,
    index: &[Item],
    history: &History,
    _config: &Config,
    clipboard_history: &[ClipboardEntry],
) -> Vec<Item> {
    match QueryIntent::parse(raw_query) {
        QueryIntent::Empty => Vec::new(),
        QueryIntent::Command(cmd) => {
            if cmd.is_empty() {
                Vec::new()
            } else {
                vec![Item::new_command(cmd)]
            }
        }
        QueryIntent::Calculator(expr) => eval_calculator(expr),
        QueryIntent::DirectUrl(url) => vec![Item::new_web(&format!("Open URL: {url}"), url)],
        QueryIntent::WebSearch { engine, query } => {
            let display_query = if query.is_empty() { "..." } else { query };
            let target_url = format!("{}{}", engine.url_template, url_encode(query));
            vec![Item::new_web(
                &format!("Search {}: \"{}\"", engine.name, display_query),
                &target_url,
            )]
        }
        QueryIntent::Clipboard { filter } => query_clipboard(clipboard_history, filter),
        QueryIntent::System { subcmd } => query_system(subcmd),
        QueryIntent::AppMgmt { subcmd } => query_app_mgmt(subcmd),
        QueryIntent::PathNavigation(path_str) => vec![Item::new_path("Open Folder", path_str)],
        QueryIntent::AppSearch(q) => search_top_k(index, q, history),
    }
}

/// Performs fuzzy and prefix matching on the indexed applications, returning top-k scored items.
pub fn search_top_k(index: &[Item], query: &str, history: &History) -> Vec<Item> {
    if index.is_empty() {
        return Vec::new();
    }

    let query_lower = query.to_lowercase();
    let q_norm: String = query_lower.chars().filter(|&c| c != ' ').collect();
    let mut scored: Vec<(&Item, i32)> = Vec::with_capacity(index.len());

    for item in index {
        let f_score = history.get_score(&item.path);
        if let Some(m) = search::match_item(item, &query_lower, &q_norm, f_score) {
            scored.push((item, m.score));
        }
    }

    if scored.is_empty() {
        return Vec::new();
    }

    let k = scored.len().min(500);
    scored.select_nth_unstable_by(k - 1, |a, b| match b.1.cmp(&a.1) {
        Ordering::Equal => a.0.name.cmp(&b.0.name),
        other => other,
    });

    scored[..k].sort_unstable_by(|a, b| match b.1.cmp(&a.1) {
        Ordering::Equal => a.0.name.cmp(&b.0.name),
        other => other,
    });

    scored.into_iter().take(k).map(|(i, _)| i.clone()).collect()
}

/// Filters and converts recent clipboard entries into launcher display items.
fn query_clipboard(clipboard_history: &[ClipboardEntry], filter: &str) -> Vec<Item> {
    let filter_lower = filter.to_lowercase();
    let mut results = Vec::new();

    for entry in clipboard_history {
        match &entry.payload {
            ClipboardPayload::Text {
                full_text,
                preview_title,
                line_count,
                char_count,
            } => {
                if !filter.is_empty() && !full_text.to_lowercase().contains(&filter_lower) {
                    continue;
                }
                let sub = if *line_count > 1 {
                    format!("{char_count} chars · {line_count} lines · Clipboard Text")
                } else {
                    format!("{char_count} chars · Clipboard Text")
                };
                results.push(Item::new_clipboard_text(
                    preview_title.clone(),
                    &sub,
                    full_text.clone(),
                ));
            }
            ClipboardPayload::Files { summary, paths } => {
                if !filter.is_empty() && !summary.to_lowercase().contains(&filter_lower) {
                    continue;
                }
                let sub = format!("{} files · Clipboard Files", paths.len());
                results.push(Item::new_clipboard_files(
                    summary.clone(),
                    &sub,
                    paths.clone(),
                ));
            }
        }
    }
    results
}

/// Returns static system power management operations filtered by query.
fn query_system(args: &str) -> Vec<Item> {
    filter_static_items(
        vec![
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
        ],
        args,
    )
}

/// Returns static Mist application management commands filtered by query.
fn query_app_mgmt(args: &str) -> Vec<Item> {
    filter_static_items(
        vec![
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
        ],
        args,
    )
}

/// Evaluates mathematical expressions using evalexpr.
fn eval_calculator(args: &str) -> Vec<Item> {
    let q = args.trim().trim_end_matches('=').trim();
    if q.is_empty() || !q.chars().any(|c| "+-*/^%()".contains(c)) {
        return Vec::new();
    }
    if let Ok(res) = evalexpr::eval(&floatify(q)) {
        vec![Item::new_calculator(&res.to_string())]
    } else {
        Vec::new()
    }
}

/// Web search engine configuration.
#[derive(Debug, PartialEq, Eq)]
pub struct SearchEngine {
    pub prefix: &'static str,
    pub name: &'static str,
    pub url_template: &'static str,
}

pub const ENGINES: &[SearchEngine] = &[
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

/// Strips a prefix command pattern, supporting trailing space separation or direct command prefixes.
#[inline]
pub fn match_prefix<'a>(q: &'a str, prefix: &str) -> Option<&'a str> {
    if q == prefix {
        return Some("");
    }
    if let Some(rest) = q.strip_prefix(prefix)
        && (rest.starts_with(' ') || prefix == ">")
    {
        return Some(rest.trim());
    }
    None
}

/// Checks whether a query string looks like a Windows filesystem drive or UNC path.
#[inline]
pub fn is_filesystem_path(q: &str) -> bool {
    let b = q.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return b.len() == 2 || b[2] == b'\\' || b[2] == b'/';
    }
    q.starts_with(r"\\") || q.starts_with("//")
}

/// Filters a static list of items by matching keys or name against the argument query.
fn filter_static_items(items: Vec<Item>, args: &str) -> Vec<Item> {
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

/// Converts integer tokens in an expression to floating point representations for accurate division.
fn floatify(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 8);
    let mut chars = q.chars().peekable();
    while let Some(c) = chars.next() {
        if !c.is_ascii_digit() {
            out.push(c);
            continue;
        }
        let mut num = String::new();
        num.push(c);
        let mut has_dot = false;
        while let Some(&n) = chars.peek() {
            if n.is_ascii_digit() || (n == '.' && !has_dot) {
                has_dot |= n == '.';
                num.push(n);
                chars.next();
            } else {
                break;
            }
        }
        if matches!(chars.peek(), Some('e') | Some('E')) {
            num.push(chars.next().unwrap());
            if matches!(chars.peek(), Some('+') | Some('-')) {
                num.push(chars.next().unwrap());
            }
            while let Some(&n) = chars.peek() {
                if n.is_ascii_digit() {
                    num.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            has_dot = true;
        }
        if !has_dot {
            num.push_str(".0");
        }
        out.push_str(&num);
    }
    out
}

/// Encodes query parameters safely for URL inclusion.
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
    fn test_intent_parsing() {
        assert_eq!(QueryIntent::parse(""), QueryIntent::Empty);
        assert_eq!(
            QueryIntent::parse("> ping 1.1.1.1"),
            QueryIntent::Command("ping 1.1.1.1")
        );
        assert_eq!(
            QueryIntent::parse("? 12 * 4"),
            QueryIntent::Calculator("12 * 4")
        );
        assert_eq!(
            QueryIntent::parse("/cb hello"),
            QueryIntent::Clipboard { filter: "hello" }
        );
        assert_eq!(
            QueryIntent::parse("/sys lock"),
            QueryIntent::System { subcmd: "lock" }
        );
        assert_eq!(
            QueryIntent::parse("/app config"),
            QueryIntent::AppMgmt { subcmd: "config" }
        );
        assert_eq!(
            QueryIntent::parse(r"C:\Windows"),
            QueryIntent::PathNavigation(r"C:\Windows")
        );
        assert_eq!(
            QueryIntent::parse("https://rust-lang.org"),
            QueryIntent::DirectUrl("https://rust-lang.org")
        );
        assert_eq!(QueryIntent::parse("code"), QueryIntent::AppSearch("code"));

        if let QueryIntent::WebSearch { engine, query } = QueryIntent::parse("!gh rust") {
            assert_eq!(engine.prefix, "!gh");
            assert_eq!(query, "rust");
        } else {
            panic!("Bang syntax parsing failed");
        }
    }

    #[test]
    fn test_calc_eval() {
        let res = eval_calculator("1 + 2 * 3");
        assert_eq!(res.len(), 1);
        assert_eq!(&*res[0].name, "= 7");
    }

    #[test]
    fn test_path_detection() {
        assert!(is_filesystem_path(r"C:\Windows"));
        assert!(is_filesystem_path("D:/Tools"));
        assert!(is_filesystem_path(r"\\Server\Share"));
        assert!(!is_filesystem_path("code.exe"));
    }
}
