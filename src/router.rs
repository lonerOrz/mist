use crate::config::Config;
use crate::domain::Item;
use crate::history::History;
use crate::plugins;
use crate::search;

pub fn route_query(
    raw_query: &str,
    index: &[Item],
    history: &History,
    config: &Config,
) -> Vec<Item> {
    let q = raw_query.trim();
    if q.is_empty() {
        return Vec::new();
    }

    // 1. Explicit prefix routing
    if let Some(rest) = match_prefix(q, ">") {
        return plugins::cmd::query(rest);
    }

    // Calculator: allow ?1+1 or ? 1+1
    if let Some(rest) = q.strip_prefix('?') {
        return plugins::calc::query(rest.trim());
    }

    if let Some(rest) = match_prefix(q, "/sys") {
        return plugins::sys::query(rest);
    }
    if let Some(rest) = match_prefix(q, "/app") {
        return plugins::app_mgmt::query(rest);
    }

    // Web: allow !gh rust, !gh, or direct URLs
    if q.starts_with('!') || q.starts_with("http://") || q.starts_with("https://") {
        return plugins::web::query(q);
    }

    // 2. Implicit feature routing
    if plugins::path::is_path(q) {
        return plugins::path::query(q);
    }

    // 3. Default: App search
    app_search(q, index, history, config)
}

fn match_prefix<'a>(q: &'a str, prefix: &str) -> Option<&'a str> {
    if q == prefix {
        return Some("");
    }
    if let Some(rest) = q.strip_prefix(prefix) {
        if rest.starts_with(' ') {
            return Some(rest.trim());
        }
    }
    None
}

fn app_search(q: &str, index: &[Item], history: &History, config: &Config) -> Vec<Item> {
    let q_lower = q.to_lowercase();
    let mut scored: Vec<(&Item, i32)> = Vec::with_capacity(index.len());

    for item in index {
        let f_score = history.get_score(&item.path);
        if let Some(m) = search::match_item(item, q, &q_lower, f_score) {
            scored.push((item, m.score));
        }
    }

    scored.sort_unstable_by_key(|a| std::cmp::Reverse(a.1));
    scored
        .into_iter()
        .take(config.max_results)
        .map(|(i, _)| i.clone())
        .collect()
}
