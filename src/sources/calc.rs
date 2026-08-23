use crate::domain::Item;

pub fn eval(query: &str) -> Option<Item> {
    let q = query.trim().trim_end_matches('=').trim();
    if !q.chars().any(|c| "+-*/^%()".contains(c)) {
        return None;
    }

    let res = evalexpr::eval(&floatify(q)).ok()?.to_string();
    Some(Item::new_calculator(&res))
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
        assert_eq!(&*eval("1 + 2 * 3").unwrap().name, "= 7");
        assert_eq!(&*eval("2^10").unwrap().name, "= 1024");
        assert_eq!(&*eval("10 % 4").unwrap().name, "= 2");
        assert_eq!(&*eval("1 + 2 * 3 =").unwrap().name, "= 7");
        assert_eq!(&*eval("5/2").unwrap().name, "= 2.5");
        assert!(eval("hello").is_none());
    }
}
