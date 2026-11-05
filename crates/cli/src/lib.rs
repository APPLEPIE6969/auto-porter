//! auto-porter CLI orchestration (spec §7).
//!
//! The binary is `auto-porter` (`crates/cli` [[bin]]); this library holds the
//! pipeline so integration tests can drive it in-process.

use classify::ClassifyOutput;
use port_code::sources::JavaSources;
use shared::catalog::HookCatalog;
use shared::config::PorterConfig;
use shared::error::PortError;
use shared::report::*;
use shared::types::*;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct PortOptions {
    pub horizon_revision: Option<String>,
    pub data_only: bool,
    pub output: PathBuf,
    pub verbose: bool,
    pub hooks_json: Option<PathBuf>,
    /// Class names that exist in the other jars of the same pack: cross-mod
    /// compat mixin targets resolve against them instead of false-failing
    /// the resolvable-targets gate (an API mod's class referenced by another
    /// mod's @Mixin is real, just not inside this jar).
    pub pack_classes: Vec<String>,
    /// Real upstream Java source tree (fetched from the mod's GitHub repo
    /// via its Modrinth metadata). When present, the LLM fallback prompt
    /// carries the actual upstream code instead of decompiled signatures.
    pub source_dir: Option<PathBuf>,
}

pub struct PortOutput {
    pub report: PortReport,
    pub report_path: Option<PathBuf>,
}

