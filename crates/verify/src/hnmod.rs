//! `.hnmod` (native mod bundle) validation: exactly `horizon-native.json` +
//! `mixins/<id>/…` at the top level, `horizon-mixin.toml` present, injections
//! resolve against the pinned hook catalog, trait-style (`[api]`) bundles
//! carry a loadable engine crate (`hooks()` export, horizon-client dep), and
//! sources pass the API lint.

use crate::{parse_json_checks, read_zip, top_level_entries, Verdict};
use shared::catalog::HookCatalog;
use std::path::Path;

pub fn validate_native_manifest(
    entries: &std::collections::BTreeMap<String, Vec<u8>>,
    catalog: Option<&HookCatalog>,
    expected_api: Option<u32>,
    v: &mut Verdict,
) {
    crate::hmod::validate_native_manifest(entries, catalog, expected_api, v)
}

pub fn validate(path: &Path, catalog: Option<&HookCatalog>, expected_api: Option<u32>) -> Verdict {
    let mut v = Verdict::new();
    let entries = match read_zip(path) {
        Ok(e) => e,
        Err(e) => {
            v.fail("zip_read", e);
            return v;
        }
    };

    // ---- top level: nothing besides the manifest and mixins/ (spec §0.6)
    for top in top_level_entries(&entries) {
        if top != "horizon-native.json" && top != "mixins" {
            v.fail(
                "top_level_entry",
                format!("top-level entry `{top}` is refused in a .hnmod (only horizon-native.json and mixins/)"),
            );
        }
    }

    crate::hmod::validate_native_manifest(&entries, catalog, expected_api, &mut v);

    parse_json_checks(&entries, &["mixins/"], &mut v);

    // ---- mixins/<id>/horizon-mixin.toml
    let mixin_prefix = entries
        .keys()
        .find_map(|k| {
            k.strip_prefix("mixins/")
                .map(|rest| rest.split('/').next().map(|id| format!("mixins/{id}/")))
        })
        .flatten();
    let Some(prefix) = mixin_prefix else {
        v.fail("bundle_layout", "no mixins/<id>/ directory found");
        return v;
    };
    let toml_path = format!("{prefix}horizon-mixin.toml");
    let mut injections: Vec<(String, i64)> = Vec::new();
    if !entries.contains_key(&toml_path) {
        v.fail("mixin_toml_missing", format!("{toml_path} is required"));
    } else {
        let text = String::from_utf8_lossy(entries.get(&toml_path).unwrap());
        match toml::from_str::<toml::Value>(&text) {
            Ok(parsed) => {
                // The launcher's `bundle()` requires [mod] id/version and
                // applies its `api::valid_id` rule; refuse here what it would
                // refuse at load time.
                let mod_table = parsed.get("mod");
                match mod_table.and_then(|m| m.as_table()) {
                    Some(mod_table) => {
                        for key in ["id", "version"] {
                            let value = mod_table.get(key).and_then(|v| v.as_str());
                            match value {
                                Some(s) if !s.is_empty() => {}
                                _ => v.fail(
                                    "mod_field",
                                    format!("{toml_path}: missing [mod] {key} (the launcher requires it)"),
                                ),
                            }
                        }
                        if let Some(id) = mod_table.get("id").and_then(|v| v.as_str()) {
                            if !valid_bundle_id(id) {
                                v.fail(
                                    "bundle_id",
                                    format!("{toml_path}: invalid mod id `{id}` (alphanumeric, `_`, `-`; at most 100 chars)"),
                                );
                            }
                        }
                    }
                    _ => v.fail(
                        "mod_table",
                        format!("{toml_path}: missing [mod] table (id, version are required)"),
                    ),
                }
                // Trait-style (`[api]`) bundles get bindability checks
                // mirroring the launcher's `api::folder` + `api::stage`.
                let api_path = parsed
                    .get("api")
                    .and_then(|a| a.get("path"))
                    .and_then(|p| p.as_str());
                if let Some(api_path) = api_path {
                    validate_api_bundle(&prefix, api_path, &entries, &mut v);
                }
                if let Some(list) = parsed.get("injection").and_then(|x| x.as_array()) {
                    for inj in list {
                        let hook = inj.get("hook").and_then(|h| h.as_str()).unwrap_or("");
                        let version = inj
                            .get("hook_version")
                            .and_then(|h| h.as_integer())
                            .unwrap_or(0);
                        if hook.is_empty() {
                            v.fail(
                                "injection_hook",
                                format!("{toml_path}: [[injection]] without `hook`"),
                            );
                            continue;
                        }
                        let kind = inj.get("kind").and_then(|k| k.as_str()).unwrap_or("HEAD");
                        if kind != "HEAD" {
                            v.warn(
                                "injection_kind",
                                format!("{toml_path}: injection on `{hook}` uses kind `{kind}`; trait injections are limited to HEAD"),
                            );
                        }
                        injections.push((hook.to_string(), version));
                        if let Some(catalog) = catalog {
                            match catalog.resolve(hook) {
                                Ok(def) => {
                                    if version != 0 && (version as u32) < def.hook_version {
                                        v.fail(
                                            "hook_version",
                                            format!(
                                                "{toml_path}: hook `{hook}` version {version} < required {}",
                                                def.hook_version
                                            ),
                                        );
                                    }
                                }
                                Err(e) => v.fail("hook_missing", e),
                            }
                        }
                    }
                } else if api_path.is_none() {
                    // A trait-style bundle legitimately has no injections
                    // (catalog hooks dispatch through `mod_api::mods()`);
                    // only a manifest with neither is suspicious.
                    v.warn(
                        "no_injections",
                        format!("{toml_path}: no [[injection]] entries and no [api] table"),
                    );
                }
            }
            Err(e) => v.fail("mixin_toml_parse", format!("{toml_path}: {e}")),
        }
    }

    // ---- sources lint
    for (name, bytes) in &entries {
        if name.starts_with(&prefix) && name.ends_with(".rs") {
            let text = String::from_utf8_lossy(bytes);
            if name.ends_with("src/lib.rs") && !text.contains("#![deny(warnings)]") {
                v.warn(
                    "source_lint",
                    format!("{name}: missing #![deny(warnings)] at the crate root"),
                );
            }
            for issue in crate::sources::lint(&text) {
                if issue.0 {
                    v.fail("source_lint", format!("{name}: {}", issue.1));
                } else {
                    v.warn("source_lint", format!("{name}: {}", issue.1));
                }
            }
        }
    }

    v
}

