use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};

use std::path::{Path, PathBuf};
use std::process::Command;

use unicode_normalization::UnicodeNormalization;

const RANGE_LO: u32 = 0x4E00;
const RANGE_HI: u32 = 0x9FA5;
const DICT_URL: &str = "https://raw.githubusercontent.com/mozillazg/pinyin-data/v0.15.0/pinyin.txt";

fn dict_path() -> PathBuf {
    env::temp_dir().join("mist-pinyin.txt")
}

fn ensure_dict(cache: &Path) {
    if cache.exists() {
        return;
    }
    let _ = Command::new("curl")
        .args(["-fL", DICT_URL, "-o"])
        .arg(cache)
        .status();
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = env::var("OUT_DIR").unwrap();
    let cache = dict_path();
    ensure_dict(&cache);
    let dict: &Path = &cache;
    if !dict.exists() {
        println!(
            "cargo:warning=pinyin dictionary unavailable (offline?): Chinese search disabled until {} can be fetched",
            cache.display()
        );
        fs::write(
            Path::new(&out_dir).join("pinyin_data.rs"),
            "static SYLLABLES: &[&str] = &[\"\"];\n",
        )
        .unwrap();
        fs::write(Path::new(&out_dir).join("pinyin_map.bin"), [0u8; 2]).unwrap();
        return;
    }

    let mut readings: BTreeMap<u32, String> = BTreeMap::new();
    let mut syllables: BTreeSet<String> = BTreeSet::new();

    for line in BufReader::new(fs::File::open(dict).unwrap())
        .lines()
        .map_while(Result::ok)
    {
        let line = line.trim();
        if line.starts_with('#') || !line.contains(':') {
            continue;
        }
        let Some((code_str, rest)) = line.split_once(':') else {
            continue;
        };
        let code_str = code_str.trim();
        if !code_str.starts_with("U+") {
            continue;
        }
        let Ok(code) = u32::from_str_radix(&code_str[2..], 16) else {
            continue;
        };
        if !(RANGE_LO..=RANGE_HI).contains(&code) {
            continue;
        }
        let first = rest
            .split('#')
            .next()
            .unwrap()
            .split(',')
            .next()
            .unwrap()
            .trim();
        let plain = strip_tones(first);
        if !plain.is_empty() {
            readings.insert(code, plain.clone());
            syllables.insert(plain);
        }
    }

    let table: Vec<String> = std::iter::once(String::new()).chain(syllables).collect();
    let id_of: BTreeMap<&str, u16> = table
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i as u16))
        .collect();

    let count = (RANGE_HI - RANGE_LO + 1) as usize;
    let mut bin = vec![0u8; count * 2];
    for (code, py) in &readings {
        let idx = ((code - RANGE_LO) as usize) * 2;
        let id = id_of[py.as_str()].to_le_bytes();
        bin[idx] = id[0];
        bin[idx + 1] = id[1];
    }
    fs::write(Path::new(&out_dir).join("pinyin_map.bin"), &bin).unwrap();

    let mut rs = String::from("static SYLLABLES: &[&str] = &[\n");
    for chunk in table.chunks(10) {
        rs.push_str("    ");
        rs.push_str(
            &chunk
                .iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(", "),
        );
        rs.push_str(",\n");
    }
    rs.push_str("];\n");
    fs::write(Path::new(&out_dir).join("pinyin_data.rs"), rs).unwrap();
}

fn strip_tones(s: &str) -> String {
    s.nfd().filter(|c| c.is_ascii_alphabetic()).collect()
}
