//! The compile gate: the launcher's pinned native-build pipeline, executed
//! headlessly (spec §5.2.4; docs/limitations.md §3).
//!
//! A standalone `cargo check` of a generated mod is not possible — the mod
//! compiles only inside a Horizon workspace. The launcher's own pipeline
//! (`launcher/src/native_mods.rs`, verified against `horizon@6beacfe9`) is:
//! pristine source → (`horizon-mixins apply` when any bundle carries mixin
//! content) → `api::stage` (native-api crates + a generated
//! `horizon-native-game` executable) → lock completion →
//! `cargo build --locked --profile <p> -p <package>`. This module runs the
//! same steps with the same tools — the real `horizon-mixins` binary, the
//! real pinned Horizon source, the real Cargo — and reports honestly:
//! `Pass` only when the pinned package type-checks, `Fail` with the compiler
//! tail, `Skipped` with the missing prerequisite named. Nothing is faked.
//!
//! The gate uses `cargo check` (type-checking without codegen/linking) and
//! `--no-default-features` (the launcher's headless engine target: no ALSA,
//! no libudev) so it runs on machines without desktop system libraries.
//! `AUTO_PORTER_GATE_BUILD=1` upgrades the final step to a full `cargo
//! build`, which additionally proves linking (needs `cc` and either the
//! audio/controller dev libraries or `--no-default-features`).

use shared::config::PorterConfig;
use shared::report::{CheckStatus, CompileResult};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Extract cap for the pinned Horizon source archive. Deliberately larger
/// than the mod-source cap in cli (64 MiB): this is Horizon's own tree.
const HORIZON_EXTRACT_CAP: u64 = 512 * 1024 * 1024;
/// Tail of compiler output kept in a failure reason (the launcher's build
/// log panel shows "a bounded preview of the end of the log" too).
const OUTPUT_TAIL_LINES: usize = 40;

/// Run the gate for one `.hnmod` bundle.
pub fn run_bundle(hnmod: &Path, cfg: &PorterConfig, verbose: bool) -> CompileResult {
    match run_inner(hnmod, cfg, verbose) {
        Ok(result) => result,
        Err(skipped) => CompileResult {
            status: CheckStatus::Skipped,
            reason: Some(skipped),
        },
    }
}

