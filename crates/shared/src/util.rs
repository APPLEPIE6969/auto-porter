//! Small helpers shared by every phase.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Mod id rule used by both manifests: ^[a-z][a-z0-9_-]{1,63}$
pub fn is_valid_mod_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    id.len() >= 2
        && id.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Escape a string for interpolation into a TOML basic string
/// (`"..."`). Mod metadata (names, descriptions) is third-party text:
/// unescaped quotes, backslashes and control characters produced invalid
/// manifests on real jars (found by the ecosystem sweep on Architectury,
/// whose description ends in a newline).
pub fn toml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || c == '\u{7F}' => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Write JSON pretty-printed with sorted keys (BTreeMap-backed where relevant)
/// and a trailing newline, so analyze snapshots stay diff-stable.
pub fn write_json_canonical<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(value).expect("serializable");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::File::create(path)?;
    f.write_all(text.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

/// Recursively collect files under `dir`, returning paths relative to `dir`,
/// sorted for determinism.
pub fn collect_files(dir: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    fn walk(dir: &Path, base: &Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
        if dir.is_dir() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let p = entry.path();
                if p.is_dir() {
                    walk(&p, base, out)?;
                } else {
                    out.push(p.strip_prefix(base).unwrap_or(&p).to_path_buf());
                }
            }
        }
        Ok(())
    }
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_escape_keeps_plain_text() {
        assert_eq!(toml_escape("hello world_42"), "hello world_42");
        // non-ASCII is legal in a basic string and stays readable
        assert_eq!(toml_escape("café ☕"), "café ☕");
    }

    #[test]
    fn toml_escape_handles_quotes_backslashes_and_control_chars() {
        assert_eq!(toml_escape("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(toml_escape("a\\b"), "a\\\\b");
        assert_eq!(toml_escape("line1\nline2"), "line1\\nline2");
        assert_eq!(toml_escape("tab\there"), "tab\\there");
        // the Architectury sweep case: trailing newline in the description
        assert_eq!(
            toml_escape("An intermediary api aimed to ease developing multiplatform mods.\n"),
            "An intermediary api aimed to ease developing multiplatform mods.\\n"
        );
        // other C0 controls become \uXXXX
        assert_eq!(toml_escape("\u{1}"), "\\u0001");
        assert_eq!(toml_escape("\u{7F}"), "\\u007F");
    }

    #[test]
    fn toml_escape_output_parses_back() {
        let v: toml::Value =
            toml::from_str(&format!("d = \"{}\"", toml_escape("q\"nl\nbs\\"))).unwrap();
        assert_eq!(v["d"].as_str().unwrap(), "q\"nl\nbs\\");
    }
}
