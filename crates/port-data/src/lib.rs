//! Phase 3: Port data (spec §4).
//!
//! Primary carrier: `horizon-port-pack` driven through the documented pipeline
//! (`check` → `import` → `inventory` → `fetch` → `carry` → `report`).
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
        match run_port_pack(jar_path, &mod_id, cfg, out_dir) {
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

    let steps: [(&str, Vec<String>); 6] = [
        ("check", vec![jar_path.display().to_string()]),
        (
            "import",
            vec![jar_path.display().to_string(), work.display().to_string()],
        ),
        ("inventory", vec![work.display().to_string()]),
        ("fetch", vec![work.display().to_string()]),
        ("carry", vec![work.display().to_string()]),
        ("report", vec![work.display().to_string()]),
    ];

    let mut last_stdout = None;
    for (sub, args) in &steps {
        let status = std::process::Command::new(&bin)
            .arg(sub)
            .args(args)
            .output()
            .map_err(|e| {
                PortError::new(
                    Phase::PortData,
                    ErrorType::MissingDependency,
                    format!("failed to spawn horizon-port-pack {sub}: {e}"),
                )
            })?;
        let stdout = String::from_utf8_lossy(&status.stdout).into_owned();
        last_stdout = Some(stdout.clone());
        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr).into_owned();
            return Err(PortError::new(
                Phase::PortData,
                ErrorType::MissingDependency,
                format!("horizon-port-pack {sub} exited non-zero: {}", stderr.trim()),
            )
            .with_suggestion(
                "run `horizon-port-pack check <jar>` manually and inspect the report",
            ));
        }
    }

    // Collect the produced .hmod, if any.
    let mut hmod: Option<PathBuf> = None;
    for e in std::fs::read_dir(&work).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().map(|x| x == "hmod").unwrap_or(false) {
            let dest = out_dir.join(
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("{mod_id}.hmod")),
            );
            let dest = if dest.extension().map(|x| x == "hmod").unwrap_or(false) {
                dest
            } else {
                out_dir.join(format!("{mod_id}.hmod"))
            };
            std::fs::create_dir_all(out_dir).ok();
            std::fs::copy(&p, &dest).ok();
            hmod = Some(dest);
            break;
        }
    }

    let _ = std::fs::remove_dir_all(&work);
    Ok(DataPortOutcome {
        hmod,
        carrier: "horizon-port-pack".to_string(),
        carried: Vec::new(),
        dropped: Vec::new(),
        errors: Vec::new(),
        port_pack_stdout: last_stdout,
    })
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

    // horizon-mod.toml (content-mod manifest, spec §0.6)
    let manifest = format!(
        "schema = 1\nid = \"{mod_id}\"\nversion = \"{version}\"\nname = \"{name}\"\ndescription = \"{description}\"\n"
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