/// The launcher's bundle-id rule (`api::valid_id`), mirrored so a bundle the
/// launcher would refuse never leaves this pipeline clean.
fn valid_bundle_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Bindability checks for a trait-style (`[api]`) bundle, mirroring what the
/// launcher's `api::folder` + `api::stage` require at load time: the path
/// points at a crate inside the bundle depending on horizon-client, whose
/// root exports the `hooks()` entry point the generated executable calls.
fn validate_api_bundle(
    prefix: &str,
    api_path: &str,
    entries: &std::collections::BTreeMap<String, Vec<u8>>,
    v: &mut Verdict,
) {
    if api_path.is_empty()
        || api_path.starts_with('/')
        || api_path.split('/').any(|seg| seg == "..")
    {
        v.fail(
            "api_path",
            format!("{prefix}: invalid [api] path `{api_path}` (must be a relative folder in the bundle)"),
        );
        return;
    }
    let crate_prefix = format!("{prefix}{api_path}");
    let cargo_key = format!("{crate_prefix}/Cargo.toml");
    let Some(cargo) = entries.get(&cargo_key) else {
        v.fail(
            "api_crate_missing",
            format!("{cargo_key} is required: [api] path must point at a crate in the bundle"),
        );
        return;
    };
    let cargo_text = String::from_utf8_lossy(cargo);
    match cargo_text.parse::<toml::Value>() {
        Ok(parsed) => {
            if parsed
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .is_none()
            {
                v.fail(
                    "api_crate_name",
                    format!("{cargo_key}: missing package name"),
                );
            }
            let has_client = parsed
                .get("dependencies")
                .and_then(|d| d.as_table())
                .is_some_and(|deps| {
                    deps.iter().any(|(k, val)| {
                        k == "horizon-client"
                            || val.get("package").and_then(|p| p.as_str()) == Some("horizon-client")
                    })
                });
            if !has_client {
                v.fail(
                    "api_crate_dep",
                    format!(
                        "{cargo_key}: missing horizon-client dependency (the launcher rewrites it to the staged game)"
                    ),
                );
            }
        }
        Err(e) => v.fail("api_cargo_parse", format!("{cargo_key}: {e}")),
    }
    let lib_key = format!("{crate_prefix}/src/lib.rs");
    match entries.get(&lib_key) {
        Some(lib) => {
            let text = String::from_utf8_lossy(lib);
            if !text.contains("pub fn hooks()") {
                v.fail(
                    "hooks_export",
                    format!(
                        "{lib_key}: missing `pub fn hooks()` — the launcher's generated entry calls `mod_<i>::hooks()`"
                    ),
                );
            }
        }
        None => v.fail("api_lib_missing", format!("{lib_key} is required")),
    }
    let src_prefix = format!("{crate_prefix}/src/");
    let any_impl = entries.iter().any(|(name, bytes)| {
        name.starts_with(&src_prefix)
            && name.ends_with(".rs")
            && String::from_utf8_lossy(bytes).contains("horizon_client::mod_api::Hooks")
    });
    if !any_impl {
        v.warn(
            "api_no_hooks_impl",
            format!(
                "{crate_prefix}: no `impl horizon_client::mod_api::Hooks` found; the installed Hooks object implements no hooks"
            ),
        );
    }
}
