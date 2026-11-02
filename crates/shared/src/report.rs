//! PortReport (pipeline output) and VerificationReport (phase 5 output), spec §6.4/§7.

use crate::error::PortError;
use crate::types::{FeatureKind, Tier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Overall {
    Pass,
    PassWithWarnings,
    Partial,
    Failed,
}

impl Overall {
    pub fn as_str(&self) -> &'static str {
        match self {
            Overall::Pass => "PASS",
            Overall::PassWithWarnings => "PASS_WITH_WARNINGS",
            Overall::Partial => "PARTIAL",
            Overall::Failed => "FAILED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureStatus {
    Ported,
    Partial,
    Skipped,
    Blocked,
}

impl FeatureStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            FeatureStatus::Ported => "PORTED",
            FeatureStatus::Partial => "PARTIAL",
            FeatureStatus::Skipped => "SKIPPED",
            FeatureStatus::Blocked => "BLOCKED",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Outputs {
    pub hmod: Option<String>,
    pub hnmod: Option<String>,
    pub report: Option<String>,
    pub tests: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureOutcome {
    pub feature_id: String,
    pub kind: FeatureKind,
    pub tier: Tier,
    pub status: FeatureStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReportSummary {
    pub tiers: BTreeMap<String, u32>,
    pub ported: u32,
    pub partial: u32,
    pub skipped: u32,
    pub blocked: u32,
    pub total_features: u32,
}

/// The structured result of one `auto-porter port` run (spec §7 output contract).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortReport {
    pub schema_version: u32,
    pub mod_id: String,
    pub loader: String,
    pub source_jar: String,
    pub generated_by: String,
    pub horizon_revision: Option<String>,
    /// false while the pinned revision is the documented placeholder (see docs/limitations.md).
    pub horizon_revision_verified: bool,
    /// "horizon-port-pack" | "built-in-fallback" | null
    pub data_carry: Option<String>,
    pub outputs: Outputs,
    pub features: Vec<FeatureOutcome>,
    pub errors: Vec<PortError>,
    pub summary: ReportSummary,
    pub overall: Overall,
}

impl PortReport {
    pub fn new(mod_id: &str, loader: &str, source_jar: &str) -> Self {
        PortReport {
            schema_version: 1,
            mod_id: mod_id.to_string(),
            loader: loader.to_string(),
            source_jar: source_jar.to_string(),
            generated_by: format!("auto-porter {}", env!("CARGO_PKG_VERSION")),
            horizon_revision: None,
            horizon_revision_verified: false,
            data_carry: None,
            outputs: Outputs::default(),
            features: Vec::new(),
            errors: Vec::new(),
            summary: ReportSummary::default(),
            overall: Overall::Partial,
        }
    }

    /// Recompute the tier counts and per-status counters, then derive `overall`.
    pub fn finalize(&mut self) {
        let mut tiers = BTreeMap::new();
        let mut ported = 0u32;
        let mut partial = 0u32;
        let mut skipped = 0u32;
        let mut blocked = 0u32;
        for f in &self.features {
            *tiers.entry(f.tier.as_str().to_string()).or_insert(0) += 1;
            match f.status {
                FeatureStatus::Ported => ported += 1,
                FeatureStatus::Partial => partial += 1,
                FeatureStatus::Skipped => skipped += 1,
                FeatureStatus::Blocked => blocked += 1,
            }
        }
        self.summary = ReportSummary {
            tiers,
            ported,
            partial,
            skipped,
            blocked,
            total_features: self.features.len() as u32,
        };
        self.overall = if !self.errors.is_empty() && ported == 0 && partial == 0 {
            Overall::Failed
        } else if self.errors.is_empty() && skipped == 0 && blocked == 0 {
            if self
                .features
                .iter()
                .any(|f| f.status == FeatureStatus::Partial)
            {
                Overall::PassWithWarnings
            } else {
                Overall::Pass
            }
        } else if ported > 0 || partial > 0 {
            Overall::PassWithWarnings
        } else {
            Overall::Partial
        };
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Skipped,
}

impl CheckStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CheckStatus::Pass => "pass",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
            CheckStatus::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticResult {
    pub schema: CheckStatus,
    pub hooks: CheckStatus,
    pub deps: CheckStatus,
    pub issues: Vec<StaticIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticIssue {
    pub code: String,
    pub severity: CheckStatus,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParityFailure {
    pub scenario: String,
    pub expected: serde_json::Value,
    pub actual: serde_json::Value,
    pub diagnosis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParityResult {
    pub status: CheckStatus,
    pub scenarios_run: u32,
    pub scenarios_passed: u32,
    pub scenarios_failed: u32,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub failures: Vec<ParityFailure>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InGameResult {
    pub status: CheckStatus,
    pub steps_run: u32,
    pub steps_passed: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub screenshot_ssim: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileResult {
    pub status: CheckStatus,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
}

/// Phase 5 verification report (spec §6.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReport {
    pub mod_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tier: Option<String>,
    pub r#static: StaticResult,
    pub compile: CompileResult,
    pub parity: ParityResult,
    pub in_game: InGameResult,
    pub overall: Overall,
    pub human_review_needed: Vec<String>,
}
