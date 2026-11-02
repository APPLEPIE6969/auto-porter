//! Parity / in-game orchestration (spec §6.2–§6.3).
//!
//! The scenario logic lives in the Python harnesses (`tests/parity/run.py`,
//! `tests/in-game/run.py`); this module shells out to them and normalizes
//! their JSON output into a `VerificationReport`. When the environment lacks
//! `horizon-ctl` or the Java oracle, the corresponding layer reports
//! `OracleUnavailable` / `skipped` instead of pretending to pass.

use shared::config::PorterConfig;
use shared::error::{ErrorType, Phase, PortError};
use shared::report::{CheckStatus, InGameResult, ParityFailure, ParityResult};
use std::path::{Path, PathBuf};

fn find_repo_driver(rel: &str) -> Option<PathBuf> {
    // Drivers live at the repository root: walk up from cwd.
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join(rel);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn horizon_ctl_available(cfg: &PorterConfig) -> bool {
    let p = cfg.expand(&cfg.horizon.ctl_path);
    p.is_file() || which("horizon-ctl")
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

pub fn run_parity(
    cfg: &PorterConfig,
    port_dir: &Path,
    verbose: bool,
) -> (ParityResult, Vec<PortError>) {
    let mut errors = Vec::new();

    if !horizon_ctl_available(cfg) {
        errors.push(
            PortError::new(
                Phase::Verify,
                ErrorType::OracleUnavailable,
                "horizon-ctl is not available; the Horizon side of every parity scenario cannot run",
            )
            .with_suggestion("install Horizon (launcher or horizon-server) and enable Agent Control"),
        );
        return (
            ParityResult {
                status: CheckStatus::Skipped,
                scenarios_run: 0,
                scenarios_passed: 0,
                scenarios_failed: 0,
                failures: Vec::new(),
                note: Some("horizon-ctl not found".to_string()),
            },
            errors,
        );
    }

    let Some(driver) = find_repo_driver("tests/parity/run.py") else {
        errors.push(PortError::new(
            Phase::Verify,
            ErrorType::OracleUnavailable,
            "tests/parity/run.py not found relative to the working directory",
        ));
        return (
            ParityResult {
                status: CheckStatus::Skipped,
                scenarios_run: 0,
                scenarios_passed: 0,
                scenarios_failed: 0,
                failures: Vec::new(),
                note: Some("parity driver missing".to_string()),
            },
            errors,
        );
    };

    let scenarios = port_dir.join("tests/parity");
    let out = port_dir.join("results/parity");
    let _ = std::fs::create_dir_all(&out);
    let mut cmd = std::process::Command::new("python3");
    cmd.arg(&driver)
        .arg("--horizon-ctl")
        .arg(cfg.expand(&cfg.horizon.ctl_path))
        .arg("--scenarios")
        .arg(&scenarios)
        .arg("--output")
        .arg(&out)
        .arg("--timeout")
        .arg(cfg.parity.scenario_timeout_secs.to_string());
    if verbose {
        cmd.arg("--verbose");
    }
    match cmd.output() {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
            let results_path = out.join("results.json");
            if let Ok(text) = std::fs::read_to_string(&results_path) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                    return (parse_parity(&json), errors);
                }
            }
            errors.push(PortError::new(
                Phase::Verify,
                ErrorType::OracleUnavailable,
                format!(
                    "parity driver did not produce {results_path:?} (exit {:?}); stdout: {}",
                    o.status.code(),
                    stdout.trim()
                ),
            ));
            (
                ParityResult {
                    status: CheckStatus::Fail,
                    scenarios_run: 0,
                    scenarios_passed: 0,
                    scenarios_failed: 0,
                    failures: Vec::new(),
                    note: Some("driver produced no results.json".to_string()),
                },
                errors,
            )
        }
        Err(e) => {
            errors.push(PortError::new(
                Phase::Verify,
                ErrorType::OracleUnavailable,
                format!("cannot spawn python3 for the parity driver: {e}"),
            ));
            (
                ParityResult {
                    status: CheckStatus::Skipped,
                    scenarios_run: 0,
                    scenarios_passed: 0,
                    scenarios_failed: 0,
                    failures: Vec::new(),
                    note: Some("python3 unavailable".to_string()),
                },
                errors,
            )
        }
    }
}

