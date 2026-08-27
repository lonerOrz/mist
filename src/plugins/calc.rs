use crate::domain::Item;
use crate::plugins::{Plugin, PluginContext};

pub struct CalcPlugin;

impl Plugin for CalcPlugin {
    fn can_handle(&self, raw_input: &str) -> bool {
        raw_input.starts_with('?')
    }
    fn query(&self, raw_input: &str, _ctx: &PluginContext) -> Vec<Item> {
        let rest = raw_input.strip_prefix('?').unwrap_or(raw_input).trim();
        query(rest)
    }
}

pub fn query(args: &str) -> Vec<Item> {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_eval() {
        assert_eq!(&*query("1 + 2 * 3")[0].name, "= 7");
        assert!(query("hello").is_empty());
    }
}
