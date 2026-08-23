include!(concat!(env!("OUT_DIR"), "/pinyin_data.rs"));

static PINYIN_MAP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pinyin_map.bin"));

#[inline]
pub fn get_char_pinyin(c: char) -> Option<&'static str> {
    let code = c as u32;
    if (0x4E00..=0x9FA5).contains(&code) {
        let idx = ((code - 0x4E00) as usize) * 2;
        if idx + 1 < PINYIN_MAP.len() {
            let id = u16::from_le_bytes([PINYIN_MAP[idx], PINYIN_MAP[idx + 1]]) as usize;
            let py = SYLLABLES[id];
            if !py.is_empty() {
                return Some(py);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_basics() {
        let has_dictionary = PINYIN_MAP.len() > 2;

        if has_dictionary {
            assert_eq!(get_char_pinyin('\u{5fae}'), Some("wei"));
            assert_eq!(get_char_pinyin('\u{4fe1}'), Some("xin"));
        } else {
            assert_eq!(get_char_pinyin('\u{5fae}'), None);
            assert_eq!(get_char_pinyin('\u{4fe1}'), None);
        }

        assert_eq!(get_char_pinyin('A'), None);
        assert_eq!(get_char_pinyin('\u{9fa6}'), None);
    }
}
