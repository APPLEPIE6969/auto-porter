//! LLM fallback for features no template covers (spec §10).
//!
//! Provider-configurable; the model name comes from porter.toml, never from
//! code. Missing credentials are `LlmUnavailable`, a real timeout is
//! `LlmTimeout` — the two are deliberately distinct. Providers: `anthropic`,
//! `openai`, and `nvidia` (OpenAI-compatible; the key is read from the
//! provider's documented env var, model ids look like
//! `vendor/model:variant`). `base_url` overrides the endpoint for
//! self-hosted gateways and tests.
//!
//! Correctness by construction: every request carries a system message with
//! an explicit output contract; responses are normalized (fences and
//! wrapping prose stripped) and validated before use; a rejected candidate
//! is retried with the validator's reason appended to the prompt so the
//! model can repair itself. A model that declares the feature infeasible
//! (`// PORT_INFEASIBLE`) produces an honest skip, not invented code.

use shared::config::{LlmCfg, PorterConfig};
use shared::error::{ErrorType, Phase, PortError};
use std::io::Write;
use std::time::Duration;

/// Sentinel the model may return instead of code when the feature cannot be
/// expressed with the given hook contract. Treated as an honest skip.
pub const PORT_INFEASIBLE: &str = "// PORT_INFEASIBLE";

/// System message sent with every LLM request. This is the contract; the
/// task-specific prompt is built by [`build_prompt`].
pub const SYSTEM_CONTRACT: &str = r#"You are the code-generation backend of auto-porter, a pipeline that ports Minecraft Java Edition mods to Horizon.

CONTEXT:
- Horizon is Minecraft: Java Edition 26.3 rewritten in Rust.
- Native mods implement the generated `horizon_client::mod_api::Hooks` trait.
- The `Host` trait provides game helpers (spawn entities, play sounds, redstone, fluid tanks, network, ...).
- Hook contracts: "listen" (observe), "cancel" (veto), "replace" (override the return), "collect" (gather from all mods).
- In the generated trait, cancel/replace arrive as out-parameters (e.g. `canceled: &mut bool`, `result: &mut Option<...>`); the signature in the prompt is authoritative.

OUTPUT CONTRACT — violations are rejected and retried with the reason attached:
1. Output ONLY compilable Rust: the single method requested. No markdown fences, no prose, no explanation before or after.
2. The method signature must match the provided hook signature exactly. Do not rename it, do not add `main`, do not add `mod` declarations.
3. Use ONLY `mod_api` re-exports and `Host` methods. Never invent functions, crates, or Java-style calls. Never import `horizon_client` internals.
4. Never use `unsafe`.
5. Match the upstream Java behaviour shown in the prompt, including edge cases and ordering.
6. If the Java behaviour cannot be replicated with the given hook, helpers and contract, output exactly one line — // PORT_INFEASIBLE — and nothing else. An honest infeasible beats invented code."#;

pub struct LlmRequest {
    pub mod_id: String,
    pub hook_name: String,
    pub contract: String,
    pub hook_version: u32,
    pub java_ir: String,
    pub hook_signature: String,
    pub relevant_helpers: String,
    pub few_shot_examples: String,
    /// Validator reason from the previous rejected attempt, if any. When
    /// set, the prompt grows a PREVIOUS ATTEMPT REJECTED section so the
    /// model can repair its output instead of repeating the same mistake.
    pub rejection: Option<String>,
}

