//! LLM fallback for features no template covers (spec §10).
//!
//! Provider-configurable; the model name comes from porter.toml, never from
//! code. Missing credentials are `LlmUnavailable`, a real 60 s timeout is
//! `LlmTimeout` — the two are deliberately distinct. Providers: `anthropic`,
//! `openai`, `openrouter`, and `nvidia` (OpenAI-compatible; the key is read
//! from the provider's documented env var, model ids look like
//! `vendor/model:variant`). `base_url` overrides the endpoint for
//! self-hosted gateways and tests.

use shared::config::{LlmCfg, PorterConfig};
use shared::error::{ErrorType, Phase, PortError};
use std::time::Duration;

pub struct LlmRequest {
    pub mod_id: String,
    pub hook_name: String,
    pub contract: String,
    pub hook_version: u32,
    pub java_ir: String,
    pub hook_signature: String,
    pub relevant_helpers: String,
    pub few_shot_examples: String,
}

/// Build the prompt exactly as specified in §10.
pub fn build_prompt(req: &LlmRequest) -> String {
    format!(
        r#"You are generating Rust code for a Horizon native mod.

CONTEXT:
- Horizon is Minecraft: Java Edition 26.3 rewritten in Rust.
- Native mods implement the generated `horizon_client::mod_api::Hooks` trait.
- The `Host` trait provides game helpers (spawn entities, play sounds, redstone, fluid tanks, network, ...).
- Hook contracts: "listen" (observe), "cancel" (veto), "replace" (override the return), "collect" (gather from all mods).
- In the generated trait, cancel/replace arrive as out-parameters (e.g. `canceled: &mut bool`,
  `result: &mut Option<...>`); match the signature in native-hooks.json exactly.

TASK:
Implement `{hook_name}` (contract: {contract}, hook_version: {hook_version}) for the mod `{mod_id}`.

JAVA BEHAVIOUR (from the original mod's bytecode):
{java_ir}

AVAILABLE HOOK SIGNATURE (from native-hooks.json):
{hook_signature}

AVAILABLE HOST HELPERS:
{relevant_helpers}

WORKED EXAMPLES:
{few_shot_examples}

CONSTRAINTS:
- Use ONLY `mod_api` re-exports and `Host` methods.
- Do NOT use `unsafe`.
- Do NOT import `horizon_client` internals.
- The method signature must match the generated trait exactly.
- Add a doc comment saying what the hook does.
- If the Java behaviour cannot be replicated with the available hooks,
  return the default (no-op) and add a `// TODO: {{reason}}` comment.

OUTPUT:
Return ONLY the Rust method (signature and body), no explanations.
"#,
        hook_name = req.hook_name,
        contract = req.contract,
        hook_version = req.hook_version,
        mod_id = req.mod_id,
        java_ir = req.java_ir,
        hook_signature = req.hook_signature,
        relevant_helpers = req.relevant_helpers,
        few_shot_examples = req.few_shot_examples,
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
    /// request 400/422). Retrying cannot heal it — fail immediately.
    Fatal(PortError),
}

#[allow(clippy::result_large_err)]
pub fn call(req: &LlmRequest, cfg: &PorterConfig) -> LlmOutcome {
    let prompt = build_prompt(req);

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
                return LlmOutcome::Unavailable(
                        PortError::new(
                            Phase::PortCode,
                            ErrorType::LlmUnavailable,
                            "ANTHROPIC_API_KEY is not set; the LLM fallback cannot run",
                        )
                        .with_suggestion(
                            "export ANTHROPIC_API_KEY, add a template for this pattern, or accept the SKIPPED feature",
                        ),
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
                    "messages": [{"role": "user", "content": prompt}]
                }),
            )
        }
        "openai" | "openrouter" | "nvidia" => {
            let (env_var, default_url) = match cfg.llm.provider.as_str() {
                "openrouter" => (
                    "OPENROUTER_API_KEY",
                    "https://openrouter.ai/api/v1/chat/completions",
                ),
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
                return LlmOutcome::Unavailable(
                    PortError::new(
                        Phase::PortCode,
                        ErrorType::LlmUnavailable,
                        format!("{env_var} is not set; the LLM fallback cannot run"),
                    )
                    .with_suggestion(format!(
                        "export {env_var} or add a template for this pattern"
                    )),
                );
            };
            let mut headers = vec![("authorization".to_string(), format!("Bearer {key}"))];
            if cfg.llm.provider == "openrouter" {
                // OpenRouter attribution headers (recommended by their docs).
                headers.push((
                    "http-referer".to_string(),
                    "https://github.com/APPLEPIE6969/auto-porter".to_string(),
                ));
                headers.push(("x-title".to_string(), "auto-porter".to_string()));
            }
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
                "messages": [{"role": "user", "content": prompt}]
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
            return LlmOutcome::Unavailable(PortError::new(
                Phase::PortCode,
                ErrorType::LlmUnavailable,
                format!("unsupported LLM provider `{other}` (porter.toml [llm].provider)"),
            ));
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
                match text
                    .map(|t| strip_fences(&t))
                    .filter(|t| !t.trim().is_empty())
                {
                    Some(code) => LlmOutcome::Code(code),
                    None => LlmOutcome::Failed(PortError::new(
                        Phase::PortCode,
                        ErrorType::LlmFailed,
                        "LLM returned an empty response",
                    )),
                }
            }
            Err(e) => LlmOutcome::Failed(PortError::new(
                Phase::PortCode,
                ErrorType::LlmFailed,
                format!("cannot decode LLM response: {e}"),
            )),
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
            if LlmCfg::is_fatal_status(code) {
                LlmOutcome::Fatal(err)
            } else {
                LlmOutcome::Failed(err)
            }
        }
        Err(ureq::Error::Transport(t)) => {
            let msg = t.to_string();
            if msg.contains("timed out") || msg.contains("timeout") {
                LlmOutcome::Timeout(PortError::new(
                    Phase::PortCode,
                    ErrorType::LlmTimeout,
                    format!("LLM call exceeded {}s", cfg.llm.timeout_secs),
                ))
            } else {
                LlmOutcome::Failed(PortError::new(
                    Phase::PortCode,
                    ErrorType::LlmFailed,
                    format!("LLM transport error: {msg}"),
                ))
            }
        }
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
        "openai" | "openrouter" | "nvidia" => json
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
