use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use unicode_normalization::UnicodeNormalization;

const RANGE_LO: u32 = 0x4E00;
const RANGE_HI: u32 = 0x9FA5;
const PINYIN_URL: &str =
    "https://raw.githubusercontent.com/mozillazg/pinyin-data/master/pinyin.txt";

fn main() {
    println!("cargo:rerun-if-env-changed=MIST_PINYIN_DICT");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    if let Ok(env_path) = env::var("MIST_PINYIN_DICT") {
        let path = Path::new(&env_path);
        if path.is_file() && process_dictionary(path, &out_dir) {
            return;
        }
    }

    let tmp_dict = env::temp_dir().join("mist_pinyin_data.txt");

    if !is_valid_file(&tmp_dict) {
        let _ = fetch_file(PINYIN_URL, &tmp_dict);
    }

    if tmp_dict.is_file() && process_dictionary(&tmp_dict, &out_dir) {
        return;
    }

    write_stub_files(&out_dir);
}

fn is_valid_file(path: &Path) -> bool {
    path.is_file() && fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false)
}

fn fetch_file(url: &str, dest: &Path) -> bool {
    let dest_str = dest.to_string_lossy();

    let curl_ok = Command::new("curl")
        .args(["-fsSL", url, "-o", &dest_str])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if curl_ok && is_valid_file(dest) {
        return true;
    }

    if cfg!(windows) {
        let ps_cmd = format!("Invoke-WebRequest -Uri '{url}' -OutFile '{dest_str}'");
        let ps_ok = Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps_cmd])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if ps_ok && is_valid_file(dest) {
            return true;
        }
    }

    false
}

fn process_dictionary(dict_path: &Path, out_dir: &Path) -> bool {
    let Ok(file) = fs::File::open(dict_path) else {
        return false;
    };

    let mut readings: BTreeMap<u32, String> = BTreeMap::new();
    let mut syllables: BTreeSet<String> = BTreeSet::new();

    for line in BufReader::new(file).lines().map_while(Result::ok) {
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
            .unwrap_or("")
            .split(',')
            .next()
            .unwrap_or("")
            .trim();
        let plain = strip_tones(first);
        if !plain.is_empty() {
            readings.insert(code, plain.clone());
            syllables.insert(plain);
        }
    }

    if readings.is_empty() {
        return false;
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
        if let Some(&id) = id_of.get(py.as_str()) {
            let bytes = id.to_le_bytes();
            bin[idx] = bytes[0];
            bin[idx + 1] = bytes[1];
        }
    }

    if fs::write(out_dir.join("pinyin_map.bin"), &bin).is_err() {
        return false;
    }

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

    fs::write(out_dir.join("pinyin_data.rs"), rs).is_ok()
}

fn strip_tones(s: &str) -> String {
    s.nfd().filter(|c| c.is_ascii_alphabetic()).collect()
}

fn write_stub_files(out_dir: &Path) {
    println!(
        "cargo:warning=Pinyin dictionary unavailable (offline/no env); built with pinyin search disabled."
    );

    let _ = fs::write(
        out_dir.join("pinyin_data.rs"),
        "static SYLLABLES: &[&str] = &[\"\"];\n",
    );
    let _ = fs::write(out_dir.join("pinyin_map.bin"), [0u8; 2]);
}