/// Build the task-specific user prompt (the system message carries the
/// output contract). Spec §10 structure, extended with the rejection
/// feedback for self-repair retries.
pub fn build_prompt(req: &LlmRequest) -> String {
    let rejection = match &req.rejection {
        Some(reason) => format!(
            "\nPREVIOUS ATTEMPT REJECTED:\nYour previous output was rejected by the static validator: {reason}\nFix that problem and return the corrected Rust method only.\n"
        ),
        None => String::new(),
    };
    format!(
        r#"TASK:
Implement `{hook_name}` (contract: {contract}, hook_version: {hook_version}) for the mod `{mod_id}`.

JAVA BEHAVIOUR (from the original mod; real upstream source when available):
{java_ir}

AVAILABLE HOOK SIGNATURE (from native-hooks.json — authoritative):
{hook_signature}

AVAILABLE HOST HELPERS:
{relevant_helpers}

WORKED EXAMPLES:
{few_shot_examples}
{rejection}
CONSTRAINTS:
- Use ONLY `mod_api` re-exports and `Host` methods; no `unsafe`; no invented APIs.
- Add a doc comment saying what the hook does.
- If the behaviour cannot be replicated with this hook, output exactly `// PORT_INFEASIBLE` and nothing else.

OUTPUT:
Return ONLY the Rust method (signature and body), no explanations, no markdown fences.
"#,
        hook_name = req.hook_name,
        contract = req.contract,
        hook_version = req.hook_version,
        mod_id = req.mod_id,
        java_ir = req.java_ir,
        hook_signature = req.hook_signature,
        relevant_helpers = req.relevant_helpers,
        few_shot_examples = req.few_shot_examples,
        rejection = rejection,
    )
}

pub enum LlmOutcome {
    Code(String),
    Unavailable(PortError),
    Timeout(PortError),
    /// Retryable failure: 408/429/5xx, transport errors, empty/undecodable
    /// bodies. The caller may re-send within the attempt budget.
    Failed(PortError),
    /// Non-retryable failure (auth 401/403, unknown model 404, malformed
    /// request 400/422). Retrying cannot heal it — fail immediately. The one
    /// carve-out is NVIDIA's `DEGRADED function cannot be invoked` 400,
    /// which is a server-side outage wearing a 4xx status; `call` routes it
    /// to `Failed` instead.
    Fatal(PortError),
}

/// One chat-completion request. When `AUTO_PORTER_LLM_TRACE` names a file,
/// every request/response pair is appended there as one JSON line (soak
/// analysis, CI debugging) — tracing is diagnostics only and never alters
/// outcomes.
#[allow(clippy::result_large_err)]
pub fn call(req: &LlmRequest, cfg: &PorterConfig) -> LlmOutcome {
    let started = std::time::Instant::now();
    let prompt = build_prompt(req);
    let (outcome, extra) = call_inner(cfg, &prompt);
    trace_call(req, cfg, &prompt, &outcome, &extra, started.elapsed());
    outcome
}