/// `auto-porter analyze` — decompose + classify only; writes
/// `decomposed.json` and `classified.json`.
pub fn analyze(
    jar: &Path,
    out: &Path,
    hooks_json: Option<&Path>,
) -> Result<ClassifyOutput, String> {
    let decompiled = decompose::decompose(jar)?;
    let catalog = HookCatalog::load_or_snapshot(hooks_json)?;
    let classified = classify::classify(&decompiled.mod_ir, &catalog);

    std::fs::create_dir_all(out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    shared::util::write_json_canonical(&out.join("decomposed.json"), &decompiled.mod_ir)
        .map_err(|e| format!("write decomposed.json: {e}"))?;
    shared::util::write_json_canonical(
        &out.join("classified.json"),
        &serde_json::json!({
            "plan": classified.plan,
            "gaps": classified.gaps,
            "unresolved_hooks": classified.unresolved_hooks,
        }),
    )
    .map_err(|e| format!("write classified.json: {e}"))?;
    Ok(classified)
}

/// `auto-porter port` — the full five-phase pipeline.
pub fn port(jar: &Path, cfg: &PorterConfig, opts: &PortOptions) -> Result<PortOutput, String> {
    let out_dir = &opts.output;
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;

    // ---- Phase 1: Decompose
    let decompiled = decompose::decompose_with(jar, &opts.pack_classes)?;
    let mod_ir = &decompiled.mod_ir;
    if opts.verbose {
        eprintln!(
            "[decompose] {} {} ({}): {} classes, {} mixins, {} events, {} data files",
            mod_ir.metadata.id,
            mod_ir.metadata.version,
            mod_ir.metadata.loader,
            mod_ir.classes.len(),
            mod_ir.mixins.len(),
            mod_ir.events.len(),
            mod_ir.data_files.len()
        );
    }

    // ---- Phase 2: Classify
    let catalog = HookCatalog::load_or_snapshot(opts.hooks_json.as_deref())?;
    let mut classified = classify::classify(mod_ir, &catalog);
    if opts.verbose {
        eprintln!(
            "[classify] tiers {:?}, {} gap(s), unresolved hooks: {:?}",
            classified.plan.tier_summary,
            classified.gaps.len(),
            classified.unresolved_hooks
        );
    }

    let mut report = PortReport::new(
        &mod_ir.metadata.id,
        &mod_ir.metadata.loader.to_string(),
        &jar.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let mut errors: Vec<PortError> = decompiled.gate_errors.clone();

    // An explicitly passed but empty --horizon-revision (e.g. CI expanding an
    // unset repo variable) means "not provided", never "pin to ''".
    let horizon_revision = opts
        .horizon_revision
        .clone()
        .filter(|s| !s.trim().is_empty());

    // ---- Phase 3: Port data
    let data = port_data::port_data(jar, mod_ir, cfg, out_dir);
    report.data_carry = Some(data.carrier.clone());
    if opts.verbose {
        eprintln!(
            "[port-data] carrier {} carried {} file(s), dropped {}",
            data.carrier,
            data.carried.len(),
            data.dropped.len()
        );
    }

    // ---- Phase 4: Port code (skipped for --data-only)
    let code = if opts.data_only {
        port_code::CodeGenOutcome {
            hnmod: None,
            outcomes: Vec::new(),
            errors: Vec::new(),
            lint_clean: true,
            compile: CompileResult {
                status: CheckStatus::Skipped,
                reason: Some("--data-only: code generation not attempted".to_string()),
            },
            revision_verified: false,
            hooks_used: Vec::new(),
        }
    } else {
        let revision = horizon_revision
            .clone()
            .unwrap_or_else(|| cfg.horizon.revision.clone());
        // Real upstream source when the pack flow (or --source) staged it;
        // empty otherwise, which keeps the prompt on decompiled inventory.
        let sources = match &opts.source_dir {
            Some(dir) => JavaSources::load(dir),
            None => JavaSources::empty(),
        };
        let outcome = port_code::port_code(
            &classified.plan,
            mod_ir,
            &catalog,
            cfg,
            Some(&revision),
            out_dir,
            opts.verbose,
            &sources,
        );
        if opts.verbose {
            eprintln!(
                "[port-code] {} hook(s) used, {} module(s) generated, lint {}",
                outcome.hooks_used.len(),
                outcome
                    .outcomes
                    .iter()
                    .filter(|o| o.status == FeatureStatus::Ported)
                    .count(),
                if outcome.lint_clean {
                    "clean"
                } else {
                    "FAILED"
                }
            );
        }
        outcome
    };

    report.horizon_revision = Some(
        horizon_revision
            .clone()
            .unwrap_or_else(|| cfg.horizon.revision.clone()),
    );
    report.horizon_revision_verified = code.revision_verified
        && shared::config::is_real_sha(&report.horizon_revision.clone().unwrap_or_default());

    // ---- outputs
    if let Some(hmod) = &data.hmod {
        report.outputs.hmod = hmod.file_name().map(|n| n.to_string_lossy().into_owned());
    }
    if let Some(hnmod) = &code.hnmod {
        report.outputs.hnmod = hnmod.file_name().map(|n| n.to_string_lossy().into_owned());
    }

    // ---- feature outcomes: classify plan merged with the phase results
    for feature in &classified.plan.features {
        let code_outcome = code.outcomes.iter().find(|o| o.feature_id == feature.id);
        let (status, detail) = if feature.kind == FeatureKind::DataOnly {
            if data.hmod.is_some() && data.dropped.is_empty() {
                (
                    FeatureStatus::Ported,
                    format!(
                        "{} data file(s) carried via {}",
                        data.carried.len(),
                        data.carrier
                    ),
                )
            } else if data.hmod.is_some() {
                (
                    FeatureStatus::Partial,
                    format!(
                        "carried with {} drop(s) via {}",
                        data.dropped.len(),
                        data.carrier
                    ),
                )
            } else {
                (FeatureStatus::Blocked, "no .hmod produced".to_string())
            }
        } else {
            match code_outcome {
                Some(o) => (o.status, o.detail.clone()),
                None => (
                    FeatureStatus::Skipped,
                    "not processed by the code phase".to_string(),
                ),
            }
        };
        report.features.push(FeatureOutcome {
            feature_id: feature.id.clone(),
            kind: feature.kind,
            tier: feature.tier,
            status,
            detail,
        });
    }

    errors.extend(data.errors.iter().cloned());
    errors.extend(code.errors.iter().cloned());
    report.errors = errors;

    // ---- tests directory (generated parity + in-game scenarios)
    if cfg.output.include_tests && !opts.data_only {
        // Tier A scenario generation (spec §6.2): harvest the mod's carried
        // data and generate place-block parity + in-game content steps +
        // craft recipes (runner-gap). Executable verbs only — a scenario
        // the runners would count as passing without checking is a lie.
        let tier_a = port_code::scenarios::harvest(jar, &mod_ir.data_files);
        if opts.verbose {
            eprintln!(
                "[scenarios] {} block(s), {} craft recipe(s), {} loot table(s)",
                tier_a.blocks.len(),
                tier_a.recipes.len(),
                tier_a.loot_tables
            );
        }
        {
            for f in classified
                .plan
                .features
                .iter_mut()
                .filter(|f| f.kind == FeatureKind::DataOnly)
            {
                f.parity_tests.extend(port_code::scenarios::parity_specs(
                    &mod_ir.metadata.id,
                    &tier_a,
                ));
            }
        }

        let tests_dir = out_dir.join(format!("{}-tests", mod_ir.metadata.id));
        let mut wrote = false;
        for feature in &classified.plan.features {
            for spec in &feature.parity_tests {
                let p = tests_dir.join("parity").join(format!("{}.yaml", spec.name));
                if std::fs::create_dir_all(p.parent().unwrap()).is_ok()
                    && std::fs::write(&p, &spec.scenario_yaml).is_ok()
                {
                    wrote = true;
                }
            }
        }
        // craft scenarios: complete but not executable by the parity runner
        // (no craft verb on either harness) — parked outside its glob
        for spec in port_code::scenarios::craft_specs(&mod_ir.metadata.id, &tier_a) {
            let p = tests_dir
                .join("parity-manual")
                .join(format!("{}.yaml", spec.name));
            if std::fs::create_dir_all(p.parent().unwrap()).is_ok() {
                let _ = std::fs::write(&p, &spec.scenario_yaml);
            }
        }
        let smoke = tests_dir
            .join("in-game")
            .join(format!("{}.yaml", mod_ir.metadata.id));
        if std::fs::create_dir_all(smoke.parent().unwrap()).is_ok() {
            let yaml = port_code::scenarios::smoke_yaml(&mod_ir.metadata.id, &tier_a);
            let _ = std::fs::write(&smoke, yaml);
            wrote = true;
        }
        if wrote {
            report.outputs.tests = Some(format!("{}-tests", mod_ir.metadata.id));
        }
    }

    report.finalize();

    // ---- write the report
    let mut report_path = None;
    if cfg.output.include_report {
        let p = out_dir.join(format!("{}-report.json", mod_ir.metadata.id));
        match shared::util::write_json_canonical(&p, &report) {
            Ok(()) => {
                report.outputs.report = p.file_name().map(|n| n.to_string_lossy().into_owned());
                report_path = Some(p);
            }
            Err(e) => return Err(format!("cannot write report: {e}")),
        }
    }

    Ok(PortOutput {
        report,
        report_path,
    })
}

/// `auto-porter port-pack` — a directory of jars, a Modrinth `.mrpack`, or a
/// CurseForge zip.
pub fn port_pack(
    input: &Path,
    cfg: &PorterConfig,
    opts: &PortOptions,
    continue_on_error: bool,
) -> Result<serde_json::Value, String> {
    std::fs::create_dir_all(&opts.output).map_err(|e| format!("cannot create output: {e}"))?;
    let mut results = Vec::new();
    let mut failures = 0u32;
    let mut ported = 0u32;

    let jars: Vec<PathBuf> = if input.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(input)
            .map_err(|e| format!("cannot read {}: {e}", input.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "jar").unwrap_or(false))
            .collect();
        v.sort();
        v
    } else if input.extension().map(|x| x == "jar").unwrap_or(false) {
        vec![input.to_path_buf()]
    } else {
        // .mrpack / CurseForge zip: handled by modpack_archives below, not as a jar
        Vec::new()
    };

    // Class names of every sibling jar: a compat mixin in jar A targeting an
    // API class from jar B is real and must not fail the targets gate.
    let dir_pack_classes =
        pack_class_index(&jars.iter().map(|p| p.as_path()).collect::<Vec<&Path>>());

    let pack_bin = cfg.expand(&cfg.horizon.port_pack_path);
    let pack_available = pack_bin.is_file()
        || std::env::var_os("PATH")
            .map(|paths| {
                std::env::split_paths(&paths).any(|d| d.join("horizon-port-pack").is_file())
            })
            .unwrap_or(false);

    for jar in &jars {
        let jar_opts = PortOptions {
            pack_classes: dir_pack_classes.clone(),
            ..opts.clone()
        };
        match port(jar, cfg, &jar_opts) {
            Ok(out) => {
                ported += 1;
                results.push(serde_json::json!({
                    "jar": jar.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    "mod_id": out.report.mod_id,
                    "overall": out.report.overall.as_str(),
                    "features": out.report.summary.total_features,
                    "errors": out.report.errors.len(),
                }));
            }
            Err(e) => {
                failures += 1;
                results.push(serde_json::json!({
                    "jar": jar.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    "error": e,
                }));
                if !continue_on_error {
                    break;
                }
            }
        }
    }

    // Modpack archives (.mrpack / CurseForge zip), spec §7. With the real
    // carrier installed the pack goes through `horizon-port-pack import
    // <archive> <work>` (the launcher's own import; verified against the
    // upstream CLI surface — there is no `port` subcommand). Without it,
    // auto-porter fills exactly the gap the mission names: parse the
    // manifest, download the listed jars (Modrinth packs carry public CDN
    // URLs pinned by sha1), verify the hashes, and port each mod through the
    // full pipeline. CurseForge manifests list files only as (projectID,
    // fileID) pairs that need the CF API — recorded honestly, never guessed.
    for archive in modpack_archives(input) {
        let name = archive
            .0
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "modpack".to_string());
        let work = opts.output.join(&name);
        if pack_available {
            let status = std::process::Command::new("horizon-port-pack")
                .arg("import")
                .arg(archive.0.display().to_string())
                .arg(work.display().to_string())
                .status();
            results.push(serde_json::json!({
                "archive": name,
                "carrier": "horizon-port-pack",
                "status": if status.map(|s| s.success()).unwrap_or(false) { "ported" } else { "failed" },
            }));
            continue;
        }

        let overrides = carry_modpack_overrides(&archive.1, &work);
        let mut entry = serde_json::json!({
            "archive": name,
            "carrier": "built-in-fallback",
            "overrides_carried": overrides,
        });
        match manifest_mods(&archive.1) {
            Some(("mrpack", mods)) if !mods.is_empty() => {
                let (mods_json, mod_ported, mod_failures) =
                    port_mrpack_mods(&mods, cfg, opts, continue_on_error, &work);
                ported += mod_ported;
                failures += mod_failures;
                entry["format"] = serde_json::json!("mrpack");
                entry["mods"] = serde_json::Value::Array(mods_json);
            }
            Some(("curseforge", mods)) => {
                entry["format"] = serde_json::json!("curseforge");
                entry["mods"] = serde_json::Value::Array(
                    mods.iter()
                        .map(|m| {
                            let (project, file) = m.curseforge.unwrap_or_default();
                            serde_json::json!({
                                "project_id": project,
                                "file_id": file,
                                "status": "requires-curseforge-api",
                                "note": "CurseForge manifests name files only by ids; \
                                         downloading them needs the CF API (key-gated), \
                                         so this mod is recorded, not ported",
                            })
                        })
                        .collect(),
                );
            }
            _ => {
                entry["note"] = serde_json::json!(
                    "no manifest mods recognized; overrides carried as data only"
                );
            }
        }
        results.push(entry);
    }

    let summary = serde_json::json!({
        "input": input.display().to_string(),
        "ported": ported,
        "failures": failures,
        "results": results,
    });
    let summary_path = opts.output.join("port-pack-summary.json");
    shared::util::write_json_canonical(&summary_path, &summary)
        .map_err(|e| format!("cannot write summary: {e}"))?;
    Ok(summary)
}

