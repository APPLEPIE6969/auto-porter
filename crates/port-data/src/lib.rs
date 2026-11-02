//! Phase 3: Port data (spec §4).
//!
//! Primary carrier: `horizon-port-pack check <jar> <out>` — the real single-mod
//! flow. It carries the jar's content into `<out>/carry/` (a ready
//! `horizon-mod.toml` + `assets/` + `data/` payload) and writes `check.json`
//! and `carry-report.json` next to it; the porter zips `carry/` into the
//! `.hmod`. (`import`/`inventory`/`fetch`/`carry`/`report` are the tool's
//! *modpack* flow — `import` rejects single jars.)
//!
//! When the binary is not installed (e.g. a machine without a Horizon
//! checkout), a clearly-labelled `built-in-fallback` carrier packs the jar's
//! `assets/` + `data/` into a `.hmod` so the pipeline stays exercisable; the
//! PortReport always records which carrier ran (see docs/limitations.md).

use shared::config::PorterConfig;
use shared::error::{ErrorType, Phase, PortError};
use shared::types::DecomposedMod;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct DataPortOutcome {
    pub hmod: Option<PathBuf>,
    /// "horizon-port-pack" | "built-in-fallback"
    pub carrier: String,
    pub carried: Vec<String>,
    pub dropped: Vec<String>,
    pub errors: Vec<PortError>,
    pub port_pack_stdout: Option<String>,
}

pub fn port_data(
    jar_path: &Path,
    decomposed: &DecomposedMod,
    cfg: &PorterConfig,
    out_dir: &Path,
) -> DataPortOutcome {
    let mod_id = decomposed.metadata.id.clone();
    let pack_bin = cfg.expand(&cfg.horizon.port_pack_path);

    if binary_available(&pack_bin, "horizon-port-pack") {
        let planned: Vec<String> = decomposed
            .data_files
            .iter()
            .map(|f| f.path.clone())
            .collect();
        match run_port_pack(jar_path, &mod_id, cfg, out_dir, &planned) {
            Ok(mut outcome) => {
                validate_result(&mut outcome, &mod_id);
                return outcome;
            }
            Err(e) => {
                return DataPortOutcome {
                    hmod: None,
                    carrier: "horizon-port-pack".to_string(),
                    carried: Vec::new(),
                    dropped: Vec::new(),
                    errors: vec![e],
                    port_pack_stdout: None,
                };
            }
        }
    }

    let mut outcome = fallback_carry(jar_path, decomposed, out_dir);
    validate_result(&mut outcome, &mod_id);
    outcome
}

fn binary_available(explicit: &Path, name: &str) -> bool {
    if explicit.is_file() {
        return true;
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let p = dir.join(name);
            if p.is_file() {
                return true;
            }
        }
    }
    false
}

