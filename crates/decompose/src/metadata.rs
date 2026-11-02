//! Loader metadata parsing: `fabric.mod.json`, `META-INF/neoforge.mods.toml`,
//! legacy `META-INF/mods.toml`, plus MANIFEST.MF fallbacks.

use shared::types::{Dependency, Loader, ModMetadata};
use std::collections::BTreeMap;

pub struct RawJar<'a> {
    /// All file entries of the jar, sorted by path.
    pub entries: BTreeMap<String, Vec<u8>>,
    pub _marker: std::marker::PhantomData<&'a ()>,
}

impl RawJar<'_> {
    pub fn text(&self, path: &str) -> Option<String> {
        self.entries
            .get(path)
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }
}

fn sanitize_id(stem: &str) -> String {
    let s: String = stem
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if s.is_empty() {
        "unknown_mod".to_string()
    } else {
        s
    }
}

fn parse_manifest(jar: &RawJar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(text) = jar.text("META-INF/MANIFEST.MF") {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once(':') {
                out.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    out
}

pub fn detect_loader(jar: &RawJar) -> Loader {
    if jar.entries.contains_key("fabric.mod.json") {
        Loader::Fabric
    } else if jar.entries.contains_key("META-INF/neoforge.mods.toml") {
        Loader::NeoForge
    } else if jar.entries.contains_key("META-INF/mods.toml")
        || jar.entries.contains_key("META-INF/MANIFEST.MF")
            && jar
                .text("META-INF/MANIFEST.MF")
                .map(|m| m.contains("Forge"))
                .unwrap_or(false)
    {
        Loader::Forge
    } else {
        Loader::Unknown
    }
}

pub fn parse_metadata(
    jar: &RawJar,
    jar_stem: &str,
    loader: Loader,
) -> (ModMetadata, Vec<Dependency>) {
    let manifest = parse_manifest(jar);
    match loader {
        Loader::Fabric => parse_fabric(jar, jar_stem),
        Loader::NeoForge => parse_mods_toml(jar, "META-INF/neoforge.mods.toml", jar_stem, loader),
        Loader::Forge => parse_mods_toml(jar, "META-INF/mods.toml", jar_stem, loader),
        Loader::Unknown => {
            let id = manifest
                .get("Specification-Title")
                .map(|s| sanitize_id(s))
                .unwrap_or_else(|| sanitize_id(jar_stem));
            let version = manifest
                .get("Implementation-Version")
                .cloned()
                .unwrap_or_else(|| "0.0.0".to_string());
            (
                ModMetadata {
                    id,
                    name: manifest
                        .get("Specification-Title")
                        .cloned()
                        .unwrap_or_else(|| jar_stem.to_string()),
                    version,
                    loader,
                    minecraft_range: None,
                    entrypoints: Vec::new(),
                    mixin_configs: manifest
                        .get("MixinConfigs")
                        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
                        .unwrap_or_default(),
                    description: None,
                    authors: Vec::new(),
                },
                Vec::new(),
            )
        }
    }
}

fn parse_fabric(jar: &RawJar, jar_stem: &str) -> (ModMetadata, Vec<Dependency>) {
    let mut deps = Vec::new();
    let empty = serde_json::Value::Null;
    let v = jar
        .text("fabric.mod.json")
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .unwrap_or(empty);

    let id = v
        .get("id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| sanitize_id(jar_stem));
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or(&id)
        .to_string();
    let version = v
        .get("version")
        .and_then(|x| x.as_str())
        .unwrap_or("0.0.0")
        .to_string();

    let mut entrypoints = Vec::new();
    if let Some(eps) = v.get("entrypoints").and_then(|e| e.get("main")) {
        if let Some(arr) = eps.as_array() {
            for e in arr {
                if let Some(s) = e.as_str() {
                    entrypoints.push(s.to_string());
                } else if let Some(s) = e.get("value").and_then(|x| x.as_str()) {
                    entrypoints.push(s.to_string());
                }
            }
        }
    }

    let mixin_configs = v
        .get("mixins")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    for (table, kind) in [("depends", "required"), ("suggests", "optional")] {
        if let Some(map) = v.get(table).and_then(|m| m.as_object()) {
            for (dep_id, ver) in map {
                deps.push(Dependency {
                    id: dep_id.clone(),
                    version_req: ver.as_str().map(|s| s.to_string()),
                    kind: kind.to_string(),
                });
            }
        }
    }

    let authors = v
        .get("authors")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| {
                    x.as_str().map(|s| s.to_string()).or_else(|| {
                        x.get("name")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string())
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    (
        ModMetadata {
            id,
            name,
            version,
            loader: Loader::Fabric,
            minecraft_range: deps
                .iter()
                .find(|d| d.id == "minecraft")
                .and_then(|d| d.version_req.clone()),
            entrypoints,
            mixin_configs,
            description: v
                .get("description")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
            authors,
        },
        deps,
    )
}

fn parse_mods_toml(
    jar: &RawJar,
    path: &str,
    jar_stem: &str,
    loader: Loader,
) -> (ModMetadata, Vec<Dependency>) {
    let mut deps = Vec::new();
    let text = jar.text(path).unwrap_or_default();
    let parsed: toml::Value =
        toml::from_str(&text).unwrap_or(toml::Value::Table(Default::default()));

    let mods_arr = parsed
        .get("mods")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    let first = mods_arr
        .first()
        .cloned()
        .unwrap_or(toml::Value::Table(Default::default()));

    let id = first
        .get("modId")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| sanitize_id(jar_stem));
    let version = first
        .get("version")
        .and_then(|x| x.as_str())
        .unwrap_or("0.0.0")
        .to_string();
    let display_name = first
        .get("displayName")
        .and_then(|x| x.as_str())
        .unwrap_or(&id)
        .to_string();

    let mut authors: Vec<String> = Vec::new();
    match first.get("authors") {
        Some(toml::Value::String(s)) => {
            authors = s.split(',').map(|a| a.trim().to_string()).collect();
        }
        Some(toml::Value::Array(arr)) => {
            authors = arr
                .iter()
                .filter_map(|a| a.as_str().map(|s| s.to_string()))
                .collect();
        }
        _ => {}
    }

    if let Some(dep_tables) = parsed
        .get(format!("dependencies.{id}"))
        .and_then(|d| d.as_array())
    {
        for t in dep_tables {
            let dep_id = t.get("modId").and_then(|x| x.as_str()).unwrap_or("");
            let version_range = t.get("versionRange").and_then(|x| x.as_str());
            let dep_type = t
                .get("type")
                .and_then(|x| x.as_str())
                .unwrap_or("required")
                .to_string();
            if !dep_id.is_empty() {
                deps.push(Dependency {
                    id: dep_id.to_string(),
                    version_req: version_range.map(|s| s.to_string()),
                    kind: if dep_type == "optional" {
                        "optional".to_string()
                    } else {
                        "required".to_string()
                    },
                });
            }
        }
    }

    let mixin_configs = manifest_mixin_configs(jar);

    (
        ModMetadata {
            id,
            name: display_name,
            version,
            loader,
            minecraft_range: deps
                .iter()
                .find(|d| d.id == "minecraft")
                .and_then(|d| d.version_req.clone()),
            entrypoints: Vec::new(),
            mixin_configs,
            description: first
                .get("description")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
            authors,
        },
        deps,
    )
}

fn manifest_mixin_configs(jar: &RawJar) -> Vec<String> {
    let manifest = parse_manifest(jar);
    manifest
        .get("MixinConfigs")
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default()
}
