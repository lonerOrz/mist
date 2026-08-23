use crate::domain::{Item, KeyKind, Match};

impl KeyKind {
    #[inline]
    pub fn score(&self, text: &str, q: &str) -> Option<i32> {
        if text.is_empty() || q.is_empty() {
            return None;
        }

        if text == q {
            return Some(match self {
                KeyKind::Name => 2500,
                KeyKind::Pinyin => 2000,
                KeyKind::Initials => 1800,
                KeyKind::Alias => 1600,
            });
        }
        if text.starts_with(q) {
            let base = match self {
                KeyKind::Name => 1500,
                KeyKind::Pinyin => 1300,
                KeyKind::Initials => 1200,
                KeyKind::Alias => 1000,
            };
            let completeness_bonus = ((q.len() as f32 / text.len() as f32) * 80.0) as i32;
            return Some(base + completeness_bonus);
        }
        if *self == KeyKind::Name && text.is_ascii() && abbr_matches(text.as_bytes(), q.as_bytes())
        {
            return Some(1100);
        }
        if let Some(pos) = text.find(q) {
            let base = match self {
                KeyKind::Name => 700,
                KeyKind::Pinyin => 600,
                KeyKind::Initials => 500,
                KeyKind::Alias => 400,
            };
            let boundary_bonus = if pos > 0 && is_word_sep(text.as_bytes()[pos - 1]) {
                250
            } else {
                0
            };
            return Some(base + boundary_bonus - (pos as i32 * 10));
        }
        None
    }
}

#[inline]
fn is_word_sep(b: u8) -> bool {
    b.is_ascii_whitespace() || matches!(b, b'-' | b'_' | b'.' | b'+')
}

fn abbr_matches(name_lower: &[u8], q: &[u8]) -> bool {
    match name_lower.iter().position(|b| !is_word_sep(*b)) {
        None => q.is_empty(),
        Some(start) => {
            let s = &name_lower[start..];
            let w_len = s.iter().position(|b| is_word_sep(*b)).unwrap_or(s.len());
            let (w, rest) = s.split_at(w_len);
            (1..=w.len().min(q.len()))
                .any(|take| w[..take] == q[..take] && abbr_matches(rest, &q[take..]))
        }
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
    fn test_unified_search_model() {
        let vscode = Item::new_application("Visual Studio Code", r"C:\code.exe");
        let wx = Item::new_application("\u{5fae}\u{4fe1}", r"C:\wx.exe");
        let taskmgr = Item::new_application(
            "\u{4efb}\u{52a1}\u{7ba1}\u{7406}\u{5668}",
            r"C:\System32\Taskmgr.exe",
        );

        assert_eq!(match_item(&vscode, "vsc", "vsc", 0).unwrap().score, 1100);
        assert_eq!(
            match_item(&vscode, "vscode", "vscode", 0).unwrap().score,
            1100
        );
        assert_eq!(match_item(&vscode, "code", "code", 0).unwrap().score, 1600);

        assert_eq!(match_item(&wx, "weixin", "weixin", 0).unwrap().score, 2000);
        assert_eq!(match_item(&wx, "weix", "weix", 0).unwrap().score, 1353);
        assert_eq!(match_item(&wx, "wx", "wx", 0).unwrap().score, 1800);
        assert_eq!(match_item(&wx, "xin", "xin", 0).unwrap().score, 570);

        assert_eq!(
            match_item(&taskmgr, "taskmgr", "taskmgr", 0).unwrap().score,
            1600
        );
        assert_eq!(
            match_item(&taskmgr, "rwglq", "rwglq", 0).unwrap().score,
            1800
        );

        let nj = Item::new_application("Node.js", r"C:\node.exe");
        assert_eq!(match_item(&nj, "nj", "nj", 0).unwrap().score, 1100);

        assert_eq!(match_item(&wx, "wx", "wx", 50).unwrap().score, 1850);

        let chrome = Item::new_application("Google Chrome", r"C:\chrome.exe");
        assert!(match_item(&chrome, "c", "c", 400).unwrap().score < 1800);

        assert!(match_item(&wx, "qq", "qq", 0).is_none());
    }

    #[test]
    fn test_builtin_item_keys() {
        let cfg = Item::new_config();
        let exit = Item::new_exit();

        assert_eq!(match_item(&cfg, "conf", "conf", 0).unwrap().score, 1553);
        assert_eq!(
            match_item(&cfg, "settings", "settings", 0).unwrap().score,
            1600
        );
        assert!(cfg.is_name_exact("config"));
        assert!(!cfg.is_name_exact("settings"));

        assert_eq!(match_item(&exit, ":q", ":q", 0).unwrap().score, 1600);
        assert_eq!(match_item(&exit, "mist", "mist", 0).unwrap().score, 1600);

        let calc = Item::new_calculator("7");
        assert!(match_item(&calc, "7", "7", 0).is_none());

        let uwp = Item::new_application("Photos", r"shell:AppsFolder\abc.def");
        assert_eq!(uwp.keys.len(), 1);
    }
}
