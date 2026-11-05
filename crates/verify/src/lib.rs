//! Phase 5: Verify — static gates (spec §6.1) plus the parity / in-game
//! orchestration (spec §6.2/§6.3, driven through the Python harnesses).

pub mod compile;
pub mod hmod;
pub mod hnmod;
pub mod runner;
pub mod sources;

use shared::report::{CheckStatus, StaticIssue};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct Verdict {
    pub pass: bool,
    pub issues: Vec<StaticIssue>,
}

impl Verdict {
    pub fn new() -> Self {
        Verdict {
            pass: true,
            issues: Vec::new(),
        }
    }
    pub fn fail(&mut self, code: &str, message: impl Into<String>) {
        self.pass = false;
        self.issues.push(StaticIssue {
            code: code.to_string(),
            severity: CheckStatus::Fail,
            message: message.into(),
        });
    }
    pub fn warn(&mut self, code: &str, message: impl Into<String>) {
        self.issues.push(StaticIssue {
            code: code.to_string(),
            severity: CheckStatus::Warn,
            message: message.into(),
        });
    }
}

/// Read all entries of a zip file into a sorted map.
pub(crate) fn read_zip(path: &Path) -> Result<std::collections::BTreeMap<String, Vec<u8>>, String> {
    let f =
        std::fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = std::collections::BTreeMap::new();
    for i in 0..archive.len() {
        let mut zf = archive.by_index(i).map_err(|e| e.to_string())?;
        if zf.is_dir() {
            continue;
        }
        let name = zf.name().trim_start_matches("./").to_string();
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut zf, &mut buf).map_err(|e| format!("{name}: {e}"))?;
        out.insert(name, buf);
    }
    Ok(out)
}

/// Unique top-level path components of a zip's file entries.
pub(crate) fn top_level_entries(
    entries: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Vec<String> {
    let mut tops = std::collections::BTreeSet::new();
    for name in entries.keys() {
        match name.split_once('/') {
            Some((top, _)) => {
                tops.insert(top.to_string());
            }
            None => {
                tops.insert(name.clone());
            }
        }
    }
    tops.into_iter().collect()
}

pub(crate) fn parse_json_checks(
    entries: &std::collections::BTreeMap<String, Vec<u8>>,
    roots: &[&str],
    verdict: &mut Verdict,
) {
    for (name, bytes) in entries {
        let in_root = roots.iter().any(|r| name.starts_with(r));
        if in_root && name.ends_with(".json") {
            if let Err(e) = serde_json::from_slice::<serde_json::Value>(bytes) {
                verdict.fail("json_parse", format!("{name}: invalid JSON: {e}"));
            }
        }
    }
}
