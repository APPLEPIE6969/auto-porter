//! PortReport error model (spec §9). The pipeline never fails silently: every
//! failure becomes a `PortError` in the `PortReport`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Decompose,
    Classify,
    PortData,
    PortCode,
    Verify,
}

impl Phase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase::Decompose => "decompose",
            Phase::Classify => "classify",
            Phase::PortData => "port_data",
            Phase::PortCode => "port_code",
            Phase::Verify => "verify",
        }
    }
}

/// Error taxonomy from spec §9. `llm_unavailable` is an additive variant
/// (documented in docs/limitations.md): the LLM was never configured, which is
/// a different situation from a 60 s timeout and must not be reported as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    DecompileFailed,
    UnknownMixinTarget,
    NoHookEquivalent,
    CompileError,
    ParityMismatch,
    VisualMismatch,
    MissingDependency,
    ApiVersionMismatch,
    OracleUnavailable,
    LlmTimeout,
    /// Additive: no API key / unsupported provider configured for the fallback path.
    LlmUnavailable,
    /// Additive: the provider answered but rejected or returned nothing usable
    /// (HTTP error status, undecodable body, empty completion).
    LlmFailed,
}

impl ErrorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorType::DecompileFailed => "decompile_failed",
            ErrorType::UnknownMixinTarget => "unknown_mixin_target",
            ErrorType::NoHookEquivalent => "no_hook_equivalent",
            ErrorType::CompileError => "compile_error",
            ErrorType::ParityMismatch => "parity_mismatch",
            ErrorType::VisualMismatch => "visual_mismatch",
            ErrorType::MissingDependency => "missing_dependency",
            ErrorType::ApiVersionMismatch => "api_version_mismatch",
            ErrorType::OracleUnavailable => "oracle_unavailable",
            ErrorType::LlmTimeout => "llm_timeout",
            ErrorType::LlmUnavailable => "llm_unavailable",
            ErrorType::LlmFailed => "llm_failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortError {
    pub phase: Phase,
    pub feature: Option<String>,
    pub error_type: ErrorType,
    pub message: String,
    pub java_source: Option<String>,
    /// What a human could do.
    pub suggestion: Option<String>,
    /// Which Horizon issue/PR would close the gap.
    pub horizon_gap: Option<String>,
}

impl PortError {
    pub fn new(phase: Phase, error_type: ErrorType, message: impl Into<String>) -> Self {
        PortError {
            phase,
            feature: None,
            error_type,
            message: message.into(),
            java_source: None,
            suggestion: None,
            horizon_gap: None,
        }
    }

    pub fn for_feature(mut self, feature: impl Into<String>) -> Self {
        self.feature = Some(feature.into());
        self
    }

    pub fn with_source(mut self, java_source: impl Into<String>) -> Self {
        self.java_source = Some(java_source.into());
        self
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    pub fn with_gap(mut self, gap: impl Into<String>) -> Self {
        self.horizon_gap = Some(gap.into());
        self
    }
}