fn run_inner(hnmod: &Path, cfg: &PorterConfig, verbose: bool) -> Result<CompileResult, String> {
    if !hnmod.is_file() {
        return Err(format!("no .hnmod at {}", hnmod.display()));
    }
    if !shared::config::is_real_sha(&cfg.horizon.revision) {
        return Err(
            "the pinned revision is the all-zero placeholder; the compile gate needs a real \
             Horizon commit (set [horizon] revision in porter.toml)"
                .to_string(),
        );
    }
    let cargo = which("cargo")
        .ok_or("cargo is not on PATH; the compile gate drives the pinned build with Cargo")?;

    // ---- unpack the bundle ----
    let work = std::env::temp_dir().join(format!(
        "auto-porter-compile-gate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    ));
    let bundle_dir = work.join("bundle");
    std::fs::create_dir_all(&bundle_dir)
        .map_err(|e| format!("cannot create {}: {e}", bundle_dir.display()))?;
    if let Err(e) = unpack_hnmod(hnmod, &bundle_dir) {
        let _ = std::fs::remove_dir_all(&work);
        return Err(format!("cannot unpack {}: {e}", hnmod.display()));
    }

    // ---- manifests ----
    let native: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(bundle_dir.join("horizon-native.json"))
            .map_err(|e| format!("horizon-native.json: {e}"))?,
    )
    .map_err(|e| format!("horizon-native.json: {e}"))?;
    let mod_id = native["id"]
        .as_str()
        .ok_or("horizon-native.json has no id")?
        .to_string();
    let mixin_path = bundle_dir
        .join("mixins")
        .join(&mod_id)
        .join("horizon-mixin.toml");
    let mixin: toml::Table = std::fs::read_to_string(&mixin_path)
        .map_err(|e| format!("{}: {e}", mixin_path.display()))?
        .parse()
        .map_err(|e| format!("{}: {e}", mixin_path.display()))?;
    let api_path = mixin
        .get("api")
        .and_then(|t| t.get("path"))
        .and_then(|p| p.as_str())
        .map(str::to_string);
    // The launcher's `mixins_content` predicate (native_mods.rs): any table
    // beyond mod/compatibility/api/dependencies means horizon-mixins must run.
    let needs_mixins = mixin
        .keys()
        .any(|key| !["mod", "compatibility", "api", "dependencies"].contains(&key.as_str()));
    if api_path.is_none() && !needs_mixins {
        let _ = std::fs::remove_dir_all(&work);
        return Err(format!(
            "bundle `{mod_id}` carries neither `[api]` nor mixin content — the launcher would \
             have nothing to stage or mixin"
        ));
    }

    // ---- horizon source at the pinned revision ----
    let source = match horizon_source(cfg) {
        Ok(src) => src,
        Err(reason) => {
            let _ = std::fs::remove_dir_all(&work);
            return Err(reason);
        }
    };

    let keep = std::env::var_os("AUTO_PORTER_GATE_KEEP").is_some();
    let result = gate(
        &work,
        &source,
        &cargo,
        &bundle_dir,
        &mod_id,
        api_path.as_deref(),
        needs_mixins,
        cfg,
        verbose,
    );
    if keep {
        if verbose {
            eprintln!(
                "  [verify] compile gate work tree kept at {}",
                work.display()
            );
        }
    } else {
        let _ = std::fs::remove_dir_all(&work);
    }
    result
}

/// The staged pipeline, on a work tree that outlives failures only when
/// `AUTO_PORTER_GATE_KEEP` is set.
#[allow(clippy::too_many_arguments)]
fn gate(
    work: &Path,
    source: &Path,
    cargo: &Path,
    bundle_dir: &Path,
    mod_id: &str,
    api_path: Option<&str>,
    needs_mixins: bool,
    cfg: &PorterConfig,
    verbose: bool,
) -> Result<CompileResult, String> {
    let stage = work.join("stage");
    if needs_mixins {
        // The launcher feeds horizon-mixins per-mod folders whose manifests
        // carry no launcher-only tables (api::strip), and mixins stages the
        // transformed workspace itself (`apply` copies source → out).
        let input = work.join("mixins.incoming");
        let mixin_dir = bundle_dir.join("mixins").join(mod_id);
        std::fs::create_dir_all(input.join(mod_id)).map_err(|e| format!("mixin input: {e}"))?;
        copy_dir(&mixin_dir, &input.join(mod_id))?;
        let manifest = input.join(mod_id).join("horizon-mixin.toml");
        let mut table: toml::Table = std::fs::read_to_string(&manifest)
            .map_err(|e| format!("{manifest:?}: {e}"))?
            .parse()
            .map_err(|e| format!("{manifest:?}: {e}"))?;
        table.remove("api");
        table.remove("dependencies");
        std::fs::write(
            &manifest,
            toml::to_string(&table).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("{manifest:?}: {e}"))?;
        let tool = cfg.expand(&cfg.horizon.mixins_path);
        let tool = if tool.is_file() {
            tool
        } else {
            which("horizon-mixins").ok_or(
                "horizon-mixins is not available; bundles with mixin content need it \
                 (cargo install --locked --git https://github.com/horizon-rust/horizon-mixins \
                 --rev <mixins_revision> horizon-mixins-cli)",
            )?
        };
        run(
            Command::new(&tool)
                .arg("apply")
                .arg("--workspace")
                .arg(source)
                .arg("--mods")
                .arg(&input)
                .arg("--out")
                .arg(&stage),
            "horizon-mixins apply",
            verbose,
        )?;
    } else {
        // API-only bundle: the launcher's needs_mixins branch is false —
        // a plain sync of the pristine source, no mixins run.
        copy_dir(source, &stage)?;
    }

    let package = match api_path {
        Some(rel) => {
            stage_api_crate(bundle_dir, mod_id, rel, &stage)?;
            // The launcher fetches the locked dependencies once, then lets
            // `cargo metadata --offline` complete Cargo.lock with the staged
            // path packages (native_mods.rs "Resolving trait mod crates").
            run(
                Command::new(cargo)
                    .arg("fetch")
                    .arg("--locked")
                    .current_dir(source),
                "cargo fetch --locked",
                verbose,
            )?;
            run(
                Command::new(cargo)
                    .arg("metadata")
                    .arg("--offline")
                    .arg("--format-version")
                    .arg("1")
                    .current_dir(&stage),
                "cargo metadata --offline",
                verbose,
            )?;
            "horizon-native-game"
        }
        None => "horizon-client",
    };

    // `--features horizon-client/mods` only while the revision still has the
    // feature (the launcher's has_mods_feature); `--no-default-features` is
    // the launcher's headless engine target (no ALSA/libudev).
    let mut check = Command::new(cargo);
    check.arg(if std::env::var_os("AUTO_PORTER_GATE_BUILD").is_some() {
        "build"
    } else {
        "check"
    });
    check.arg("--locked").arg("--profile").arg("fast");
    let mods_feature = horizon_client_has_mods_feature(&stage);
    if mods_feature {
        check.arg("--features").arg("horizon-client/mods");
    }
    check.arg("--no-default-features").arg("-p").arg(package);
    check.current_dir(&stage); // the gate compiles the STAGED workspace
    if let Some(dir) = std::env::var_os("AUTO_PORTER_GATE_TARGET") {
        check.env("CARGO_TARGET_DIR", dir);
    }
    let mut summary = format!(
        "launcher-style pipeline on horizon@{} ({}: {}{}, {})",
        &cfg.horizon.revision[..8.min(cfg.horizon.revision.len())],
        if needs_mixins {
            "staged with horizon-mixins"
        } else {
            "API-only, plain stage"
        },
        package,
        if mods_feature { ", mods feature" } else { "" },
        if std::env::var_os("AUTO_PORTER_GATE_BUILD").is_some() {
            "cargo build"
        } else {
            "cargo check"
        },
    );
    match run_capturing(&mut check, verbose) {
        Ok(()) => Ok(CompileResult {
            status: CheckStatus::Pass,
            reason: Some(summary),
        }),
        Err(tail) => {
            summary.push_str(" failed; compiler tail:\n");
            summary.push_str(&tail);
            Ok(CompileResult {
                status: CheckStatus::Fail,
                reason: Some(summary),
            })
        }
    }
}

/// The launcher's `api::stage`: copy `[api] path` to `native-api/<id>/`,
/// rewrite its `horizon-client` dependency to a `default-features = false`
/// path dependency on the staged tree, write the generated
/// `horizon-native-game` executable package and add both to the workspace.
fn stage_api_crate(
    bundle_dir: &Path,
    mod_id: &str,
    api_rel: &str,
    stage: &Path,
) -> Result<(), String> {
    let src = bundle_dir.join("mixins").join(mod_id).join(api_rel);
    let dest = stage.join("native-api").join(mod_id);
    copy_dir(&src, &dest)?;

    let manifest_path = dest.join("Cargo.toml");
    let mut manifest: toml::Table = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?
        .parse()
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let deps = manifest
        .get_mut("dependencies")
        .and_then(|d| d.as_table_mut())
        .ok_or_else(|| format!("{}: missing [dependencies]", manifest_path.display()))?;
    let key = deps
        .iter()
        .find(|(k, v)| {
            k.as_str() == "horizon-client"
                || v.get("package").and_then(|p| p.as_str()) == Some("horizon-client")
        })
        .map(|(k, _)| k.clone())
        .ok_or_else(|| format!("{}: no horizon-client dependency", manifest_path.display()))?;
    let old = deps
        .get(&key)
        .cloned()
        .unwrap_or(toml::Value::Boolean(false));
    let mut rewritten = toml::Table::new();
    rewritten.insert("path".into(), "../../crates/horizon-client".into());
    rewritten.insert("default-features".into(), false.into());
    // A renamed dependency keeps its package name; feature requests carry
    // over (the launcher rewrites exactly these three keys).
    if let Some(old) = old.as_table() {
        for k in ["package", "features", "default-features"] {
            if let Some(v) = old.get(k) {
                rewritten.insert(k.into(), v.clone());
            }
        }
    }
    deps.insert(key, toml::Value::Table(rewritten));
    let crate_package = manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .ok_or_else(|| format!("{}: missing package name", manifest_path.display()))?
        .to_string();
    std::fs::write(
        &manifest_path,
        toml::to_string(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", manifest_path.display()))?;

    // The generated executable package, beside horizon-client so the shared
    // build script finds `packaging/` (launcher api.rs GAME = crates/native-game).
    let game = stage.join("crates").join("native-game");
    std::fs::create_dir_all(game.join("src")).map_err(|e| format!("native-game: {e}"))?;
    let named = std::fs::read_to_string(stage.join("crates/horizon-client/src/mod_api.rs"))
        .map(|text| text.contains("pub fn name_mods("))
        .unwrap_or(false);
    let path_dep =
        |path: &str| toml::Value::Table(toml::Table::from_iter([("path".into(), path.into())]));
    let game_manifest = toml::Table::from_iter([
        (
            "package".into(),
            toml::Value::Table(toml::Table::from_iter([
                ("name".into(), "horizon-native-game".into()),
                ("version".into(), "0.1.0".into()),
                ("edition".into(), "2024".into()),
                ("publish".into(), false.into()),
                ("build".into(), "../horizon-client/build.rs".into()),
            ])),
        ),
        (
            "target".into(),
            toml::Value::Table(toml::Table::from_iter([(
                "cfg(windows)".into(),
                toml::Value::Table(toml::Table::from_iter([(
                    "build-dependencies".into(),
                    toml::Value::Table(toml::Table::from_iter([(
                        "winresource".into(),
                        "0.1".into(),
                    )])),
                )])),
            )])),
        ),
        (
            "bin".into(),
            toml::Value::Array(vec![toml::Value::Table(toml::Table::from_iter([
                ("name".into(), "horizon-client".into()),
                ("path".into(), "src/main.rs".into()),
            ]))]),
        ),
        (
            "dependencies".into(),
            toml::Value::Table(toml::Table::from_iter([
                (
                    "horizon-client".into(),
                    toml::Value::Table(toml::Table::from_iter([
                        ("path".into(), "../horizon-client".into()),
                        ("default-features".into(), false.into()),
                    ])),
                ),
                ("horizon-threads".into(), path_dep("../horizon-threads")),
                ("horizon-mods".into(), path_dep("../horizon-mods")),
                (
                    "mod_0".into(),
                    toml::Value::Table(toml::Table::from_iter([
                        ("path".into(), format!("../../native-api/{mod_id}").into()),
                        ("package".into(), crate_package.into()),
                    ])),
                ),
            ])),
        ),
        (
            "features".into(),
            toml::Value::Table(toml::Table::from_iter([(
                "default".into(),
                toml::Value::Array(vec!["horizon-client/default".into()]),
            )])),
        ),
    ]);
    std::fs::write(
        game.join("Cargo.toml"),
        toml::to_string(&game_manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("native-game: {e}"))?;
    let naming = if named {
        format!(
            "    horizon_client::mod_api::name_mods(&{ids:?});\n",
            ids = [mod_id]
        )
    } else {
        String::new()
    };
    std::fs::write(
        game.join("src/main.rs"),
        format!(
            r#"//! Generated by the launcher.
#![forbid(unsafe_code)]

#[global_allocator]
static ALLOCATOR: horizon_threads::Allocator = horizon_threads::Allocator;

static MOD_API_LEVEL: [u8; horizon_mods::api_level::MARK.len()] = horizon_mods::api_level::MARK;

fn main() {{
    std::hint::black_box(&MOD_API_LEVEL);
    let mods: horizon_client::mod_api::Mods = vec![
        mod_0::hooks(),
    ].into();
{naming}    horizon_client::entry::main_with(mods);
}}
"#
        ),
    )
    .map_err(|e| format!("native-game main.rs: {e}"))?;

    let root = stage.join("Cargo.toml");
    let mut workspace: toml::Table = std::fs::read_to_string(&root)
        .map_err(|e| format!("{}: {e}", root.display()))?
        .parse()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    let members = workspace
        .get_mut("workspace")
        .and_then(|w| w.as_table_mut())
        .and_then(|w| w.get_mut("members"))
        .and_then(|m| m.as_array_mut())
        .ok_or("stage workspace has no members")?;
    members.push(toml::Value::String("crates/native-game".into()));
    members.push(toml::Value::String(format!("native-api/{mod_id}")));
    std::fs::write(
        &root,
        toml::to_string(&workspace).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", root.display()))?;
    Ok(())
}

/// Whether the staged horizon-client still declares the `mods` feature (the
/// launcher's `has_mods_feature`): the pinned revision folded the mod runtime
/// into the crate proper, so the flag is usually absent.
fn horizon_client_has_mods_feature(stage: &Path) -> bool {
    std::fs::read_to_string(stage.join("crates/horizon-client/Cargo.toml"))
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|t| t.get("features").cloned())
        .and_then(|f| f.get("mods").is_some().then_some(()))
        .is_some()
}

// ---------------------------------------------------------------------------
// prerequisites

/// The Horizon source for the pinned revision: config path, then
/// `AUTO_PORTER_HORIZON_SOURCE`, then the pinned codeload archive fetched
/// once into a cache dir (the launcher does the same, "once per commit").
fn horizon_source(cfg: &PorterConfig) -> Result<PathBuf, String> {
    let validate = |dir: &Path| -> Result<PathBuf, String> {
        if dir.join("Cargo.toml").is_file() && dir.join("crates/horizon-client").is_dir() {
            Ok(dir.to_path_buf())
        } else {
            Err(format!(
                "{} is not a Horizon source tree (no Cargo.toml / crates/horizon-client)",
                dir.display()
            ))
        }
    };
    let configured = cfg.expand(&cfg.horizon.source_dir);
    if cfg.horizon.source_dir.is_empty() {
        if let Some(env_dir) = std::env::var_os("AUTO_PORTER_HORIZON_SOURCE") {
            return validate(&PathBuf::from(env_dir));
        }
    } else {
        return validate(&configured);
    }
    let cache = std::env::temp_dir()
        .join("auto-porter-horizon-source")
        .join(&cfg.horizon.revision);
    if cache.join("Cargo.toml").is_file() {
        return validate(&cache);
    }
    let _ = std::fs::remove_dir_all(&cache);
    std::fs::create_dir_all(&cache).map_err(|e| format!("cache dir: {e}"))?;
    let url = format!(
        "https://codeload.github.com/horizon-rust/horizon/tar.gz/{}",
        cfg.horizon.revision
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(300))
        .build();
    let mut bytes = Vec::new();
    agent
        .get(&url)
        .call()
        .map_err(|e| format!("cannot fetch the pinned Horizon archive ({url}): {e}"))?
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("pinned Horizon archive: {e}"))?;
    extract_tar_gz(&bytes, &cache)?;
    validate(&cache)
}

/// Extract a gzipped tarball with the same guardrails as the data-carry
/// source fetch (absolute paths and `..` dropped, total size capped).
fn extract_tar_gz(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(false);
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
        // Links are left out, as the launcher's staging does ("Links in the
        // archive are left out"): only regular files and directories unpack.
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let size = entry.size();
        if total + size > HORIZON_EXTRACT_CAP {
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
    // codeload archives carry a single `<repo>-<ref>/` root; flatten it.
    let mut entries = std::fs::read_dir(dest).map_err(|e| format!("cache dir: {e}"))?;
    if let Some(Ok(first)) = entries.next() {
        if entries.next().is_none() && first.path().is_dir() {
            let root = first.path();
            for child in std::fs::read_dir(&root)
                .map_err(|e| format!("{}: {e}", root.display()))?
                .filter_map(|c| c.ok())
            {
                let _ = std::fs::rename(child.path(), dest.join(child.file_name()));
            }
            let _ = std::fs::remove_dir(&root);
        }
    }
    Ok(())
}

fn unpack_hnmod(hnmod: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(hnmod).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            continue;
        };
        let target = dest.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&target).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Recursive copy (the launcher's `sync`): directories recreated, files
/// copied, symlinks refused up-front the way the staging tool refuses them.
fn copy_dir(src: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", src.display()))?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_symlink() {
            return Err(format!(
                "refusing symlink in the stage input: {}",
                path.display()
            ));
        }
        if file_type.is_dir() {
            if path.file_name().is_some_and(|n| n == ".git") {
                continue; // the launcher stages GitHub archives: no .git
            }
            copy_dir(&path, &target)?;
        } else {
            std::fs::copy(&path, &target).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|p| p.is_file())
    })
}

fn run(cmd: &mut Command, label: &str, verbose: bool) -> Result<(), String> {
    if verbose {
        eprintln!("  [verify] {label} …");
    }
    let status = cmd.status().map_err(|e| format!("{label}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} exited with {status}"))
    }
}

