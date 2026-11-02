//! `.hnmod` (native mod bundle) validation: exactly `horizon-native.json` +
//! `mixins/<id>/…` at the top level, `horizon-mixin.toml` present, injections
//! resolve against the pinned hook catalog, sources pass the API lint.

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
                } else {
                    v.warn(
                        "no_injections",
                        format!("{toml_path}: no [[injection]] entries"),
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