fn read_zip_pub(path: &Path) -> Result<std::collections::BTreeMap<String, Vec<u8>>, String> {
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

fn modpack_archives(input: &Path) -> Vec<(PathBuf, std::collections::BTreeMap<String, Vec<u8>>)> {
    let mut out = Vec::new();
    let candidates: Vec<PathBuf> = if input.is_dir() {
        std::fs::read_dir(input)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| {
                        matches!(
                            p.extension().and_then(|x| x.to_str()),
                            Some("mrpack") | Some("zip")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![input.to_path_buf()]
    };
    for path in candidates {
        if let Ok(entries) = read_zip_pub(&path) {
            if entries.contains_key("modrinth.index.json") || entries.contains_key("manifest.json")
            {
                out.push((path, entries));
            }
        }
    }
    out
}

/// Carry the override trees of a Modrinth/CurseForge archive (data only;
/// the real import/carry pipeline lives in horizon-port-pack). Modrinth
/// allows three roots — `overrides/`, `client-overrides/`,
/// `server-overrides/` (aligned with the upstream importer); CurseForge
/// uses the root named by `manifest.json["overrides"]`.
fn carry_modpack_overrides(
    entries: &std::collections::BTreeMap<String, Vec<u8>>,
    dest: &Path,
) -> std::collections::BTreeMap<String, usize> {
    let roots = ["overrides", "client-overrides", "server-overrides"];
    let mut carried = std::collections::BTreeMap::new();
    for (name, bytes) in entries {
        for root in roots {
            let Some(rel) = name
                .strip_prefix(&format!("{root}/"))
                .filter(|_| !name.ends_with('/'))
            else {
                continue;
            };
            let target = dest.join(rel);
            if std::fs::create_dir_all(target.parent().unwrap_or(dest)).is_ok()
                && std::fs::write(&target, bytes).is_ok()
            {
                *carried.entry(root.to_string()).or_default() += 1;
            }
        }
    }
    carried
}

/// One mod file referenced by a modpack manifest (spec §7 modpack inputs).
struct ManifestMod {
    /// Instance path, e.g. `mods/veinminer.jar`.
    path: String,
    /// Direct download URLs (Modrinth packs; empty for CurseForge ids).
    downloads: Vec<String>,
    /// Expected sha1 hex, when the manifest pins it.
    sha1: Option<String>,
    /// Declared size in bytes, when the manifest pins it.
    size: Option<u64>,
    /// CurseForge (projectID, fileID) — no public download without the API.
    curseforge: Option<(u64, u64)>,
}

/// Detect a modpack archive's manifest and list the mods it references.
/// Returns `(format, mods)` — `mrpack` for `modrinth.index.json`,
/// `curseforge` for `manifest.json` — or `None` when neither is present.
fn manifest_mods(
    entries: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Option<(&'static str, Vec<ManifestMod>)> {
    if let Some(bytes) = entries.get("modrinth.index.json") {
        let index: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let mods = index["files"]
            .as_array()
            .map(|files| {
                files
                    .iter()
                    .filter_map(|f| {
                        let path = f.get("path")?.as_str()?.to_string();
                        let downloads = f
                            .get("downloads")
                            .and_then(|d| d.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|u| u.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let sha1 = f
                            .get("hashes")
                            .and_then(|h| h.get("sha1"))
                            .and_then(|s| s.as_str())
                            .map(String::from);
                        let size = f.get("fileSize").and_then(|s| s.as_u64());
                        Some(ManifestMod {
                            path,
                            downloads,
                            sha1,
                            size,
                            curseforge: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Some(("mrpack", mods));
    }
    if let Some(bytes) = entries.get("manifest.json") {
        let manifest: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let mods = manifest["files"]
            .as_array()
            .map(|files| {
                files
                    .iter()
                    .filter_map(|f| {
                        let project = f.get("projectID")?.as_u64()?;
                        let file = f.get("fileID")?.as_u64()?;
                        Some(ManifestMod {
                            path: String::new(),
                            downloads: Vec::new(),
                            sha1: None,
                            size: None,
                            curseforge: Some((project, file)),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Some(("curseforge", mods));
    }
    None
}

/// Timeout for one manifest-mod download (mods are a few MB at most).
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;

/// Download a manifest mod's jar and verify it against the manifest pins
/// (sha1 hex and declared size, when present). Bytes live in memory and are
/// written only after verification — a tampered or truncated file is never
/// handed to the pipeline.
fn fetch_manifest_jar(mod_: &ManifestMod, url: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .build();
    let resp = match agent.get(url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => return Err(format!("HTTP {code}")),
        Err(ureq::Error::Transport(t)) => return Err(format!("transport: {t}")),
    };
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes)
        .map_err(|e| format!("read body: {e}"))?;
    if let Some(size) = mod_.size {
        if bytes.len() as u64 != size {
            return Err(format!(
                "size mismatch: manifest pins {size} bytes, downloaded {}",
                bytes.len()
            ));
        }
    }
    if let Some(expected) = &mod_.sha1 {
        let actual = sha1_hex(&bytes);
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "sha1 mismatch: manifest pins {expected}, downloaded file hashes to {actual}"
            ));
        }
    }
    Ok(bytes)
}

fn sha1_hex(bytes: &[u8]) -> String {
    use sha1::Digest;
    sha1::Sha1::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Download and port every jar a Modrinth manifest lists, in three phases:
///
/// 1. download + verify every `mods/*.jar` (bytes staged only after the
///    pinned sha1/size check), fetching each mod's upstream source from the
///    GitHub repo its Modrinth metadata links;
/// 2. build the sibling class index over the staged jars (cross-mod compat
///    mixin targets resolve instead of false-failing the targets gate);
/// 3. port each staged mod through the full pipeline.
///
/// Returns `(result entries, ported, failures)`; a download or hash failure
/// is a recorded failure (spec §9) that either aborts the pack or, with
/// `continue_on_error`, lets the rest move on.
fn port_mrpack_mods(
    mods: &[ManifestMod],
    cfg: &PorterConfig,
    opts: &PortOptions,
    continue_on_error: bool,
    work: &Path,
) -> (Vec<serde_json::Value>, u32, u32) {
    let mut results = Vec::new();
    let (mut ported, mut failures) = (0u32, 0u32);

    // ---- Phase A: downloads + upstream source
    struct Staged {
        jar_path: PathBuf,
        entry: serde_json::Value,
    }
    let mut staged: Vec<Staged> = Vec::new();
    for mod_ in mods {
        // Only `mods/*.jar` entries are code mods for this pipeline. A
        // Modrinth manifest can also list e.g. `resourcepacks/*.zip`; those
        // are data the launcher import / real carrier stages on its own —
        // pushing them through the Java pipeline would misreport them as
        // "ported" (seen for real with Fabulously Optimized's resource
        // packs), so they are recorded with their kind instead.
        if !(mod_.path.starts_with("mods/") && mod_.path.ends_with(".jar")) {
            let kind = if mod_.path.starts_with("resourcepacks/") {
                "resource-pack"
            } else {
                "non-mod-file"
            };
            results.push(serde_json::json!({
                "mod": mod_.path,
                "status": kind,
                "note": "staged by the launcher import / horizon-port-pack; \
                         not a code mod for this pipeline",
            }));
            continue;
        }
        let file_name = mod_
            .path
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or("mod.jar")
            .to_string();
        let Some(url) = mod_.downloads.first().cloned() else {
            failures += 1;
            results.push(serde_json::json!({
                "mod": mod_.path,
                "error": "manifest lists no direct download URL",
            }));
            if !continue_on_error {
                return (results, ported, failures);
            }
            continue;
        };
        let fail = |msg: String| {
            serde_json::json!({
                "mod": mod_.path,
                "source": url,
                "error": msg,
            })
        };
        match fetch_manifest_jar(mod_, &url) {
            Ok(bytes) => {
                let jar_path = work.join("mods").join(&file_name);
                if let Err(e) = std::fs::create_dir_all(jar_path.parent().unwrap_or(work))
                    .and_then(|_| std::fs::write(&jar_path, &bytes))
                {
                    failures += 1;
                    results.push(fail(format!("cannot stage jar: {e}")));
                    if !continue_on_error {
                        return (results, ported, failures);
                    }
                    continue;
                }
                // Upstream source: resolved from the Modrinth metadata the
                // CDN URL points at; every outcome recorded, none fatal.
                let stem = file_name.trim_end_matches(".jar");
                let code_source = fetch_upstream_source(&url, &work.join("sources").join(stem));
                let entry = serde_json::json!({
                    "mod": mod_.path,
                    "source": url,
                    "sha1": if mod_.sha1.is_some() { "verified" } else { "unpinned" },
                    "code_source": code_source,
                });
                staged.push(Staged { jar_path, entry });
            }
            Err(e) => {
                failures += 1;
                results.push(fail(e));
                if !continue_on_error {
                    return (results, ported, failures);
                }
            }
        }
    }

    // ---- Phase B: sibling class index over the staged jars
    let jar_refs: Vec<&Path> = staged.iter().map(|s| s.jar_path.as_path()).collect();
    let pack_classes = pack_class_index(&jar_refs);

    // ---- Phase C: full pipeline per staged mod
    for s in staged {
        let mod_opts = PortOptions {
            pack_classes: pack_classes.clone(),
            output: work.to_path_buf(),
            ..opts.clone()
        };
        let mut entry = s.entry;
        match port(&s.jar_path, cfg, &mod_opts) {
            Ok(out) => {
                ported += 1;
                entry["mod_id"] = serde_json::json!(out.report.mod_id);
                entry["overall"] = serde_json::json!(out.report.overall.as_str());
                entry["features"] = serde_json::json!(out.report.summary.total_features);
                entry["errors"] = serde_json::json!(out.report.errors.len());
            }
            Err(e) => {
                failures += 1;
                entry["error"] = serde_json::json!(e);
            }
        }
        results.push(entry);
    }
    (results, ported, failures)
}

/// Class names present in the given jars (entry names only — no parsing):
/// `a/b/C.class` -> `a/b/C`. Sibling-jar knowledge for the mixin-targets
/// gate: a compat mixin's target class lives in the API mod's jar, not the
/// mixin author's.
fn pack_class_index(jars: &[&Path]) -> Vec<String> {
    let mut out = std::collections::BTreeSet::new();
    for jar in jars {
        let Ok(file) = std::fs::File::open(jar) else {
            continue;
        };
        let Ok(mut archive) = zip::ZipArchive::new(file) else {
            continue;
        };
        for i in 0..archive.len() {
            let Ok(zf) = archive.by_index(i) else {
                continue;
            };
            if zf.is_dir() {
                continue;
            }
            let name = zf.name().trim_start_matches("./");
            let Some(cls) = name.strip_suffix(".class") else {
                continue;
            };
            if cls.ends_with("module-info") || cls.ends_with("package-info") {
                continue;
            }
            out.insert(cls.to_string());
        }
    }
    out.into_iter().collect()
}

/// The Modrinth project id embedded in a CDN download URL:
/// `https://cdn.modrinth.com/data/<project id>/versions/…`.
fn modrinth_project_id(url: &str) -> Option<String> {
    let rest = url.split("/data/").nth(1)?;
    let id = rest.split('/').next()?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())).then(|| id.to_string())
}

/// `https://github.com/<owner>/<repo>[.git][/tree/…]` -> `owner/repo`, for
/// github.com hosts only. Anything else is recorded, never guessed at.
fn github_repo_from_source_url(source_url: &str) -> Option<String> {
    let rest = source_url
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| source_url.trim().strip_prefix("http://github.com/"))?;
    let mut segs = rest.trim_end_matches('/').split('/');
    let owner = segs.next()?.trim();
    let repo = segs.next()?.trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    Some(format!("{owner}/{repo}"))
}

fn modrinth_api_base() -> String {
    std::env::var("AUTO_PORTER_MODRINTH_API").unwrap_or_else(|_| "https://api.modrinth.com".into())
}

fn github_codeload_base() -> String {
    std::env::var("AUTO_PORTER_GITHUB_CODELOAD")
        .unwrap_or_else(|_| "https://codeload.github.com".into())
}

/// Total bytes extracted from one source tarball (repositories can be
/// arbitrarily large; the pipeline needs the Java sources, not the vendoring).
const SOURCE_EXTRACT_CAP: u64 = 64 * 1024 * 1024;

/// Resolve a mod's upstream source and stage it under `dest`. The project id
/// comes from the Modrinth CDN URL, the repo link from the public Modrinth
/// API, and the code from GitHub's default-branch tarball (codeload, no API
/// key). Every outcome is recorded — a missing link or a non-GitHub host is
/// data for the report, not a pipeline error.
fn fetch_upstream_source(download_url: &str, dest: &Path) -> serde_json::Value {
    let Some(project_id) = modrinth_project_id(download_url) else {
        return serde_json::Value::Null;
    };
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .user_agent(&format!("auto-porter/{}", env!("CARGO_PKG_VERSION")))
        .build();
    let project: serde_json::Value = match agent
        .get(&format!(
            "{}/v2/project/{}",
            modrinth_api_base(),
            project_id
        ))
        .call()
    {
        Ok(resp) => match resp.into_json() {
            Ok(v) => v,
            Err(e) => {
                return serde_json::json!({
                    "status": "fetch-failed",
                    "detail": format!("modrinth api body: {e}")
                })
            }
        },
        Err(ureq::Error::Status(code, _)) => {
            return serde_json::json!({
                "status": "fetch-failed",
                "detail": format!("modrinth api HTTP {code}")
            })
        }
        Err(ureq::Error::Transport(t)) => {
            return serde_json::json!({
                "status": "fetch-failed",
                "detail": format!("modrinth api: {t}")
            })
        }
    };
    let source_url = project
        .get("source_url")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if source_url.is_empty() {
        return serde_json::json!({ "status": "no-source-url" });
    }
    let Some(repo) = github_repo_from_source_url(&source_url) else {
        return serde_json::json!({ "status": "non-github", "repo": source_url });
    };

    let tar_url = format!("{}/{}/tar.gz/HEAD", github_codeload_base(), repo);
    let bytes = match agent.get(&tar_url).call() {
        Ok(resp) => {
            let mut bytes = Vec::new();
            if std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes).is_err() {
                return serde_json::json!({
                    "status": "fetch-failed",
                    "repo": repo,
                    "detail": "could not read the source tarball body"
                });
            }
            bytes
        }
        Err(ureq::Error::Status(code, _)) => {
            return serde_json::json!({
                "status": "fetch-failed",
                "repo": repo,
                "detail": format!("codeload HTTP {code}")
            })
        }
        Err(ureq::Error::Transport(t)) => {
            return serde_json::json!({
                "status": "fetch-failed",
                "repo": repo,
                "detail": format!("codeload: {t}")
            })
        }
    };

    if let Err(e) = extract_tar_gz(&bytes, dest) {
        return serde_json::json!({
            "status": "fetch-failed",
            "repo": repo,
            "detail": e
        });
    }
    flatten_single_root(dest);
    if !dir_has_java(dest) {
        return serde_json::json!({ "status": "no-java-sources", "repo": repo });
    }
    serde_json::json!({
        "status": "fetched",
        "repo": repo,
        "dir": dest.display().to_string()
    })
}

/// Extract a gzipped tarball (the codeload default-branch archive) into
/// `dest`. Guardrails: absolute paths and `..` components are dropped, and
/// the total extracted size is capped at [`SOURCE_EXTRACT_CAP`].
fn extract_tar_gz(bytes: &[u8], dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    let mut total: u64 = 0;
    let entries = archive.entries().map_err(|e| format!("tar: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("tar: {e}"))?;
        let path = entry.path().map_err(|e| format!("tar: {e}"))?.into_owned();
        let cleaned: PathBuf = path
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect();
        if cleaned.as_os_str().is_empty() {
            continue;
        }
        if entry.header().entry_type().is_dir() {
            let _ = std::fs::create_dir_all(dest.join(&cleaned));
            continue;
        }
        let size = entry.size();
        if total + size > SOURCE_EXTRACT_CAP {
            continue;
        }
        let target = dest.join(&cleaned);
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if entry.unpack(&target).is_ok() {
            total += size;
        }
    }
    Ok(())
}

/// codeload archives carry a single `<repo>-<ref>/` root; move its children
/// up one level so the staged tree starts at the repo root.
fn flatten_single_root(dir: &Path) {
    let Ok(mut entries) = std::fs::read_dir(dir) else {
        return;
    };
    let first = match entries.next() {
        Some(Ok(e)) => e.path(),
        _ => return,
    };
    if entries.next().is_some() || !first.is_dir() {
        return;
    }
    if let Ok(children) = std::fs::read_dir(&first) {
        for child in children.filter_map(|c| c.ok()) {
            let name = child.file_name();
            let _ = std::fs::rename(child.path(), dir.join(&name));
        }
    }
    let _ = std::fs::remove_dir(&first);
}

fn dir_has_java(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().map(|x| x == "java").unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

/// `auto-porter verify` — three layers (spec §6). Single-artifact-set form:
/// scans `dir` for one `<mod-id>.hmod` / `.hnmod` / `-report.json` group.
pub fn verify_dir(
    dir: &Path,
    cfg: &PorterConfig,
    do_static: bool,
    do_compile: bool,
    do_parity: bool,
    do_in_game: bool,
    verbose: bool,
) -> Result<VerificationReport, String> {
    // find the artifacts in the dir
    let mut hmod_path = None;
    let mut hnmod_path = None;
    let mut mod_id = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mod".to_string());
    for entry in
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        match p.extension().and_then(|x| x.to_str()) {
            Some("hmod") => hmod_path = Some(p.clone()),
            Some("hnmod") => hnmod_path = Some(p.clone()),
            _ => {}
        }
        if let Some(stem) = p.file_name().and_then(|n| {
            n.to_string_lossy()
                .into_owned()
                .strip_suffix("-report.json")
                .map(|s| s.to_string())
        }) {
            mod_id = stem;
        }
    }
    verify_group(
        dir, &mod_id, hmod_path, hnmod_path, cfg, do_static, do_compile, do_parity, do_in_game,
        verbose,
    )
}

/// Outcome of a multi-mod `verify` run (spec §8.2 sweep form).
pub struct VerifyOutcome {
    /// One verification report per artifact group found in the directory.
    pub reports: Vec<VerificationReport>,
    /// Groups that could not be verified at all (only with
    /// `continue_on_error`, which keeps the sweep moving).
    pub hard_errors: Vec<String>,
}

/// `auto-porter verify <dir> --continue-on-error` — verify every artifact
/// group in a flat output directory (the ecosystem-sweep layout: N mods each
/// contributing `<id>.hmod` / `<id>.hnmod` / `<id>-report.json`). With
/// `continue_on_error` a failing group is recorded and the sweep moves on;
/// without it the first failure aborts.
#[allow(clippy::too_many_arguments)]
pub fn verify_output_dir(
    dir: &Path,
    cfg: &PorterConfig,
    do_static: bool,
    do_compile: bool,
    do_parity: bool,
    do_in_game: bool,
    verbose: bool,
    continue_on_error: bool,
) -> Result<VerifyOutcome, String> {
    // group artifacts by their `<mod-id>` prefix
    let mut groups: std::collections::BTreeMap<String, (Option<PathBuf>, Option<PathBuf>)> =
        Default::default();
    for entry in
        std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        if !p.is_file() {
            continue; // `-tests/` dirs and friends are not artifact groups
        }
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
        let stem = name.as_deref().and_then(|n| {
            n.strip_suffix(".hmod")
                .or_else(|| n.strip_suffix(".hnmod"))
                .or_else(|| n.strip_suffix("-report.json"))
                .map(|s| s.to_string())
        });
        if let Some(id) = stem {
            let g = groups.entry(id).or_default();
            match p.extension().and_then(|x| x.to_str()) {
                Some("hmod") => g.0 = Some(p.clone()),
                Some("hnmod") => g.1 = Some(p.clone()),
                _ => {} // the -report.json side is read inside verify_group
            }
        }
    }
    if groups.is_empty() {
        // legacy single-mod layout: keep the old dir-scan semantics
        let r = verify_dir(
            dir, cfg, do_static, do_compile, do_parity, do_in_game, verbose,
        )?;
        return Ok(VerifyOutcome {
            reports: vec![r],
            hard_errors: Vec::new(),
        });
    }
    let mut reports = Vec::new();
    let mut hard_errors = Vec::new();
    for (id, (hmod, hnmod)) in groups {
        match verify_group(
            dir, &id, hmod, hnmod, cfg, do_static, do_compile, do_parity, do_in_game, verbose,
        ) {
            Ok(v) => reports.push(v),
            Err(e) => {
                let msg = format!("{id}: {e}");
                if continue_on_error {
                    hard_errors.push(msg);
                } else {
                    return Err(msg);
                }
            }
        }
    }
    reports.sort_by(|a, b| a.mod_id.cmp(&b.mod_id));
    hard_errors.sort();
    Ok(VerifyOutcome {
        reports,
        hard_errors,
    })
}

/// Verify one artifact group (all files share the `<mod-id>` prefix) inside
/// `dir` and write `<mod-id>-verification.json` next to them.
#[allow(clippy::too_many_arguments)]
fn verify_group(
    dir: &Path,
    mod_id: &str,
    hmod_path: Option<PathBuf>,
    hnmod_path: Option<PathBuf>,
    cfg: &PorterConfig,
    do_static: bool,
    do_compile: bool,
    do_parity: bool,
    do_in_game: bool,
    verbose: bool,
) -> Result<VerificationReport, String> {
    let catalog = HookCatalog::load_or_snapshot(None).ok();

    // tier from the pipeline report when present
    let pipeline = std::fs::read_to_string(dir.join(format!("{mod_id}-report.json")))
        .ok()
        .and_then(|t| serde_json::from_str::<PortReport>(&t).ok());
    let tier = pipeline.as_ref().and_then(|r| {
        ["A", "B", "C", "D"]
            .iter()
            .find(|t| r.summary.tiers.contains_key(**t))
            .map(|t| t.to_string())
    });

    // ---- static layer
    let mut issues: Vec<StaticIssue> = Vec::new();
    if do_static {
        if let Some(hmod) = &hmod_path {
            issues.extend(verify::hmod::validate(hmod).issues);
        }
        if let Some(hnmod) = &hnmod_path {
            let expected = catalog.as_ref().map(|c| c.native_hooks_api);
            issues.extend(verify::hnmod::validate(hnmod, catalog.as_ref(), expected).issues);
        }
        if hmod_path.is_none() && hnmod_path.is_none() {
            issues.push(StaticIssue {
                code: "no_artifacts".to_string(),
                severity: CheckStatus::Fail,
                message: format!("no .hmod/.hnmod found in {}", dir.display()),
            });
        }
    } else {
        issues.push(StaticIssue {
            code: "static_skipped".to_string(),
            severity: CheckStatus::Skipped,
            message: "--static not requested".to_string(),
        });
    }

    let schema_status = status_of(
        &issues,
        &[
            "top_level_entry",
            "zip_read",
            "manifest_",
            "json_parse",
            "missing_texture",
            "missing_sound",
            "bundle_layout",
            "mixin_toml",
            "injection_",
            "no_artifacts",
            "static_skipped",
        ],
    );
    let hooks_status = status_of(&issues, &["hook_", "api_version_mismatch", "source_lint"]);
    let deps_status = status_of(&issues, &["dep_"]);

    // ---- compile layer: the launcher-style build, executed headlessly
    // (spec §5.2.4, docs/limitations.md §3). Skipped with the missing
    // prerequisite named when the gate cannot run; never pretended.
    let compile = if do_compile {
        match hnmod_path {
            Some(ref bundle) => verify::compile::run_bundle(bundle, cfg, verbose),
            None => CompileResult {
                status: CheckStatus::Skipped,
                reason: Some("no .hnmod in this artifact group; nothing to compile".to_string()),
            },
        }
    } else {
        CompileResult {
            status: CheckStatus::Skipped,
            reason: Some("--compile not requested".to_string()),
        }
    };

    // ---- parity / in-game layers
    let (parity, mut v_errors) = if do_parity {
        verify::runner::run_parity(cfg, dir, verbose)
    } else {
        (
            ParityResult {
                status: CheckStatus::Skipped,
                scenarios_run: 0,
                scenarios_passed: 0,
                scenarios_failed: 0,
                failures: Vec::new(),
                note: Some("--parity not requested".to_string()),
            },
            Vec::new(),
        )
    };
    let (in_game, mut g_errors) = if do_in_game {
        verify::runner::run_in_game(cfg, dir, verbose)
    } else {
        (
            InGameResult {
                status: CheckStatus::Skipped,
                steps_run: 0,
                steps_passed: 0,
                screenshot_ssim: None,
                note: Some("--in-game not requested".to_string()),
            },
            Vec::new(),
        )
    };
    v_errors.append(&mut g_errors);
    let _ = v_errors; // surfaced via the statuses above

    // ---- overall
    let any_fail = issues.iter().any(|i| i.severity == CheckStatus::Fail)
        || parity.status == CheckStatus::Fail
        || in_game.status == CheckStatus::Fail
        || compile.status == CheckStatus::Fail;
    let any_warn = issues.iter().any(|i| i.severity == CheckStatus::Warn)
        || parity.status == CheckStatus::Skipped
        || in_game.status == CheckStatus::Skipped
        || compile.status == CheckStatus::Skipped;
    let overall = if any_fail {
        Overall::Failed
    } else if any_warn {
        Overall::PassWithWarnings
    } else {
        Overall::Pass
    };

    let mut human_review = Vec::new();
    if parity.status == CheckStatus::Skipped {
        human_review.push("parity layer skipped in this environment".to_string());
    }
    if in_game.status == CheckStatus::Skipped {
        human_review.push("in-game smoke skipped in this environment".to_string());
    }
    if compile.status == CheckStatus::Skipped {
        human_review.push(format!(
            "compile gate skipped: {}",
            compile.reason.as_deref().unwrap_or("no reason given")
        ));
    }

    let verification = VerificationReport {
        mod_id: mod_id.to_string(),
        tier,
        r#static: StaticResult {
            schema: schema_status,
            hooks: hooks_status,
            deps: deps_status,
            issues,
        },
        compile,
        parity,
        in_game,
        overall,
        human_review_needed: human_review,
    };

    shared::util::write_json_canonical(
        &dir.join(format!("{mod_id}-verification.json")),
        &verification,
    )
    .map_err(|e| format!("cannot write verification report: {e}"))?;

    Ok(verification)
}

fn status_of(issues: &[StaticIssue], codes: &[&str]) -> CheckStatus {
    let mut status = CheckStatus::Pass;
    for issue in issues {
        if codes.iter().any(|c| issue.code.starts_with(c)) {
            match issue.severity {
                CheckStatus::Fail => return CheckStatus::Fail,
                CheckStatus::Warn => status = CheckStatus::Warn,
                _ => {}
            }
        }
    }
    status
}
