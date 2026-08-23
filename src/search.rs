use crate::domain::{Item, Match};
use pinyin::ToPinyin;

pub fn pinyin_abbr(text: &str) -> String {
    let mut abbr = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            abbr.push(c.to_ascii_lowercase());
        } else if let Some(p) = c.to_pinyin()
            && let Some(first_char) = p.plain().chars().next()
        {
            abbr.push(first_char);
        }
    }
    // Prepend word-initial acronyms for all-ASCII names so multi-word English
    // matches (e.g. "Visual Studio Code" -> "vsc...") hit the pinyin-prefix tier.
    // For mixed Chinese+English names we append instead to keep pinyin at the front.
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() > 1 {
        let mut acronym = String::new();
        for word in &words {
            if let Some(first) = word.chars().next()
                && first.is_ascii_alphanumeric()
            {
                acronym.push(first.to_ascii_lowercase());
            }
        }
        if text.is_ascii() {
            abbr = format!("{acronym}{abbr}");
        } else if !acronym.is_empty() {
            abbr = format!("{abbr}{acronym}");
        }
    }
    abbr
}

/// Match an item, with Frecency weighting
pub fn match_item<'a>(
    item: &'a Item,
    query: &str,
    query_lower: &str,
    frecency_score: i32,
) -> Option<Match<'a>> {
    if query.is_empty() {
        return None;
    }

    let mut base_score = None;

    // Exact
    if &*item.name_lower == query_lower {
        base_score = Some(2000);
    // Prefix
    } else if item.name_lower.starts_with(query_lower) {
        base_score = Some(1200);
    // Contains
    } else if let Some(pos) = item.name_lower.find(query_lower) {
        base_score = Some(600 - (pos as i32 * 10));
    // Pinyin prefix
    } else if !item.pinyin_abbr.is_empty() && item.pinyin_abbr.starts_with(query_lower) {
        base_score = Some(900);
    // Keyword prefix
    } else if !item.keywords_lower.is_empty() && item.keywords_lower.starts_with(query_lower) {
        base_score = Some(800);
    // Pinyin contains
    } else if !item.pinyin_abbr.is_empty()
        && let Some(pos) = item.pinyin_abbr.find(query_lower)
    {
        base_score = Some(450 - (pos as i32 * 10));
    // Keyword contains
    } else if !item.keywords_lower.is_empty()
        && let Some(pos) = item.keywords_lower.find(query_lower)
    {
        base_score = Some(400 - (pos as i32 * 10));
    }

    base_score.map(|s| {
        // Total = base + frecency - penalty
        let total_score = s + frecency_score - item.priority_penalty;
        Match {
            item,
            score: total_score,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Action;
    use std::sync::Arc;

    #[test]
    fn test_pinyin_match() {
        assert_eq!(pinyin_abbr("VSCode"), "vscode");
        assert!(pinyin_abbr("Visual Studio Code").starts_with("vsc"));

        let item = Item {
            name: Arc::from("Visual Studio Code"),
            name_lower: Arc::from("visual studio code"),
            keywords_lower: Arc::from(""),
            pinyin_abbr: Arc::from("vsc"),
            path: Arc::from("C:\\vscode.exe"),
            kind: crate::domain::ItemKind::Application,
            priority_penalty: 0,
            action: Action::Launch(Arc::from("C:\\vscode.exe")),
        };

        let m = match_item(&item, "vsc", "vsc", 200);
        assert!(m.is_some());
        assert_eq!(m.unwrap().score, 1100);
    }

    #[test]
    fn test_exact_name_and_keyword_tiers() {
        let item = Item {
            name: Arc::from("Task Manager"),
            name_lower: Arc::from("task manager"),
            keywords_lower: Arc::from("mgr taskmgr"),
            pinyin_abbr: Arc::from("tm"),
            path: Arc::from(r"C:\Windows\System32\taskmgr.exe"),
            kind: crate::domain::ItemKind::Application,
            priority_penalty: 0,
            action: Action::Launch(Arc::from(r"C:\Windows\System32\taskmgr.exe")),
        };

        // Exact name scores 2000
        assert_eq!(
            match_item(&item, "Task Manager", "task manager", 0)
                .unwrap()
                .score,
            2000
        );
        // Keyword prefix
        assert_eq!(match_item(&item, "mgr", "mgr", 0).unwrap().score, 800);
        // Keyword contains
        assert_eq!(
            match_item(&item, "taskmgr", "taskmgr", 0).unwrap().score,
            360
        );
        // No match
        assert!(match_item(&item, "xyz", "xyz", 0).is_none());
    }
}
