//! `.hmod` (content mod) validation against the mod-files rules (spec §0.6,
//! §4.2): allowed top-level entries, manifest fields, JSON parse, asset
//! reference existence.

use crate::{parse_json_checks, read_zip, top_level_entries, Verdict};
use shared::catalog::HookCatalog;
use std::path::Path;

pub fn validate(path: &Path) -> Verdict {
    let mut v = Verdict::new();
    let entries = match read_zip(path) {
        Ok(e) => e,
        Err(e) => {
            v.fail("zip_read", e);
            return v;
        }
    };

    // ---- top-level entry whitelist: horizon-mod.toml + assets/ + data/,
    //      optionally both manifests under one id (spec §0.6).
    let allowed = ["horizon-mod.toml", "assets", "data", "horizon-native.json"];
    for top in top_level_entries(&entries) {
        if !allowed.contains(&top.as_str()) {
            v.fail(
                "top_level_entry",
                format!("top-level entry `{top}` is refused by docs/api/mod-files.md"),
            );
        }
    }
    if !entries.contains_key("horizon-mod.toml") {
        v.fail(
            "manifest_missing",
            "horizon-mod.toml is required in a .hmod",
        );
    } else {
        validate_hmod_manifest(&entries, &mut v);
    }
    if entries.contains_key("horizon-native.json") {
        // one .hmod may hold both manifests under the same id
        crate::hnmod::validate_native_manifest(&entries, None, None, &mut v);
    }

    parse_json_checks(&entries, &["assets/", "data/"], &mut v);
    check_asset_references(&entries, &mut v);

    v
}

fn validate_hmod_manifest(entries: &std::collections::BTreeMap<String, Vec<u8>>, v: &mut Verdict) {
    let text = String::from_utf8_lossy(entries.get("horizon-mod.toml").unwrap());
    let parsed: toml::Value = match toml::from_str(&text) {
        Ok(t) => t,
        Err(e) => {
            v.fail("manifest_parse", format!("horizon-mod.toml: {e}"));
            return;
        }
    };
    let get = |k: &str| {
        parsed
            .get(k)
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
    };
    match parsed.get("schema").and_then(|x| x.as_integer()) {
        Some(1) => {}
        Some(n) => v.fail(
            "manifest_schema",
            format!("horizon-mod.toml: schema {n} != 1"),
        ),
        None => {
            // The real horizon-port-pack (api 0.2) omits `schema` entirely —
            // the authoritative generator is the format, not our seed guess.
            v.warn(
                "manifest_schema",
                "horizon-mod.toml has no `schema` key (horizon-port-pack 0.2 format); treating as 1",
            );
        }
    }
    match get("id") {
        Some(id) => {
            if !shared::util::is_valid_mod_id(&id) {
                v.fail(
                    "manifest_id",
                    format!("horizon-mod.toml: invalid mod id `{id}`"),
                );
            }
        }
        None => v.fail("manifest_id", "horizon-mod.toml: `id` missing"),
    }
    match get("version") {
        Some(ver) if ver.is_empty() => {
            v.fail("manifest_version", "horizon-mod.toml: `version` empty")
        }
        Some(_) => {}
        None => v.fail("manifest_version", "horizon-mod.toml: `version` missing"),
    }
}

/// Cross-reference checks: every texture/model/sound referenced by an asset in
/// the mod's own namespace must exist inside the bundle.
fn check_asset_references(entries: &std::collections::BTreeMap<String, Vec<u8>>, v: &mut Verdict) {
    // discover the mod namespace(s) present under assets/
    let mut namespaces: std::collections::BTreeSet<String> = Default::default();
    for name in entries.keys() {
        if let Some(rest) = name.strip_prefix("assets/") {
            if let Some(ns) = rest.split('/').next() {
                namespaces.insert(ns.to_string());
            }
        }
    }

    for (name, bytes) in entries {
        if !name.starts_with("assets/") || !name.ends_with(".json") {
            continue;
        }
        let Ok(json) = serde_json::from_slice::<serde_json::Value>(bytes) else {
            continue; // JSON parse reported elsewhere
        };
        collect_texture_refs(&json, &mut |reference| {
            // Vanilla model semantics: an un-namespaced reference resolves to
            // `minecraft:` — it is never relative to the mod's own namespace.
            let (ns, rel) = match reference.split_once(':') {
                Some((ns, rel)) => (ns.to_string(), rel.to_string()),
                None => ("minecraft".to_string(), reference.to_string()),
            };
            if !namespaces.contains(&ns) {
                return; // vanilla or another namespace: not our responsibility
            }
            let candidate = format!("assets/{ns}/textures/{rel}.png");
            if !entries.contains_key(&candidate) {
                if ns == "minecraft" {
                    // Vanilla-namespace refs resolve against the game's own
                    // assets at runtime; a mod carries `minecraft:` textures
                    // only to *replace* vanilla ones, so not carrying one is
                    // expected (e.g. a model citing `block/cobweb`).
                    v.warn(
                        "vanilla_texture_not_carried",
                        format!(
                            "{name}: texture `{reference}` is not carried ({candidate}); resolves against vanilla assets"
                        ),
                    );
                } else {
                    v.fail(
                        "missing_texture",
                        format!(
                            "{name}: referenced texture `{reference}` is missing ({candidate})"
                        ),
                    );
                }
            }
        });
        collect_sound_refs(&json, &mut |reference| {
            let (ns, rel) = match reference.split_once(':') {
                Some((ns, rel)) => (ns.to_string(), rel.to_string()),
                None => return, // relative sound refs are ambiguous; skip
            };
            if !namespaces.contains(&ns) {
                return;
            }
            let candidate = format!("assets/{ns}/sounds/{rel}.ogg");
            if !entries.contains_key(&candidate) {
                v.fail(
                    "missing_sound",
                    format!("{name}: referenced sound `{reference}` is missing ({candidate})"),
                );
            }
        });
    }
}