fn parse_parity(json: &serde_json::Value) -> ParityResult {
    let s = |k: &str| json.get(k).and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    let failures = json
        .get("failures")
        .and_then(|f| f.as_array())
        .map(|arr| {
            arr.iter()
                .map(|f| ParityFailure {
                    scenario: f
                        .get("scenario")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    expected: f.get("expected").cloned().unwrap_or_default(),
                    actual: f.get("actual").cloned().unwrap_or_default(),
                    diagnosis: f
                        .get("diagnosis")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let failed = s("scenarios_failed");
    let env_unavailable = json
        .get("environment")
        .and_then(|e| e.get("horizon_unavailable"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    ParityResult {
        // an environment that cannot run the layer is a skip with a reason,
        // never a pass and never a fail (spec §6.2 honesty)
        status: match (&env_unavailable, failed) {
            (Some(_), _) => CheckStatus::Skipped,
            (None, f) if f > 0 => CheckStatus::Fail,
            (None, _) => CheckStatus::Pass,
        },
        scenarios_run: s("scenarios_run"),
        scenarios_passed: s("scenarios_passed"),
        scenarios_failed: failed,
        failures,
        note: env_unavailable,
    }
}

pub fn run_in_game(
    cfg: &PorterConfig,
    port_dir: &Path,
    verbose: bool,
) -> (InGameResult, Vec<PortError>) {
    let mut errors = Vec::new();
    if !horizon_ctl_available(cfg) {
        errors.push(
            PortError::new(
                Phase::Verify,
                ErrorType::OracleUnavailable,
                "horizon-ctl is not available; the in-game smoke layer cannot run",
            )
            .with_suggestion("install Horizon and enable Agent Control (the launcher's \"No AI\" switch blocks horizon-ctl)"),
        );
        return (
            InGameResult {
                status: CheckStatus::Skipped,
                steps_run: 0,
                steps_passed: 0,
                screenshot_ssim: None,
                note: Some("horizon-ctl not found".to_string()),
            },
            errors,
        );
    }
    let Some(driver) = find_repo_driver("tests/in-game/run.py") else {
        errors.push(PortError::new(
            Phase::Verify,
            ErrorType::OracleUnavailable,
            "tests/in-game/run.py not found relative to the working directory",
        ));
        return (
            InGameResult {
                status: CheckStatus::Skipped,
                steps_run: 0,
                steps_passed: 0,
                screenshot_ssim: None,
                note: Some("in-game driver missing".to_string()),
            },
            errors,
        );
    };

    let scenarios = port_dir.join("tests/in-game");
    let out = port_dir.join("results/in-game");
    let _ = std::fs::create_dir_all(&out);
    let mut cmd = std::process::Command::new("python3");
    cmd.arg(&driver)
        .arg("--horizon-ctl")
        .arg(cfg.expand(&cfg.horizon.ctl_path))
        .arg("--scenarios")
        .arg(&scenarios)
        .arg("--golden")
        .arg(cfg.in_game.golden_dir.clone())
        .arg("--output")
        .arg(&out)
        .arg("--ssim-threshold")
        .arg(cfg.in_game.ssim_threshold.to_string());
    if cfg.in_game.ssim_advisory {
        cmd.arg("--ssim-advisory");
    }
    if verbose {
        cmd.arg("--verbose");
    }
    match cmd.output() {
        Ok(o) => {
            let results_path = out.join("results.json");
            if let Ok(text) = std::fs::read_to_string(&results_path) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                    let s = |k: &str| json.get(k).and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                    let failed = json
                        .get("steps_failed")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0);
                    let env_unavailable = json
                        .get("environment")
                        .and_then(|e| e.get("horizon_unavailable"))
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                    return (
                        InGameResult {
                            // an environment that cannot run the layer is a
                            // skip with a reason — never a pass or a fail
                            status: match (&env_unavailable, failed) {
                                (Some(_), _) => CheckStatus::Skipped,
                                (None, f) if f > 0 => CheckStatus::Fail,
                                (None, _) => CheckStatus::Pass,
                            },
                            steps_run: s("steps_run"),
                            steps_passed: s("steps_passed"),
                            screenshot_ssim: json.get("screenshot_ssim").and_then(|x| x.as_f64()),
                            note: env_unavailable,
                        },
                        errors,
                    );
                }
            }
            errors.push(PortError::new(
                Phase::Verify,
                ErrorType::OracleUnavailable,
                format!(
                    "in-game driver produced no results.json (exit {:?})",
                    o.status.code()
                ),
            ));
            (
                InGameResult {
                    status: CheckStatus::Fail,
                    steps_run: 0,
                    steps_passed: 0,
                    screenshot_ssim: None,
                    note: Some("driver produced no results.json".to_string()),
                },
                errors,
            )
        }
        Err(e) => {
            errors.push(PortError::new(
                Phase::Verify,
                ErrorType::OracleUnavailable,
                format!("cannot spawn python3 for the in-game driver: {e}"),
            ));
            (
                InGameResult {
                    status: CheckStatus::Skipped,
                    steps_run: 0,
                    steps_passed: 0,
                    screenshot_ssim: None,
                    note: Some("python3 unavailable".to_string()),
                },
                errors,
            )
        }
    }
}
