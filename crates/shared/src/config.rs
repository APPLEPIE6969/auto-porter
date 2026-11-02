//! `porter.toml` configuration (spec §11) with the additions documented in
//! docs/limitations.md (`hooks_json`, `mixins_revision`).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HorizonCfg {
    /// Pinned Horizon commit. Must be a full 40-hex SHA; the documented
    /// placeholder (all zeros) keeps the pipeline runnable offline but marks
    /// every report `horizon_revision_verified: false`.
    pub revision: String,
    /// Pinned horizon-mixins commit (40-hex) written into horizon-native.json.
    pub mixins_revision: String,
    /// NATIVE_HOOKS_API at the pinned revision; port-code fails on mismatch.
    pub hooks_api: u32,
    /// Path to native-hooks.json (the real one, or the seed catalog offline).
    pub hooks_json: String,
    pub client_path: String,
    pub ctl_path: String,
    pub port_pack_path: String,
}

impl Default for HorizonCfg {
    fn default() -> Self {
        HorizonCfg {
            revision: "0".repeat(40),
            mixins_revision: "0".repeat(40),
            hooks_api: 0,
            hooks_json: String::new(),
            client_path: "~/.horizon/horizon-client".to_string(),
            ctl_path: "~/.horizon/horizon-ctl".to_string(),
            port_pack_path: "~/.cargo/bin/horizon-port-pack".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmCfg {
    pub provider: String,
    pub model: String,
    pub max_retries: u32,
    pub timeout_secs: u64,
    pub temperature: f64,
}

impl Default for LlmCfg {
    fn default() -> Self {
        LlmCfg {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-5-5".to_string(),
            max_retries: 3,
            timeout_secs: 60,
            temperature: 0.1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ParityCfg {
    pub java_server_dir: String,
    pub scenario_timeout_secs: u64,
    pub shards: u32,
}

impl Default for ParityCfg {
    fn default() -> Self {
        ParityCfg {
            java_server_dir: "tests/parity/java-oracle/".to_string(),
            scenario_timeout_secs: 30,
            shards: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InGameCfg {
    pub ssim_threshold: f64,
    pub ssim_advisory: bool,
    pub golden_dir: String,
    pub virtual_display: bool,
}

impl Default for InGameCfg {
    fn default() -> Self {
        InGameCfg {
            ssim_threshold: 0.95,
            ssim_advisory: true,
            golden_dir: "tests/in-game/golden/".to_string(),
            virtual_display: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputCfg {
    pub dir: String,
    pub include_tests: bool,
    pub include_report: bool,
}

impl Default for OutputCfg {
    fn default() -> Self {
        OutputCfg {
            dir: "./output/".to_string(),
            include_tests: true,
            include_report: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct PorterConfig {
    pub horizon: HorizonCfg,
    pub llm: LlmCfg,
    pub parity: ParityCfg,
    pub in_game: InGameCfg,
    pub output: OutputCfg,
}

impl PorterConfig {
    /// Load `porter.toml`. Missing file -> defaults (with the zero placeholder
    /// revision), so the pipeline can still run and say so in its report.
    pub fn load(path: &Path) -> PorterConfig {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                eprintln!(
                    "warning: {} is not valid TOML ({}); using defaults",
                    path.display(),
                    e
                );
                PorterConfig::default()
            }),
            Err(_) => PorterConfig::default(),
        }
    }

    /// Revision usable for bundle generation: 40-hex and not the zero placeholder.
    pub fn revision_usable(&self) -> bool {
        is_real_sha(&self.horizon.revision)
    }

    /// Expand a `~`-prefixed path from the config.
    pub fn expand(&self, p: &str) -> PathBuf {
        expand_tilde(p)
    }
}

pub fn is_real_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()) && s.chars().any(|c| c != '0')
}

pub fn is_40_hex(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}
