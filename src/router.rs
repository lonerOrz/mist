use crate::clipboard::ClipboardEntry;
use crate::config::Config;
use crate::domain::Item;
use crate::history::History;
use crate::plugins::{self, Plugin};
use crate::search;
use std::cmp::Ordering;
use std::sync::OnceLock;

static PLUGINS: OnceLock<Vec<Box<dyn Plugin>>> = OnceLock::new();

fn get_plugins() -> &'static [Box<dyn Plugin>] {
    PLUGINS.get_or_init(|| {
        vec![
            Box::new(plugins::cmd::CmdPlugin),
            Box::new(plugins::calc::CalcPlugin),
            Box::new(plugins::sys::SysPlugin),
            Box::new(plugins::app_mgmt::AppMgmtPlugin),
            Box::new(plugins::web::WebPlugin),
            Box::new(plugins::clip::ClipPlugin),
            Box::new(plugins::path::PathPlugin),
        ]
    })
}

pub fn route_query(
    raw_query: &str,
    index: &[Item],
    history: &History,
    config: &Config,
    clipboard_history: &[ClipboardEntry],
) -> Vec<Item> {
    let q = raw_query.trim();
    if q.is_empty() {
        return Vec::new();
    }

    let ctx = plugins::PluginContext {
        index,
        history,
        config,
        clipboard_history,
    };

    for plugin in get_plugins() {
        if plugin.can_handle(q) {
            return plugin.query(q, &ctx);
        }
    }

    search_top_k(index, q, history)
}

pub fn search_top_k(index: &[Item], query: &str, history: &History) -> Vec<Item> {
    if index.is_empty() {
        return Vec::new();
    }

    let query_lower = query.to_lowercase();
    let mut scored: Vec<(&Item, i32)> = Vec::with_capacity(index.len());

    for item in index {
        let f_score = history.get_score(&item.path);
        if let Some(m) = search::match_item(item, query, &query_lower, f_score) {
            scored.push((item, m.score));
        }
    }

    if scored.is_empty() {
        return Vec::new();
    }

    let k = scored.len().min(500);
    if k == 0 {
        return Vec::new();
    }

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