#[allow(clippy::result_large_err)]
fn run_port_pack(
    jar_path: &Path,
    mod_id: &str,
    cfg: &PorterConfig,
    out_dir: &Path,
    planned: &[String],
) -> Result<DataPortOutcome, PortError> {
    let bin = {
        let p = cfg.expand(&cfg.horizon.port_pack_path);
        if p.is_file() {
            p
        } else {
            PathBuf::from("horizon-port-pack")
        }
    };
    let work = std::env::temp_dir().join(format!("auto-porter-{mod_id}"));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| {
        PortError::new(
            Phase::PortData,
            ErrorType::MissingDependency,
            format!("cannot create port dir {}: {e}", work.display()),
        )
    })?;

    // Single-mod carrier: `check <jar> <out>` (the import/inventory/… chain
    // is the modpack flow and rejects single jars).
    let output = std::process::Command::new(&bin)
        .arg("check")
        .arg(jar_path)
        .arg(&work)
        .output()
        .map_err(|e| {
            PortError::new(
                Phase::PortData,
                ErrorType::MissingDependency,
                format!("failed to spawn horizon-port-pack check: {e}"),
            )
            .with_suggestion("check that the horizon-port-pack binary is executable")
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(PortError::new(
            Phase::PortData,
            ErrorType::MissingDependency,
            format!("horizon-port-pack check exited non-zero: {}", stderr.trim()),
        )
        .with_suggestion(
            "run `horizon-port-pack check <jar> <out>` manually and inspect the report",
        ));
    }

    // check.json: verdict, ports, skipped — the human-readable summaries go
    // to port_pack_stdout (they are counts, not paths).
    let mut summary = stdout.clone();
    if let Ok(text) = std::fs::read_to_string(work.join("check.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            summary = format!(
                "verdict: {}; {}",
                v.get("verdict").and_then(|x| x.as_str()).unwrap_or("?"),
                v.get("summary").and_then(|x| x.as_str()).unwrap_or("")
            );
        }
    }

    // The carry/ payload is the content mod: zip it with horizon-mod.toml at
    // the archive root (docs/api/mod-files.md top-level rules).
    let carry = work.join("carry");
    if !carry.is_dir() {
        return Err(PortError::new(
            Phase::PortData,
            ErrorType::MissingDependency,
            format!(
                "horizon-port-pack check produced no carry/ payload in {}",
                work.display()
            ),
        ));
    }
    let dest = out_dir.join(format!("{mod_id}.hmod"));
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::File::create(&dest).map_err(|e| {
        PortError::new(
            Phase::PortData,
            ErrorType::MissingDependency,
            format!("cannot write {}: {e}", dest.display()),
        )
    })?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut files = Vec::new();
    collect_files(&carry, "", &mut files);
    if files.is_empty() {
        return Err(PortError::new(
            Phase::PortData,
            ErrorType::MissingDependency,
            "the carry/ payload is empty; refusing to write an empty .hmod",
        ));
    }
    for (rel, path) in &files {
        if zip.start_file(rel.as_str(), options).is_err() {
            return Err(PortError::new(
                Phase::PortData,
                ErrorType::MissingDependency,
                format!("failed to write {rel} into the .hmod zip"),
            ));
        }
        let mut f = std::fs::File::open(path).map_err(|e| {
            PortError::new(
                Phase::PortData,
                ErrorType::MissingDependency,
                format!("cannot read {}: {e}", path.display()),
            )
        })?;
        std::io::copy(&mut f, &mut zip).map_err(|e| {
            PortError::new(
                Phase::PortData,
                ErrorType::MissingDependency,
                format!("cannot write {rel} into the .hmod zip: {e}"),
            )
        })?;
    }
    if zip.finish().is_err() {
        return Err(PortError::new(
            Phase::PortData,
            ErrorType::MissingDependency,
            "failed to finalize the .hmod zip",
        ));
    }
    let _ = std::fs::remove_dir_all(&work);

    // Real carried/dropped gate: what actually landed in the .hmod vs what
    // decompose planned. horizon-port-pack may add 26.3 upgrade files
    // (recorded in the summary), but a planned file missing from the payload
    // is a real silent drop.
    let zipped: std::collections::BTreeSet<String> = files.iter().map(|(r, _)| r.clone()).collect();
    let carried: Vec<String> = planned
        .iter()
        .filter(|p| zipped.contains(p.as_str()))
        .cloned()
        .collect();
    let dropped: Vec<String> = planned
        .iter()
        .filter(|p| !zipped.contains(p.as_str()))
        .cloned()
        .collect();

    Ok(DataPortOutcome {
        hmod: Some(dest),
        carrier: "horizon-port-pack".to_string(),
        carried,
        dropped,
        errors: Vec::new(),
        port_pack_stdout: Some(summary),
    })
}

/// Collect (relative-zip-path, absolute-path) pairs for every file under
/// `dir`, with forward-slash relative paths for the archive.
fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if p.is_dir() {
            collect_files(&p, &rel, out);
        } else if p.is_file() {
            out.push((rel, p));
        }
    }
}

