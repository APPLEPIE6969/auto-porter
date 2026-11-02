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