#[allow(clippy::result_large_err)]
fn call_inner(cfg: &PorterConfig, prompt: &str) -> (LlmOutcome, serde_json::Value) {
    let (url, headers, body): (String, Vec<(String, String)>, serde_json::Value) = match cfg
        .llm
        .provider
        .as_str()
    {
        "anthropic" => {
            let Some(key) = std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .filter(|k| !k.is_empty())
            else {
                return (
                    LlmOutcome::Unavailable(
                        PortError::new(
                            Phase::PortCode,
                            ErrorType::LlmUnavailable,
                            "ANTHROPIC_API_KEY is not set; the LLM fallback cannot run",
                        )
                        .with_suggestion(
                            "export ANTHROPIC_API_KEY, add a template for this pattern, or accept the SKIPPED feature",
                        ),
                    ),
                    serde_json::json!({}),
                );
            };
            (
                "https://api.anthropic.com/v1/messages".to_string(),
                vec![
                    ("x-api-key".to_string(), key),
                    ("anthropic-version".to_string(), "2023-06-01".to_string()),
                ],
                serde_json::json!({
                    "model": cfg.llm.model,
                    "max_tokens": 1024,
                    "temperature": cfg.llm.temperature,
                    "system": SYSTEM_CONTRACT,
                    "messages": [{"role": "user", "content": prompt}]
                }),
            )
        }
        "openai" | "nvidia" => {
            let (env_var, default_url) = match cfg.llm.provider.as_str() {
                "nvidia" => (
                    "NVIDIA_API_KEY",
                    "https://integrate.api.nvidia.com/v1/chat/completions",
                ),
                _ => (
                    "OPENAI_API_KEY",
                    "https://api.openai.com/v1/chat/completions",
                ),
            };
            let Some(key) = std::env::var(env_var).ok().filter(|k| !k.is_empty()) else {
                return (
                    LlmOutcome::Unavailable(
                        PortError::new(
                            Phase::PortCode,
                            ErrorType::LlmUnavailable,
                            format!("{env_var} is not set; the LLM fallback cannot run"),
                        )
                        .with_suggestion(format!(
                            "export {env_var} or add a template for this pattern"
                        )),
                    ),
                    serde_json::json!({}),
                );
            };
            let headers = vec![("authorization".to_string(), format!("Bearer {key}"))];
            // NVIDIA NIM hosts long-context reasoning models; the generous
            // token budget matches their documented chat-completions example.
            let max_tokens = if cfg.llm.provider == "nvidia" {
                16384
            } else {
                2048
            };
            let mut body = serde_json::json!({
                "model": cfg.llm.model,
                "max_tokens": max_tokens,
                "temperature": cfg.llm.temperature,
                "messages": [
                    {"role": "system", "content": SYSTEM_CONTRACT},
                    {"role": "user", "content": prompt}
                ]
            });
            if cfg.llm.provider == "nvidia" {
                // Max documented reasoning effort (config override allowed).
                body["reasoning_effort"] =
                    serde_json::Value::String(cfg.llm.reasoning_effort.clone());
                body["stream"] = serde_json::Value::Bool(false);
            }
            let url = cfg
                .llm
                .base_url
                .clone()
                .unwrap_or_else(|| default_url.to_string());
            (url, headers, body)
        }
        other => {
            return (
                LlmOutcome::Unavailable(PortError::new(
                    Phase::PortCode,
                    ErrorType::LlmUnavailable,
                    format!("unsupported LLM provider `{other}` (porter.toml [llm].provider)"),
                )),
                serde_json::json!({}),
            );
        }
    };

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(cfg.llm.timeout_secs))
        .build();

    let mut request = agent.post(&url);
    for (k, v) in &headers {
        request = request.set(k, v);
    }

    let response = request.send_json(body);
    match response {
        Ok(resp) => match resp.into_json::<serde_json::Value>() {
            Ok(json) => {
                let text = extract_text(&json, &cfg.llm.provider);
                // Provenance for the trace: finish reason and token usage
                // diagnose truncation (`length`) and reasoning-model stalls.
                let extra = serde_json::json!({
                    "finish_reason": json
                        .pointer("/choices/0/finish_reason")
                        .or_else(|| json.get("stop_reason"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    "usage": json.get("usage").cloned().unwrap_or(serde_json::Value::Null),
                    "raw_text": text
                        .as_deref()
                        .map(|t| truncate(t, 8000))
                        .unwrap_or_default(),
                });
                let outcome = match text
                    .map(|t| normalize_code(&t))
                    .filter(|t| !t.trim().is_empty())
                {
                    Some(code) => LlmOutcome::Code(code),
                    None => LlmOutcome::Failed(PortError::new(
                        Phase::PortCode,
                        ErrorType::LlmFailed,
                        "LLM returned an empty response",
                    )),
                };
                (outcome, extra)
            }
            Err(e) => (
                LlmOutcome::Failed(PortError::new(
                    Phase::PortCode,
                    ErrorType::LlmFailed,
                    format!("cannot decode LLM response: {e}"),
                )),
                serde_json::json!({ "decode_error": e.to_string() }),
            ),
        },
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            let err = PortError::new(
                Phase::PortCode,
                ErrorType::LlmFailed,
                format!(
                    "LLM returned an error status; body: {}",
                    truncate(&body, 400)
                ),
            );
            let extra = serde_json::json!({ "status": code, "body": truncate(&body, 600) });
            // NVIDIA's "DEGRADED function cannot be invoked" arrives as 400
            // but is a server-side outage — retry it (spec §10: the free
            // tier fails transiently by design).
            if LlmCfg::is_fatal_status(code) && !LlmCfg::is_infra_degraded_body(&body) {
                (LlmOutcome::Fatal(err), extra)
            } else {
                (LlmOutcome::Failed(err), extra)
            }
        }
        Err(ureq::Error::Transport(t)) => {
            let msg = t.to_string();
            let extra = serde_json::json!({ "transport": msg.clone() });
            if msg.contains("timed out") || msg.contains("timeout") {
                (
                    LlmOutcome::Timeout(PortError::new(
                        Phase::PortCode,
                        ErrorType::LlmTimeout,
                        format!("LLM call exceeded {}s", cfg.llm.timeout_secs),
                    )),
                    extra,
                )
            } else {
                (
                    LlmOutcome::Failed(PortError::new(
                        Phase::PortCode,
                        ErrorType::LlmFailed,
                        format!("LLM transport error: {msg}"),
                    )),
                    extra,
                )
            }
        }
    }
}

