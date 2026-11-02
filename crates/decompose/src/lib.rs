//! Phase 1: Decompose — JAR → typed AST (spec §2).

pub mod classfile;
pub mod events;
pub mod metadata;
pub mod mixins;

use shared::error::{ErrorType, Phase, PortError};
use shared::types::*;
use std::collections::BTreeMap;
use std::path::Path;

pub struct DecomposeOutput {
    pub mod_ir: DecomposedMod,
    /// Gate violations that map 1:1 to PortErrors (spec §2 quality gates, §9).
    pub gate_errors: Vec<PortError>,
}

/// Unpack the jar and produce the `DecomposedMod` typed artifact.
pub fn decompose(jar_path: &Path) -> Result<DecomposeOutput, String> {
    let file = std::fs::File::open(jar_path)
        .map_err(|e| format!("cannot open {}: {e}", jar_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("{} is not a readable jar: {e}", jar_path.display()))?;

    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut jar_entries = 0u32;
    for i in 0..archive.len() {
        let mut zf = archive
            .by_index(i)
            .map_err(|e| format!("bad jar entry #{i}: {e}"))?;
        if zf.is_dir() {
            continue;
        }
        let name = zf.name().to_string();
        // normalise a leading ./ some jars carry
        let name = name.strip_prefix("./").unwrap_or(&name).to_string();
        if name.ends_with('/') {
            continue;
        }
        jar_entries += 1;
        let mut buf = Vec::with_capacity(zf.size() as usize);
        std::io::Read::read_to_end(&mut zf, &mut buf)
            .map_err(|e| format!("cannot read jar entry {name}: {e}"))?;
        entries.insert(name, buf);
    }
    let jar = metadata::RawJar {
        entries,
        _marker: std::marker::PhantomData,
    };

    let jar_stem = jar_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mod".to_string());

    let loader = metadata::detect_loader(&jar);
    let (metadata, dependencies) = metadata::parse_metadata(&jar, &jar_stem, loader);

    // ---- classes
    let mut classes = Vec::new();
    let mut class_entries = 0u32;
    let mut parse_failures: Vec<String> = Vec::new();
    for (path, bytes) in &jar.entries {
        if !path.ends_with(".class") {
            continue;
        }
        class_entries += 1;
        match classfile::parse(bytes) {
            Ok(raw) => classes.push(raw),
            Err(e) => parse_failures.push(format!("{path}: {e}")),
        }
    }
    classes.sort_by(|a, b| a.this_class.cmp(&b.this_class));

    // ---- mixins & events
    let mixins = mixins::extract_mixins(&classes);
    let events = events::extract_events(&classes, loader);

    // ---- data inventory
    let mut data_files = Vec::new();
    let mut config: Option<ConfigSchema> = None;
    for (path, bytes) in &jar.entries {
        let kind = if path.starts_with("assets/") {
            Some("asset")
        } else if path.starts_with("data/") {
            Some("data")
        } else if path == "pack.mcmeta" {
            Some("pack_meta")
        } else {
            None
        };
        if let Some(kind) = kind {
            data_files.push(DataFile {
                path: path.clone(),
                sha256: shared::util::sha256_hex(bytes),
                size: bytes.len() as u64,
                kind: kind.to_string(),
            });
        }
        if config.is_none() && !path.contains('/') && (path.ends_with(".toml")) {
            config = Some(ConfigSchema {
                path: path.clone(),
                format: "toml".to_string(),
            });
        }
    }
    if config.is_none() {
        if let Some((path, _)) = jar.entries.iter().find(|(p, _)| {
            !p.contains('/') && p.ends_with(".json") && p.as_str() != "fabric.mod.json"
        }) {
            config = Some(ConfigSchema {
                path: path.clone(),
                format: "json".to_string(),
            });
        }
    }

    // ---- quality gates (spec §2)
    let mut gate_violations: Vec<GateViolation> = Vec::new();
    if class_entries != classes.len() as u32 {
        gate_violations.push(GateViolation {
            gate: "class_ast_coverage".to_string(),
            message: format!(
                "{}/{} class entries parsed; failures: {}",
                classes.len(),
                class_entries,
                if parse_failures.is_empty() {
                    "none".to_string()
                } else {
                    parse_failures.join("; ")
                }
            ),
        });
    }

    let class_names: std::collections::HashSet<&str> =
        classes.iter().map(|c| c.this_class.as_str()).collect();
    const KNOWN_PREFIXES: [&str; 6] = [
        "net/minecraft",
        "com/mojang",
        "net/neoforged",
        "net/minecraftforge",
        "net/fabricmc",
        "java/",
    ];
    let mut gate_errors = Vec::new();
    for m in &mixins {
        if let Some(target) = &m.target_class {
            let known = class_names.contains(target.as_str())
                || KNOWN_PREFIXES.iter().any(|p| target.starts_with(p));
            if !known {
                gate_violations.push(GateViolation {
                    gate: "mixin_targets_resolvable".to_string(),
                    message: format!(
                        "Mixin target `{}` referenced by {} does not resolve to a known class",
                        target, m.class
                    ),
                });
                gate_errors.push(
                    PortError::new(Phase::Decompose, ErrorType::UnknownMixinTarget, format!(
                        "Mixin target `{target}` referenced by {} does not resolve to a known class",
                        m.class
                    ))
                    .with_source(m.class.clone())
                    .with_suggestion(
                        "verify the target class name against the mod's Minecraft version",
                    ),
                );
            }
        } else {
            gate_violations.push(GateViolation {
                gate: "mixin_targets_resolvable".to_string(),
                message: format!(
                    "Mixin class {} has no target (@Mixin value/targets)",
                    m.class
                ),
            });
        }
    }
    let data_in_jar = data_files.len() as u32;

    let stats = DecomposeStats {
        jar_entries,
        class_entries,
        parsed_classes: classes.len() as u32,
        data_files_in_jar: data_in_jar,
        mixin_injections: mixins.iter().map(|m| m.injections.len() as u32).sum(),
        event_listeners: events.len() as u32,
        gate_violations,
    };

    let classes_ast: Vec<ClassAst> = classes
        .iter()
        .map(|c| ClassAst {
            name: c.this_class.clone(),
            super_name: c.super_class.clone(),
            interfaces: c.interfaces.clone(),
            access: c.access,
            fields: c
                .fields
                .iter()
                .map(|f| FieldAst {
                    name: f.name.clone(),
                    descriptor: f.descriptor.clone(),
                    access: f.access,
                })
                .collect(),
            methods: c
                .methods
                .iter()
                .map(|m| MethodAst {
                    name: m.name.clone(),
                    descriptor: m.descriptor.clone(),
                    access: m.access,
                    has_code: m.has_code,
                })
                .collect(),
        })
        .collect();

    let mod_ir = DecomposedMod {
        metadata,
        classes: classes_ast,
        mixins,
        events,
        data_files,
        config,
        dependencies,
        stats,
    };

    Ok(DecomposeOutput {
        mod_ir,
        gate_errors,
    })
}
