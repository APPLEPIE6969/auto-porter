//! Hook catalog: the parsed shape of `native-hooks.json` (spec §0.4/§0.5, §3.1).
//!
//! `Never hard-code counts or hook names`: the catalog is always loaded from a
//! JSON file at the pinned revision. `hooks/native-hooks.seed.json` is only a
//! clearly-labelled offline seed built from the Horizon docs; it is replaced by
//! the real `native-hooks.json` via porter.toml `hooks_json`.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HookDef {
    pub name: String,
    /// listen | cancel | replace | collect
    pub contract: String,
    pub hook_version: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub neoforge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cancellable: Option<bool>,
    /// Seed-only flag: false = proposed name from the docs, not yet verified.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confirmed: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub host_helpers: Vec<String>,
}

impl Default for HookDef {
    fn default() -> Self {
        HookDef {
            name: String::new(),
            contract: "listen".to_string(),
            hook_version: 1,
            neoforge: None,
            side: None,
            cancellable: None,
            confirmed: None,
            host_helpers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct HookCatalog {
    /// NATIVE_HOOKS_API at the pinned revision.
    pub native_hooks_api: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
    pub hooks: Vec<HookDef>,
}

impl HookCatalog {
    pub fn load(path: &Path) -> Result<HookCatalog, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read hook catalog {}: {e}", path.display()))?;
        let cat: HookCatalog = serde_json::from_str(&text)
            .map_err(|e| format!("cannot parse hook catalog {}: {e}", path.display()))?;
        if cat.native_hooks_api == 0 {
            return Err(format!(
                "hook catalog {} does not declare `native_hooks_api` (NATIVE_HOOKS_API); \
                 refusing to guess it",
                path.display()
            ));
        }
        Ok(cat)
    }

    /// Prefer an explicit path; fall back to the in-tree seed catalog so the
    /// pipeline runs before the pinned Horizon checkout exists.
    pub fn load_or_seed(explicit: Option<&Path>) -> Result<HookCatalog, String> {
        if let Some(p) = explicit {
            return HookCatalog::load(p);
        }
        let seed = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../hooks/native-hooks.seed.json");
        let seed = if seed.exists() {
            seed
        } else {
            Path::new("hooks/native-hooks.seed.json").to_path_buf()
        };
        HookCatalog::load(&seed)
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