/// Append one JSON line describing this call to the file named by
/// `AUTO_PORTER_LLM_TRACE`. Failures to write are warned on stderr but
/// never affect the port.
fn trace_call(
    req: &LlmRequest,
    cfg: &PorterConfig,
    prompt: &str,
    outcome: &LlmOutcome,
    extra: &serde_json::Value,
    elapsed: Duration,
) {
    let Ok(path) = std::env::var("AUTO_PORTER_LLM_TRACE") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let (kind, error) = match outcome {
        LlmOutcome::Code(_) => ("code", None),
        LlmOutcome::Unavailable(e) => ("unavailable", Some(e.message.clone())),
        LlmOutcome::Timeout(e) => ("timeout", Some(e.message.clone())),
        LlmOutcome::Failed(e) => ("failed", Some(e.message.clone())),
        LlmOutcome::Fatal(e) => ("fatal", Some(e.message.clone())),
    };
    let mut entry = serde_json::json!({
        "ts_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        "provider": cfg.llm.provider,
        "model": cfg.llm.model,
        "mod_id": req.mod_id,
        "hook_name": req.hook_name,
        "outcome": kind,
        "error": error,
        "latency_ms": elapsed.as_millis() as u64,
        "prompt": prompt,
    });
    if let (Some(dst), Some(src)) = (entry.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    let line = entry.to_string();
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| {
            f.write_all(line.as_bytes())
                .and_then(|_| f.write_all(b"\n"))
        });
    if let Err(e) = written {
        eprintln!("auto-porter: cannot write LLM trace to {path}: {e}");
    }
}

fn extract_text(json: &serde_json::Value, provider: &str) -> Option<String> {
    match provider {
        "anthropic" => json
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            })
            .and_then(|b| b.get("text"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string()),
        "openai" | "nvidia" => json
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}

/// True when the whole response is the infeasible sentinel (allowing
/// surrounding whitespace).
pub fn is_infeasible(code: &str) -> bool {
    code.trim() == PORT_INFEASIBLE
}

/// Normalize a raw model response into candidate Rust code: strip markdown
/// fences, and if the model wrapped the code in prose ("Here is the port:
/// ... Hope this helps!"), extract the first-`fn`-to-last-`}` region.
/// The infeasible sentinel passes through untouched. Anything the
/// heuristics cannot fix comes back as-is — [`structural_issues`] will
/// name the problem and the caller retries with it.
pub fn normalize_code(text: &str) -> String {
    let stripped = strip_fences(text);
    if is_infeasible(&stripped) {
        return stripped;
    }
    if starts_code_like(&stripped) && brace_balance(&stripped) == 0 && stripped.contains("fn ") {
        return stripped;
    }
    match extract_fn_region(&stripped) {
        Some(region) => region,
        None => stripped,
    }
}

/// A response that already begins like Rust (signature, attribute, comment,
/// import) is taken at face value; anything else is prose around code and
/// goes through extraction.
fn starts_code_like(code: &str) -> bool {
    let t = code.trim_start();
    [
        "fn ", "fn(", "pub ", "use ", "//", "#[", "#!", "mod ", "async ", "const ",
    ]
    .iter()
    .any(|p| t.starts_with(p))
}

/// Structural validation of a normalized candidate, independent of the
/// provider. Returns (fatal, reason) pairs; an empty vec means usable. The
/// caller extends this with the shared lint (`unsafe`, internals imports).
pub fn structural_issues(code: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    if code.trim().is_empty() {
        out.push((true, "response is empty".to_string()));
        return out;
    }
    if code.contains("```") {
        out.push((
            true,
            "markdown fence residue (```); return raw Rust only".to_string(),
        ));
    }
    let mentions_sentinel = code.contains(PORT_INFEASIBLE);
    if mentions_sentinel && !is_infeasible(code) {
        out.push((
            true,
            "output mixes the // PORT_INFEASIBLE sentinel with code; return either the method or only the sentinel".to_string(),
        ));
    }
    if brace_balance(code) != 0 {
        out.push((true, "braces are not balanced".to_string()));
    }
    if !mentions_sentinel && !code.contains("fn ") {
        out.push((true, "no `fn` found; return the Rust method".to_string()));
    }
    out
}

