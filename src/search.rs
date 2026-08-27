use crate::domain::{Item, KeyKind, Match};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use std::sync::OnceLock;

static MATCHER: OnceLock<SkimMatcherV2> = OnceLock::new();

fn get_matcher() -> &'static SkimMatcherV2 {
    MATCHER.get_or_init(SkimMatcherV2::default)
}

impl KeyKind {
    #[inline]
    pub fn score(&self, text: &str, q: &str) -> Option<i32> {
        if text.is_empty() || q.is_empty() {
            return None;
        }

        // Exact match
        if text == q {
            return Some(match self {
                KeyKind::Name => 1000,
                KeyKind::Pinyin => 900,
                KeyKind::Initials => 800,
                KeyKind::Alias => 700,
            });
        }

        // Prefix match
        if text.starts_with(q) {
            let base = match self {
                KeyKind::Name => 600,
                KeyKind::Pinyin => 550,
                KeyKind::Initials => 500,
                KeyKind::Alias => 450,
            };
            let bonus = ((q.len() as f32 / text.len() as f32) * 200.0) as i32;
            return Some(base + bonus);
        }

        // Fuzzy match via Skim
        if let Some(fuzzy_score) = get_matcher().fuzzy_match(text, q)
            && fuzzy_score > 0
        {
            let normalized = (fuzzy_score / 10) as i32;
            let base = match self {
                KeyKind::Name => 300,
                KeyKind::Pinyin => 280,
                KeyKind::Initials => 250,
                KeyKind::Alias => 200,
            };
            return Some(base + normalized.min(200));
        }

        None
    }
}

pub fn match_item<'a>(
    item: &'a Item,
    _query: &str,
    query_lower: &str,
    frecency_score: i32,
) -> Option<Match<'a>> {
    if query_lower.is_empty() {
        return None;
    }

    let base_score = item
        .keys
        .iter()
        .filter_map(|(kind, key)| kind.score(key, query_lower))
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

        // Fuzzy matches should now work via Skim
        assert!(match_item(&vscode, "vsc", "vsc", 0).is_some());
        assert!(match_item(&vscode, "vscode", "vscode", 0).is_some());
        assert_eq!(match_item(&vscode, "code", "code", 0).unwrap().score, 1000);

        // Pinyin exact and fuzzy
        assert_eq!(match_item(&wx, "weixin", "weixin", 0).unwrap().score, 900);
        assert!(match_item(&wx, "weix", "weix", 0).is_some());
        assert_eq!(match_item(&wx, "wx", "wx", 0).unwrap().score, 800);

        // Task manager - initials match
        assert!(match_item(&taskmgr, "rwglq", "rwglq", 0).is_some());

        // ASCII initials (Node.js)
        let nj = Item::new_application("Node.js", r"C:\node.exe");
        assert!(match_item(&nj, "nj", "nj", 0).is_some());

        // Frecency bonus
        assert_eq!(match_item(&wx, "wx", "wx", 50).unwrap().score, 850);

        // Partial fuzzy should score lower than prefix/exact
        let chrome = Item::new_application("Google Chrome", r"C:\chrome.exe");
        let chrome_match = match_item(&chrome, "c", "c", 400);
        assert!(chrome_match.is_some());
        assert!(chrome_match.unwrap().score < 800);

        // No match
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
            700
        );
        assert!(cfg.is_name_exact("config"));
        assert!(!cfg.is_name_exact("settings"));

        assert_eq!(match_item(&exit, ":q", ":q", 0).unwrap().score, 700);
        assert_eq!(match_item(&exit, "mist", "mist", 0).unwrap().score, 700);

        let calc = Item::new_calculator("7");
        assert!(match_item(&calc, "7", "7", 0).is_none());

        let uwp = Item::new_application("Photos", r"shell:AppsFolder\abc.def");
        assert_eq!(uwp.keys.len(), 1);
    }
}
