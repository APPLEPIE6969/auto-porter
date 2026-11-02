//! Hook catalog: the parsed shape of `native-hooks.json` (spec §0.4/§0.5, §3.1).
//!
//! The catalog is always loaded from a JSON file at the pinned revision;
//! `hooks/native-hooks.json` is a committed snapshot of
//! `horizon-rust/horizon`'s file at the revision in porter.toml. The real
//! schema is `{native_hooks_api?, source?, helpers: [{name, target}],
//! hooks: [{name, version, target, mod_api: {contract, ...}, neoforge?,
//! side?, cancellable?}]}`. `NATIVE_HOOKS_API` itself is stamped into
//! `crates/horizon-client/src/game/native_hooks.rs` upstream, so the snapshot
//! records it in the `native_hooks_api` annotation key and the loader refuses
//! to guess it (porter.toml `hooks_api` must match — the build fails if it
//! differs).

use serde::{Deserialize, Serialize};
use std::path::Path;

/// One entry of the catalog's global `helpers` list (the Host-side methods a
/// native mod may call, e.g. `set_block`, `give_items`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperDef {
    pub name: String,
    /// Fully-qualified method path, e.g.
    /// `horizon_client::game::native_server::Game<'_>::native_tell`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target: Option<String>,
}

/// Raw hook entry as it appears in the real JSON (contract is nested in
/// `mod_api`, the version field is named `version`).
#[derive(Debug, Clone, Deserialize)]
struct RawHook {
    name: String,
    #[serde(default = "default_version")]
    version: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    mod_api: Option<RawModApi>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    neoforge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    cancellable: Option<bool>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize)]
struct RawModApi {
    contract: Option<String>,
}

fn default_contract() -> String {
    "listen".to_string()
}

/// Normalized hook definition used across the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HookDef {
    pub name: String,
    /// listen | cancel | replace | collect (from `mod_api.contract`).
    #[serde(default = "default_contract")]
    pub contract: String,
    pub hook_version: u32,
    /// Exact trait-method path of the hook (e.g.
    /// `horizon_client::game::native_hooks::Game<'_>::native_use_item`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub neoforge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cancellable: Option<bool>,
}

impl Default for HookDef {
    fn default() -> Self {
        HookDef {
            name: String::new(),
            contract: "listen".to_string(),
            hook_version: 1,
            target: None,
            neoforge: None,
            side: None,
            cancellable: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct HookCatalog {
    /// NATIVE_HOOKS_API at the pinned revision (annotation key of the
    /// snapshot; the upstream value lives in the client source).
    pub native_hooks_api: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
    pub hooks: Vec<HookDef>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub helpers: Vec<HelperDef>,
}

impl HookCatalog {
    pub fn load(path: &Path) -> Result<HookCatalog, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read hook catalog {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("cannot parse hook catalog {}: {e}", path.display()))
    }

    /// Prefer an explicit path; fall back to the committed catalog snapshot
    /// of the pinned revision.
    pub fn load_or_snapshot(explicit: Option<&Path>) -> Result<HookCatalog, String> {
        if let Some(p) = explicit {
            return HookCatalog::load(p);
        }
        let snap = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../hooks/native-hooks.json");
        let snap = if snap.exists() {
            snap
        } else {
            Path::new("hooks/native-hooks.json").to_path_buf()
        };
        HookCatalog::load(&snap)
    }

    /// Helper names available to native mods (the catalog's global `helpers`
    /// list; the real file does not scope helpers per hook).
    pub fn helper_names(&self) -> Vec<String> {
        self.helpers.iter().map(|h| h.name.clone()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&HookDef> {
        self.hooks.iter().find(|h| h.name == name)
    }

    /// Resolve a mapping-table entry. Returns Err(message) when the name is
    /// missing from the catalog — the xtask gate fails the build on this.
    pub fn resolve(&self, name: &str) -> Result<&HookDef, String> {
        self.get(name)
            .ok_or_else(|| format!("hook `{name}` is missing from the pinned native-hooks.json"))
    }
}

// `RawHook` normalization: serde cannot pull `mod_api.contract` up one level,
// so parse raw and convert explicitly.
impl HookCatalog {
    pub fn parse(text: &str) -> Result<HookCatalog, String> {
        #[derive(Deserialize)]
        struct RawFile {
            #[serde(default)]
            native_hooks_api: u32,
            #[serde(default)]
            source: Option<String>,
            #[serde(default)]
            helpers: Vec<HelperDef>,
            #[serde(default)]
            hooks: Vec<RawHook>,
        }
        let raw: RawFile =
            serde_json::from_str(text).map_err(|e| format!("cannot parse hook catalog: {e}"))?;
        let hooks = raw
            .hooks
            .into_iter()
            .map(|h| HookDef {
                name: h.name,
                contract: h
                    .mod_api
                    .and_then(|m| m.contract)
                    .unwrap_or_else(default_contract),
                hook_version: h.version,
                target: h.target,
                neoforge: h.neoforge,
                side: h.side,
                cancellable: h.cancellable,
            })
            .collect();
        if raw.native_hooks_api == 0 {
            return Err(
                "hook catalog does not declare `native_hooks_api` (NATIVE_HOOKS_API); \
                 refusing to guess it"
                    .to_string(),
            );
        }
        Ok(HookCatalog {
            native_hooks_api: raw.native_hooks_api,
            source: raw.source,
            hooks,
            helpers: raw.helpers,
        })
    }
}