fn fallback_carry(jar_path: &Path, decomposed: &DecomposedMod, out_dir: &Path) -> DataPortOutcome {
    let mod_id = &decomposed.metadata.id;
    let version = &decomposed.metadata.version;
    let name = &decomposed.metadata.name;
    let description = decomposed
        .metadata
        .description
        .clone()
        .unwrap_or_else(|| format!("{name} content, carried by auto-porter"));

    // horizon-mod.toml (content-mod manifest, spec §0.6). Metadata is
    // third-party text: escape every field (real descriptions contain
    // quotes/newlines — found by the ecosystem sweep).
    let manifest = format!(
        "schema = 1\nid = \"{}\"\nversion = \"{}\"\nname = \"{}\"\ndescription = \"{}\"\n",
        shared::util::toml_escape(mod_id),
        shared::util::toml_escape(version),
        shared::util::toml_escape(name),
        shared::util::toml_escape(&description),
    );

    let mut carried = Vec::new();
    let mut dropped = Vec::new();
    let mut errors = Vec::new();

    let dest = out_dir.join(format!("{mod_id}.hmod"));
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = match std::fs::File::create(&dest) {
        Ok(f) => f,
        Err(e) => {
            errors.push(PortError::new(
                Phase::PortData,
                ErrorType::MissingDependency,
                format!("cannot write {}: {e}", dest.display()),
            ));
            return DataPortOutcome {
                hmod: None,
                carrier: "built-in-fallback".to_string(),
                carried,
                dropped,
                errors,
                port_pack_stdout: None,
            };
        }
    };
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let push = |zip: &mut zip::ZipWriter<std::fs::File>,
                path: &str,
                bytes: &[u8],
                carried: &mut Vec<String>,
                dropped: &mut Vec<String>| {
        if zip.start_file(path, options).is_ok() && zip.write_all(bytes).is_ok() {
            carried.push(path.to_string());
        } else {
            dropped.push(path.to_string());
        }
    };

    push(
        &mut zip,
        "horizon-mod.toml",
        manifest.as_bytes(),
        &mut carried,
        &mut dropped,
    );

    let planned: Vec<&String> = decomposed.data_files.iter().map(|f| &f.path).collect();

    let mut jar_ok = true;
    let jar_file = std::fs::File::open(jar_path);
    let mut planned_bytes: std::collections::BTreeMap<String, Vec<u8>> = Default::default();
    if let Ok(f) = jar_file {
        if let Ok(mut archive) = zip::ZipArchive::new(f) {
            for i in 0..archive.len() {
                if let Ok(mut zf) = archive.by_index(i) {
                    let name = zf.name().trim_start_matches("./").to_string();
                    if planned.iter().any(|p| p.as_str() == name) {
                        let mut buf = Vec::new();
                        if std::io::Read::read_to_end(&mut zf, &mut buf).is_ok() {
                            planned_bytes.insert(name, buf);
                        }
                    }
                }
            }
        } else {
            jar_ok = false;
        }
    } else {
        jar_ok = false;
    }
    if !jar_ok {
        errors.push(PortError::new(
            Phase::PortData,
            ErrorType::MissingDependency,
            format!(
                "cannot re-open source jar {} for the data carry",
                jar_path.display()
            ),
        ));
    }

    // pack.mcmeta is not an allowed top-level entry in a .hmod (docs/api/mod-files.md).
    for path in &planned {
        if path.as_str() == "pack.mcmeta" {
            dropped.push(path.to_string());
            continue;
        }
        match planned_bytes.get(path.as_str()) {
            Some(bytes) => push(&mut zip, path, bytes, &mut carried, &mut dropped),
            None => dropped.push(path.to_string()),
        }
    }
    if zip.finish().is_err() {
        errors.push(PortError::new(
            Phase::PortData,
            ErrorType::MissingDependency,
            "failed to finalize the .hmod zip",
        ));
    }

    DataPortOutcome {
        hmod: Some(dest),
        carrier: "built-in-fallback".to_string(),
        carried,
        dropped,
        errors,
        port_pack_stdout: None,
    }
}

/// Gate (spec §4): the produced `.hmod` must pass the verify-crate validation;
/// carried count must match the planned data files (no silent drops).
fn validate_result(outcome: &mut DataPortOutcome, mod_id: &str) {
    if let Some(hmod) = &outcome.hmod {
        let verdict = verify::hmod::validate(hmod);
        for issue in &verdict.issues {
            if issue.severity == shared::report::CheckStatus::Fail {
                outcome.errors.push(
                    PortError::new(
                        Phase::PortData,
                        ErrorType::MissingDependency,
                        issue.message.clone(),
                    )
                    .for_feature(format!("{mod_id}:data")),
                );
            }
        }
    }
    if !outcome.dropped.is_empty() {
        outcome.errors.push(
            PortError::new(
                Phase::PortData,
                ErrorType::MissingDependency,
                format!(
                    "{} planned data file(s) were not carried: {}",
                    outcome.dropped.len(),
                    outcome.dropped.join(", ")
                ),
            )
            .with_suggestion("check that the jar is readable and the paths exist"),
        );
    }
}