fn collect_texture_refs(json: &serde_json::Value, f: &mut dyn FnMut(&str)) {
    match json {
        serde_json::Value::Object(map) => {
            for (k, val) in map {
                if k == "textures" {
                    if let Some(t) = val.as_object() {
                        for tv in t.values() {
                            if let Some(s) = tv.as_str() {
                                // `#name` values reference a texture variable
                                // defined by the parent model chain (vanilla
                                // model semantics) — not a file path, so the
                                // cross-check does not apply.
                                if !s.starts_with('#') {
                                    f(s);
                                }
                            }
                        }
                    }
                } else if k == "texture" {
                    if let Some(s) = val.as_str() {
                        if !s.starts_with('#') {
                            f(s);
                        }
                    }
                } else {
                    collect_texture_refs(val, f);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_texture_refs(item, f);
            }
        }
        _ => {}
    }
}

fn collect_sound_refs(json: &serde_json::Value, f: &mut dyn FnMut(&str)) {
    if let Some(sounds) = json.get("sounds").and_then(|s| s.as_array()) {
        for s in sounds {
            if let Some(name) = s.get("name").and_then(|n| n.as_str()) {
                f(name);
            }
        }
    }
}

/// Validate a `horizon-native.json` manifest in place (shared by .hmod with
/// both manifests and .hnmod). `catalog`/`expected_api` come from the pinned
/// revision when available.
pub fn validate_native_manifest(
    entries: &std::collections::BTreeMap<String, Vec<u8>>,
    catalog: Option<&HookCatalog>,
    expected_api: Option<u32>,
    v: &mut Verdict,
) {
    let Some(bytes) = entries.get("horizon-native.json") else {
        v.fail("manifest_missing", "horizon-native.json is required");
        return;
    };
    let json: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(j) => j,
        Err(e) => {
            v.fail("manifest_parse", format!("horizon-native.json: {e}"));
            return;
        }
    };
    if json.get("schema").and_then(|x| x.as_i64()) != Some(1) {
        v.fail("manifest_schema", "horizon-native.json: schema must be 1");
    }
    match json.get("id").and_then(|x| x.as_str()) {
        Some(id) if shared::util::is_valid_mod_id(id) => {}
        Some(id) => v.fail(
            "manifest_id",
            format!("horizon-native.json: invalid id `{id}`"),
        ),
        None => v.fail("manifest_id", "horizon-native.json: `id` missing"),
    }
    if json
        .get("version")
        .and_then(|x| x.as_str())
        .map(|s| s.is_empty())
        .unwrap_or(true)
    {
        v.fail(
            "manifest_version",
            "horizon-native.json: `version` missing/empty",
        );
    }
    if json.get("game").and_then(|x| x.as_str()).is_none() {
        v.fail(
            "manifest_game",
            "horizon-native.json: `game` missing (e.g. \"26.3\")",
        );
    }
    for field in ["horizon", "mixins"] {
        match json.get(field).and_then(|x| x.as_str()) {
            Some(sha) if shared::config::is_40_hex(sha) => {
                if !shared::config::is_real_sha(sha) {
                    v.warn(
                        "manifest_placeholder_sha",
                        format!("horizon-native.json: `{field}` is the zero placeholder, not a pinned commit"),
                    );
                }
            }
            Some(sha) => v.fail(
                "manifest_sha",
                format!("horizon-native.json: `{field}` must be a full 40-hex commit, got `{sha}`"),
            ),
            None => v.fail(
                "manifest_sha",
                format!("horizon-native.json: `{field}` missing"),
            ),
        }
    }
    if let Some(hooks) = json.get("hooks") {
        match hooks.as_u64() {
            Some(n) => {
                if let (Some(catalog), Some(expected)) = (catalog, expected_api) {
                    if n != expected as u64 {
                        v.fail(
                            "api_version_mismatch",
                            format!(
                                "horizon-native.json: hooks {n} != NATIVE_HOOKS_API {expected} at the pinned revision (catalog: {})",
                                catalog.native_hooks_api
                            ),
                        );
                    }
                }
            }
            None => v.fail(
                "manifest_hooks",
                "horizon-native.json: `hooks` must be an integer",
            ),
        }
    }
}
