use crate::domain::{Item, KeyKind, Match};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use std::sync::OnceLock;

static MATCHER: OnceLock<SkimMatcherV2> = OnceLock::new();

fn get_matcher() -> &'static SkimMatcherV2 {
    MATCHER.get_or_init(SkimMatcherV2::default)
}

/// Tiered scoring model:
///   Tier 1 (exact):    2000 Name / 1800 Pinyin / 1700 Alias / 1400 Initials
///   Tier 2 (prefix):   1200 Name / 1100 Alias / 1000 Pinyin /  800 Initials
///   Tier 4 (fuzzy):     200-600 (varies by key type + 0–200 normalized bonus)
/// Frecency is capped at 250 so it can never cross tier boundaries.
impl KeyKind {
    #[inline]
    pub fn score(&self, text: &str, q: &str, q_norm: &str) -> Option<i32> {
        if text.is_empty() || q.is_empty() {
            return None;
        }

        // Exact match — try raw text first, then space-normalized for Pinyin
        if text == q || *self == KeyKind::Pinyin && q_norm == q && text == q_norm {
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

        // Fuzzy fallback: match query (with spaces) against stored pinyin (no spaces)
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

pub fn match_item<'a>(
    item: &'a Item,
    _query: &str,
    query_lower: &str,
    frecency_score: i32,
) -> Option<Match<'a>> {
    if query_lower.is_empty() {
        return None;
    }

    // Strip spaces once for pinyin keys — no allocation in the hot loop
    let q_norm: String = query_lower.chars().filter(|&c| c != ' ').collect();

    let base_score = item
        .keys
        .iter()
        .filter_map(|(kind, key)| kind.score(key, query_lower, &q_norm))
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

        // Initials: "vsc" matches VSCode initials exactly → 1400
        assert_eq!(match_item(&vscode, "vsc", "vsc", 0).unwrap().score, 1400);
        // Alias exact: "code" → Alias "code" = 1700
        assert_eq!(match_item(&vscode, "code", "code", 0).unwrap().score, 1700);

        // Pinyin exact and prefix
        assert_eq!(match_item(&wx, "weixin", "weixin", 0).unwrap().score, 1800);
        assert!(match_item(&wx, "weix", "weix", 0).is_some());
        // Initials exact: "wx" → 1400
        assert_eq!(match_item(&wx, "wx", "wx", 0).unwrap().score, 1400);

        // Initials match for Chinese
        assert!(match_item(&taskmgr, "rwglq", "rwglq", 0).is_some());

        // ASCII initials (Node.js)
        let nj = Item::new_application("Node.js", r"C:\node.exe");
        assert_eq!(match_item(&nj, "nj", "nj", 0).unwrap().score, 1400);

        // Frecency bonus (capped at 250 in history, tested with raw value)
        assert_eq!(match_item(&wx, "wx", "wx", 50).unwrap().score, 1450);

        // Chrome: "c" matches Alias "chrome" as prefix → 1100 + bonus + frecency
        let chrome = Item::new_application("Google Chrome", r"C:\chrome.exe");
        let chrome_match = match_item(&chrome, "c", "c", 400);
        assert!(chrome_match.is_some());
        // Prefix alias (1100) + bonus (200) + frecency (400) = 1700 < 2000 (exact name)
        assert!(chrome_match.unwrap().score < 2000);

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

        // "conf" fuzzy-matches Alias "configuration" → score > 0
        assert!(match_item(&cfg, "conf", "conf", 0).is_some());
        // "settings" matches Alias exactly → 1700
        assert_eq!(
            match_item(&cfg, "settings", "settings", 0).unwrap().score,
            1700
        );
        assert!(cfg.is_name_exact("config"));
        assert!(!cfg.is_name_exact("settings"));

        // ":q" matches Alias exactly → 1700
        assert_eq!(match_item(&exit, ":q", ":q", 0).unwrap().score, 1700);

        let calc = Item::new_calculator("7");
        assert!(match_item(&calc, "7", "7", 0).is_none());

        let uwp = Item::new_application("Photos", r"shell:AppsFolder\abc.def");
        // shell: paths have no Alias; only Name key "photos"
        assert_eq!(uwp.keys.len(), 1);
    }
}
