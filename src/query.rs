use crate::config::Config;
use crate::domain::Item;
use crate::history::History;
use crate::sources::{calc, path, web};

pub struct QueryPipeline;

impl QueryPipeline {
    pub fn query(raw_query: &str, index: &[Item], history: &History, config: &Config) -> Vec<Item> {
        let q = raw_query.trim();
        if q.is_empty() {
            return Vec::new();
        }

        let mut results = Vec::with_capacity(config.max_results + 1);

        if let Some(web_item) = web::evaluate(q) {
            results.push(web_item);
            return results;
        }

        if let Some(calc_item) = calc::eval(q) {
            results.push(calc_item);
            return results;
        }

        if let Some(path_item) = path::evaluate(q) {
            results.push(path_item);
            return results;
        }

        let q_lower = q.to_lowercase();
        let mut scored: Vec<(&Item, i32)> = Vec::with_capacity(index.len());
        let mut has_exact_app = false;

        for item in index {
            let f_score = history.get_score(&item.path);
            if let Some(m) = crate::search::match_item(item, q, &q_lower, f_score) {
                if item.is_name_exact(&q_lower) {
                    has_exact_app = true;
                }
                scored.push((item, m.score));
            }
        }

        scored.sort_unstable_by_key(|a| std::cmp::Reverse(a.1));
        results.extend(
            scored
                .into_iter()
                .take(config.max_results)
                .map(|(i, _)| i.clone()),
        );

        if !has_exact_app && results.is_empty() {
            results.push(Item::new_command(q));
        }

        results
    }
}
