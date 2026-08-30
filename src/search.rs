use crate::domain::{Item, KeyKind, Match};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use std::sync::OnceLock;

static MATCHER: OnceLock<SkimMatcherV2> = OnceLock::new();

/// Returns a shared static instance of the Skim fuzzy matcher.
fn get_matcher() -> &'static SkimMatcherV2 {
    MATCHER.get_or_init(SkimMatcherV2::default)
}

impl KeyKind {
    /// Computes the matching score of an indexing key against raw and normalized queries.
    #[inline]
    pub fn score(&self, text: &str, q: &str, q_norm: &str) -> Option<i32> {
        if text.is_empty() || q.is_empty() {
            return None;
        }

        // Exact match
        if text == q || (*self == KeyKind::Pinyin && q_norm == q && text == q_norm) {
            return Some(match self {
                KeyKind::Name => 2000,
                KeyKind::Pinyin => 1800,
                KeyKind::Alias => 1700,
                KeyKind::Initials => 1400,
            });
        }

        // Prefix match
        if text.starts_with(q) {
            let base = match self {
                KeyKind::Name => 1200,
                KeyKind::Alias => 1100,
                KeyKind::Pinyin => 1000,
                KeyKind::Initials => 800,
            };
            let bonus = ((q.len() as f32 / text.len() as f32) * 200.0) as i32;
            return Some(base + bonus);
        }

        // Fuzzy match via Skim
        if let Some(s) = get_matcher().fuzzy_match(text, q).filter(|&s| s > 0) {
            let normalized = ((s as f32 / 100.0) * 200.0) as i32;
            let base = match self {
                KeyKind::Name => 400,
                KeyKind::Alias => 350,
                KeyKind::Pinyin => 300,
                KeyKind::Initials => 200,
            };
            return Some(base + normalized.min(200));
        }

        // Fuzzy fallback for Pinyin keys (query without spaces against joined pinyin)
        if *self == KeyKind::Pinyin
            && q != q_norm
            && let Some(s) = get_matcher().fuzzy_match(text, q_norm).filter(|&s| s > 0)
        {
            let normalized = ((s as f32 / 100.0) * 200.0) as i32;
            return Some(300 + normalized.min(200));
        }

        None
    }
}

/// Matches an item against query strings, returning the highest scoring match with frecency applied.
#[inline]
pub fn match_item<'a>(
    item: &'a Item,
    query_lower: &str,
    q_norm: &str,
    frecency_score: i32,
) -> Option<Match<'a>> {
    if query_lower.is_empty() {
        return None;
    }

    let base_score = item
        .keys
        .iter()
        .filter_map(|(kind, key)| kind.score(key, query_lower, q_norm))
        .max()?;

    Some(Match {
        item,
        score: base_score + frecency_score - item.priority_penalty,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_search_model() {
        let vscode = Item::new_application("Visual Studio Code", r"C:\code.exe");
        let wx = Item::new_application("\u{5fae}\u{4fe1}", r"C:\wx.exe");
        let taskmgr = Item::new_application(
            "\u{4efb}\u{52a1}\u{7ba1}\u{7406}\u{5668}",
            r"C:\System32\Taskmgr.exe",
        );

        // Initials exact match
        assert_eq!(match_item(&vscode, "vsc", "vsc", 0).unwrap().score, 1400);
        // Alias exact match
        assert_eq!(match_item(&vscode, "code", "code", 0).unwrap().score, 1700);

        // Pinyin exact and prefix
        assert_eq!(match_item(&wx, "weixin", "weixin", 0).unwrap().score, 1800);
        assert!(match_item(&wx, "weix", "weix", 0).is_some());
        // Initials exact
        assert_eq!(match_item(&wx, "wx", "wx", 0).unwrap().score, 1400);

        // Initials match for Chinese characters
        assert!(match_item(&taskmgr, "rwglq", "rwglq", 0).is_some());

        // ASCII initials
        let nj = Item::new_application("Node.js", r"C:\node.exe");
        assert_eq!(match_item(&nj, "nj", "nj", 0).unwrap().score, 1400);

        // Frecency bonus
        assert_eq!(match_item(&wx, "wx", "wx", 50).unwrap().score, 1450);

        // Prefix alias match
        let chrome = Item::new_application("Google Chrome", r"C:\chrome.exe");
        let chrome_match = match_item(&chrome, "c", "c", 400);
        assert!(chrome_match.is_some());
        assert!(chrome_match.unwrap().score < 2000);

        // Negative match
        assert!(match_item(&wx, "qq", "qq", 0).is_none());
    }

    #[test]
    fn test_builtin_item_keys() {
        let cfg = Item::new_app_mgmt(
            "Open Config",
            "config",
            crate::domain::Action::OpenConfig,
            &["configuration", "settings", "options"],
        );
        let exit = Item::new_app_mgmt(
            "Exit Mist",
            "exit",
            crate::domain::Action::ExitApp,
            &["quit", "close", ":q"],
        );

        assert!(match_item(&cfg, "conf", "conf", 0).is_some());
        assert_eq!(
            match_item(&cfg, "settings", "settings", 0).unwrap().score,
            1700
        );
        assert!(cfg.is_name_exact("config"));
        assert!(!cfg.is_name_exact("settings"));
        assert_eq!(match_item(&exit, ":q", ":q", 0).unwrap().score, 1700);

        let calc = Item::new_calculator("7");
        assert!(match_item(&calc, "7", "7", 0).is_none());

        let uwp = Item::new_application("Photos", r"shell:AppsFolder\abc.def");
        assert_eq!(uwp.keys.len(), 1);
    }
}