/// Net brace depth: 0 means balanced (and never negative in a prefix).
fn brace_balance(code: &str) -> i64 {
    let mut depth: i64 = 0;
    for c in code.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return -1;
        }
    }
    depth
}

/// Extract from the first `fn` to the last `}` — good enough for
/// prose-wrapped code ("Here is the port: ...fn f() {}... done!"), and the
/// region must itself be brace-balanced to count.
fn extract_fn_region(text: &str) -> Option<String> {
    let start = text.find("fn ")?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    let region = &text[start..=end];
    if brace_balance(region) == 0 {
        Some(region.trim().to_string())
    } else {
        None
    }
}

fn strip_fences(text: &str) -> String {
    let mut out = text.trim().to_string();
    if out.starts_with("```") {
        if let Some(first_nl) = out.find('\n') {
            out = out[first_nl + 1..].to_string();
        }
    }
    if out.ends_with("```") {
        out.truncate(out.len() - 3);
    }
    out.trim().to_string()
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::config::LlmCfg;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Serializes the tests that mutate process env vars (API keys, trace
    /// path) — cargo runs tests in parallel threads and std::env is
    /// process-global.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Poison-tolerant acquisition: a panicking test must not cascade
    /// PoisonError failures into unrelated tests.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// True when `raw` holds a complete HTTP request: the header block is
    /// terminated and — when Content-Length is present — the body has fully
    /// arrived.
    fn read_request_complete(raw: &[u8]) -> bool {
        let Some(h) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
            return false;
        };
        let head = String::from_utf8_lossy(&raw[..h]);
        let mut len = 0usize;
        for line in head.lines() {
            if let Some((k, v)) = line.split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
        }
        raw.len() >= h + 4 + len
    }

    #[test]
    fn degraded_body_signature_detected() {
        // verbatim body observed in CI on 2026-11-03
        let real = r#"{"status":400,"title":"Bad Request","detail":"Function id 'eb8265c4-e47d-4047-918d-c12746cc2cee': DEGRADED function cannot be invoked"}"#;
        assert!(LlmCfg::is_infra_degraded_body(real));
        assert!(!LlmCfg::is_infra_degraded_body(
            r#"{"error":{"message":"Invalid model id","code":400}}"#
        ));
        assert!(!LlmCfg::is_infra_degraded_body(""));
    }

    #[test]
    fn normalize_strips_fences_and_prose() {
        assert_eq!(normalize_code("```rust\nfn f() {}\n```"), "fn f() {}");
        let prose =
            "Here is the port:\n\n/// doc\nfn on_player_tick(&mut self) {}\n\nHope this helps!";
        let got = normalize_code(prose);
        assert!(got.starts_with("fn on_player_tick"), "got: {got}");
        assert!(!got.contains("Hope"), "prose must be stripped: {got}");
        // already-clean code passes through unchanged
        assert_eq!(normalize_code("/// d\nfn f() {}"), "/// d\nfn f() {}");
    }

    #[test]
    fn sentinel_is_honest_and_mixed_output_is_rejected() {
        assert_eq!(normalize_code("  // PORT_INFEASIBLE \n"), PORT_INFEASIBLE);
        assert!(is_infeasible(&normalize_code("// PORT_INFEASIBLE")));
        assert!(structural_issues(PORT_INFEASIBLE).is_empty());
        let mixed = normalize_code("// PORT_INFEASIBLE\nfn f() {}");
        let issues = structural_issues(&mixed);
        assert!(
            issues.iter().any(|(_, r)| r.contains("mixes")),
            "mixed sentinel+code must be rejected: {issues:?}"
        );
        assert_eq!(structural_issues("fn broken( {").len(), 1, "unbalanced");
        assert_eq!(structural_issues("just prose, no code").len(), 1);
    }

    #[test]
    fn prompt_carries_contract_and_rejection_feedback() {
        let mk = |rejection: Option<String>| LlmRequest {
            mod_id: "m".into(),
            hook_name: "on_player_tick".into(),
            contract: "listen".into(),
            hook_version: 1,
            java_ir: "ir".into(),
            hook_signature: "fn on_player_tick(&mut self)".into(),
            relevant_helpers: String::new(),
            few_shot_examples: String::new(),
            rejection,
        };
        let clean = build_prompt(&mk(None));
        assert!(!clean.contains("PREVIOUS ATTEMPT REJECTED"));
        assert!(clean.contains("PORT_INFEASIBLE"));
        let rejected = build_prompt(&mk(Some("line 1: `unsafe` is forbidden".into())));
        assert!(rejected.contains("PREVIOUS ATTEMPT REJECTED"));
        assert!(rejected.contains("`unsafe` is forbidden"));
        assert!(SYSTEM_CONTRACT.contains("OUTPUT CONTRACT"));
        assert!(SYSTEM_CONTRACT.contains("Never use `unsafe`"));
    }

    /// A single `call` against an in-process mock: the DEGRADED-400 (the
    /// NVIDIA server-side outage signature) must come back as retryable
    /// `Failed`, not `Fatal`. The full retry loop is exercised by the
    /// e2e test in the cli crate.
    #[test]
    fn degraded_400_is_retryable_not_fatal() {
        let _env = env_lock();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let served = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf); // request line + headers + body
                served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let body = r#"{"status":400,"title":"Bad Request","detail":"Function id 'eb8265c4-e47d-4047-918d-c12746cc2cee': DEGRADED function cannot be invoked"}"#;
                let resp = format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        std::env::set_var("OPENAI_API_KEY", "test-key-for-degraded");
        let mut cfg = PorterConfig::default();
        cfg.llm.provider = "openai".to_string();
        cfg.llm.base_url = Some(format!("http://{addr}/v1/chat/completions"));
        let req = LlmRequest {
            mod_id: "mockmod".into(),
            hook_name: "on_player_tick".into(),
            contract: "listen".into(),
            hook_version: 1,
            java_ir: "class Mock { void onPlayerTick() {} }".into(),
            hook_signature: "fn on_player_tick(&mut self)".into(),
            relevant_helpers: "".into(),
            few_shot_examples: "".into(),
            rejection: None,
        };

        let outcome = call(&req, &cfg);
        let made = requests.load(std::sync::atomic::Ordering::SeqCst);
        std::env::remove_var("OPENAI_API_KEY");

        assert_eq!(
            made, 1,
            "exactly one request; classification, not the retry loop"
        );
        assert!(
            matches!(outcome, LlmOutcome::Failed(_)),
            "DEGRADED 400 must be retryable (Failed), not Fatal"
        );
    }

    /// The genuine client error must stay fatal: an auth 401 aborts instead
    /// of burning the attempt budget.
    #[test]
    fn auth_401_stays_fatal() {
        let _env = env_lock();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let served = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let body = r#"{"error":{"message":"Invalid key","code":401}}"#;
                let resp = format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        std::env::set_var("OPENAI_API_KEY", "wrong-key");
        let mut cfg = PorterConfig::default();
        cfg.llm.provider = "openai".to_string();
        cfg.llm.base_url = Some(format!("http://{addr}/v1/chat/completions"));
        cfg.llm.retry_delay_ms = 1;
        let req = LlmRequest {
            mod_id: "mockmod".into(),
            hook_name: "on_player_tick".into(),
            contract: "listen".into(),
            hook_version: 1,
            java_ir: "class Mock {}".into(),
            hook_signature: "fn on_player_tick(&mut self)".into(),
            relevant_helpers: "".into(),
            few_shot_examples: "".into(),
            rejection: None,
        };

        let outcome = call(&req, &cfg);
        let made = requests.load(std::sync::atomic::Ordering::SeqCst);
        std::env::remove_var("OPENAI_API_KEY");

        assert_eq!(made, 1, "401 must not be retried");
        assert!(matches!(outcome, LlmOutcome::Fatal(_)));
    }

    /// The request must carry the system contract as a distinct message so
    /// the output contract survives even long task prompts.
    #[test]
    fn request_messages_carry_system_contract() {
        let _env = env_lock();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let store = captured.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                // Read the FULL request (header block + Content-Length body)
                // before answering: a single read() races TCP segmentation
                // and can capture headers only (the bug that flaked this
                // test — same class as the e2e read_http_request fix).
                let mut raw: Vec<u8> = Vec::new();
                let mut buf = [0u8; 16384];
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if read_request_complete(&raw) {
                        break;
                    }
                }
                *store.lock().unwrap() = String::from_utf8_lossy(&raw).to_string();
                let body = r#"{"choices":[{"message":{"content":"/// d\nfn on_player_tick(&mut self) {}"}}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        std::env::set_var("OPENAI_API_KEY", "test-key-for-contract");
        let mut cfg = PorterConfig::default();
        cfg.llm.provider = "openai".to_string();
        cfg.llm.base_url = Some(format!("http://{addr}/v1/chat/completions"));
        let req = LlmRequest {
            mod_id: "mockmod".into(),
            hook_name: "on_player_tick".into(),
            contract: "listen".into(),
            hook_version: 1,
            java_ir: "class Mock {}".into(),
            hook_signature: "fn on_player_tick(&mut self)".into(),
            relevant_helpers: "".into(),
            few_shot_examples: "".into(),
            rejection: None,
        };

        let outcome = call(&req, &cfg);
        let body = captured.lock().unwrap().clone();
        std::env::remove_var("OPENAI_API_KEY");

        assert!(
            body.contains("\"role\": \"system\"") || body.contains("\"role\":\"system\""),
            "request must carry a system message: {body:.400}"
        );
        assert!(
            body.contains("OUTPUT CONTRACT"),
            "system message must carry the output contract"
        );
        assert!(
            matches!(outcome, LlmOutcome::Code(ref c) if c.contains("fn on_player_tick")),
            "clean code comes back as Code"
        );
    }

    /// With AUTO_PORTER_LLM_TRACE set, every call appends one JSON line
    /// carrying the prompt, the outcome and response provenance — the soak
    /// harness and CI debugging depend on it.
    #[test]
    fn trace_env_writes_jsonl_per_call() {
        let _env = env_lock();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let body = r#"{"choices":[{"message":{"content":"/// d\nfn on_player_tick(&mut self) {}"},"finish_reason":"stop"}],"usage":{"total_tokens":42}}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let dir = std::env::temp_dir().join(format!(
            "ap-trace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("trace.jsonl");

        std::env::set_var("AUTO_PORTER_LLM_TRACE", &path);
        std::env::set_var("OPENAI_API_KEY", "test-key-for-trace");
        let mut cfg = PorterConfig::default();
        cfg.llm.provider = "openai".to_string();
        cfg.llm.base_url = Some(format!("http://{addr}/v1/chat/completions"));
        let req = LlmRequest {
            mod_id: "trace-probe".into(),
            hook_name: "on_player_tick".into(),
            contract: "listen".into(),
            hook_version: 1,
            java_ir: "class Mock {}".into(),
            hook_signature: "fn on_player_tick(&mut self)".into(),
            relevant_helpers: "".into(),
            few_shot_examples: "".into(),
            rejection: None,
        };
        let outcome = call(&req, &cfg);
        std::env::remove_var("AUTO_PORTER_LLM_TRACE");
        std::env::remove_var("OPENAI_API_KEY");

        assert!(matches!(outcome, LlmOutcome::Code(_)));
        let content = std::fs::read_to_string(&path).expect("trace file written");
        // Other tests may run `call` concurrently while the env var is set;
        // select this test's calls by its unique mod_id.
        let mine: Vec<serde_json::Value> = content
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("trace line is JSON"))
            .filter(|v| v["mod_id"] == "trace-probe")
            .collect();
        assert_eq!(mine.len(), 1, "exactly one line for this call: {mine:?}");
        let v = &mine[0];
        assert_eq!(v["outcome"], "code");
        assert_eq!(v["provider"], "openai");
        assert_eq!(v["finish_reason"], "stop");
        assert_eq!(v["usage"]["total_tokens"], 42);
        assert!(
            v["prompt"].as_str().unwrap().contains("TASK:"),
            "trace carries the task prompt"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