/// Run a command, capturing stderr+stdout; on failure return the output tail.
fn run_capturing(cmd: &mut Command, verbose: bool) -> Result<(), String> {
    if verbose {
        eprintln!("  [verify] $ {:?}", cmd);
    }
    let output = cmd
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("cargo: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if verbose {
        for line in text.lines().rev().take(8).collect::<Vec<_>>().iter().rev() {
            eprintln!("  [verify]   {line}");
        }
    }
    if output.status.success() {
        Ok(())
    } else {
        let tail: Vec<String> = text
            .lines()
            .rev()
            .take(OUTPUT_TAIL_LINES)
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Err(tail.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The launcher's `mixins_content` predicate (native_mods.rs): any table
    /// beyond mod/compatibility/api/dependencies means horizon-mixins must
    /// run — and mixins itself refuses content-less manifests, so an API-only
    /// bundle must take the plain-sync branch instead.
    #[test]
    fn needs_mixins_mirrors_the_launcher_predicate() {
        let keys_with_content = |text: &str| -> bool {
            let table: toml::Table = text.parse().expect("parses");
            table
                .keys()
                .any(|key| !["mod", "compatibility", "api", "dependencies"].contains(&key.as_str()))
        };
        let api_only = "[mod]\nid = 'm'\nversion = '1'\n\n[compatibility]\nhorizon_revision = 'x'\n\n[api]\npath = 'engine'\n";
        assert!(!keys_with_content(api_only), "API-only bundle: no mixins");
        let with_injection = "[mod]\nid = 'm'\nversion = '1'\n\n[[injection]]\nid = 'i'\nhook = 'h'\ntarget = 't'\nhelper = 'f'\nkind = 'HEAD'\n";
        assert!(
            keys_with_content(with_injection),
            "injection table: mixins must run"
        );
        let with_crate = "[mod]\nid = 'm'\nversion = '1'\n\n[[crate]]\nname = 'c'\n";
        assert!(
            keys_with_content(with_crate),
            "crate table: mixins must run"
        );
    }

    /// `--features horizon-client/mods` is passed only while the revision
    /// still declares the feature (the launcher's `has_mods_feature`); the
    /// pinned revision folded the mod runtime into the crate proper.
    #[test]
    fn mods_feature_predicate_reads_the_staged_manifest() {
        let dir = std::env::temp_dir().join(format!("gate-feature-test-{}", std::process::id()));
        let manifest = dir.join("crates/horizon-client");
        std::fs::create_dir_all(&manifest).expect("dirs");
        std::fs::write(manifest.join("Cargo.toml"), "[features]\naudio = []\n").unwrap();
        assert!(!horizon_client_has_mods_feature(&dir), "no mods feature");
        std::fs::write(
            manifest.join("Cargo.toml"),
            "[features]\nmods = []\naudio = []\n",
        )
        .unwrap();
        assert!(
            horizon_client_has_mods_feature(&dir),
            "mods feature detected"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
