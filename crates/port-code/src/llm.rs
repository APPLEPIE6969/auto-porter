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
/// task-specific prompt is built by [`build_prompt`]. The hook shapes it
/// references come from `hooks/hook-signatures.json` — extracted from the
/// pinned Horizon revision, not invented (docs/limitations.md).
///
/// Contract v3.3 (2026-11-06) adds the API-REALITIES section: the message
/// helpers' visibility facts verified in the pinned build's source and in a
/// booted game (broadcast is the only message helper the server's log
/// carries; tell is player-private; acting_player is Some inside
/// per-player dispatches, the single-player owner included).
pub const SYSTEM_CONTRACT: &str = r#"You are the code-generation backend of auto-porter, a pipeline that ports Minecraft Java Edition mods to Horizon.

CONTEXT:
- Horizon is Minecraft: Java Edition 26.3 rewritten in Rust.
- Native mods implement the `horizon_client::mod_api::Hooks` trait; the HOOK CONTRACT section of the task names the exact trait method you must produce.
- The method receives `host: &mut dyn Host` (game access: spawn entities, play sounds, redstone, fluid tanks, network, ...) and `event: &mut <X>Event` (the hook's data).
- Hook contracts: "listen" (observe), "cancel" (veto), "replace" (override values), "collect" (gather from all mods).
- Cancellation and overrides live in the signature and event, NOT in out-parameters: when the signature returns `Flow`, return `Flow::Cancel` to veto (skips later mods and the game's default action) and `Flow::Continue` otherwise; when the event struct exposes mutable fields, write the replacement value into them.
- The generated source is one module of the mod's bundle, compiled with `#![deny(warnings)]`; prefix any parameter you do not use with an underscore (e.g. `_host`).

API REALITIES (verified in the pinned Horizon build's source and in a booted game; follow these where a helper's doc comment is vaguer):
- `Host::broadcast(line)` shows the line in every player's chat AND writes it to the server's log — the only message helper a headless reader (the server console, a CI job) can observe. When the Java code has the ticking player announce status periodically (progress tickers, announcers), prefer `broadcast`: in a single-player world the audience is exactly that player.
- `Host::tell(uuid, line)` shows the line in one player's chat only; it never reaches the server's log, and it returns `false` when that player is not reachable. Keep it when the Java behaviour is strictly private to that player.
- `Host::acting_player()` is the player whose state the game swapped in for this dispatch: Some in per-player hooks (player_tick runs inside each player's swap — the single-player owner included), None in world- and server-level hooks. Reach the event's player through it and skip quietly when it is None.

OUTPUT CONTRACT — violations are rejected and retried with the reason attached:
1. Output ONLY compilable Rust: first line `use horizon_client::mod_api::*;`, then one `pub struct` (any name) that derives or implements `Default` — the launcher constructs the mod's Hooks object with `<Struct>::default()` — then its `impl horizon_client::mod_api::Hooks for <Struct>` block containing exactly the one method named in the HOOK CONTRACT. A doc comment above the method is allowed. No markdown fences, no prose, no `main`, no extra `mod` declarations, no other items.
2. Copy the method signature EXACTLY as the HOOK CONTRACT shows it — same receiver, same parameter types, same return type. Do not add, drop or reorder parameters.
3. Use ONLY the event's fields, the `host` parameter's methods (the AVAILABLE HOST HELPERS list) and `mod_api` re-exports. Never invent functions, crates, or Java-style calls. Never import `horizon_client` internals.
4. Never use `unsafe`.
5. Match the upstream Java behaviour shown in the prompt, including edge cases and ordering.
6. If the Java behaviour cannot be replicated with the given hook, helpers and contract, output exactly one line — // PORT_INFEASIBLE — and nothing else. An honest infeasible beats invented code."#;

/// Worked example shown in the task prompt: the real `Hooks` shape (exact
/// signature, `Flow` return, event fields read) rather than a template
/// skeleton.
pub const FEW_SHOT: &str = r#"use horizon_client::mod_api::*;

#[derive(Default)]
pub struct ExampleDiamondGate;

impl horizon_client::mod_api::Hooks for ExampleDiamondGate {
    /// Ported from NoPickupDelay::onItemPickup: veto absurd diamond pickups.
    /// (`_host`: unused here — unused parameters are underscore-prefixed, the
    /// crate builds with `#![deny(warnings)]`.)
    fn item_picked_up(&self, _host: &mut dyn Host, event: &mut ItemPickedUpEvent<'_>) -> Flow {
        if event.item.ends_with("diamond") && event.count > 64 {
            return Flow::Cancel; // veto: later mods and the game's pickup are skipped
        }
        Flow::Continue
    }
}"#;

pub struct LlmRequest {
    pub mod_id: String,
    pub hook_name: String,
    pub contract: String,
    pub hook_version: u32,
    pub java_ir: String,
    pub hook_signature: String,
    /// Ground-truth notes under the signature: Horizon docs for the hook,
    /// the event struct's fields, Flow semantics. Empty when the signature
    /// table is not attached.
    pub contract_notes: String,
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
            "\nPREVIOUS ATTEMPT REJECTED:\nYour previous output was rejected by the static validator: {reason}\nFix that problem and return the corrected Rust only.\n"
        ),
        None => String::new(),
    };
    format!(
        r#"TASK:
Implement `{hook_name}` (contract: {contract}, hook_version: {hook_version}) for the mod `{mod_id}`.

HOOK CONTRACT — copy this signature exactly (trait `horizon_client::mod_api::Hooks`):
{hook_signature}
{contract_notes}
JAVA BEHAVIOUR (from the original mod; real upstream source when available):
{java_ir}

AVAILABLE HOST HELPERS (methods on the `host` parameter):
{relevant_helpers}

WORKED EXAMPLE (shape to follow):
{few_shot_examples}
{rejection}
CONSTRAINTS:
- Use ONLY the event fields, `host` helper methods and `mod_api` re-exports; no `unsafe`; no invented APIs.
- Add a doc comment saying what the hook does.
- Give the struct `#[derive(Default)]` (or `impl Default`) — the launcher constructs the mod's hooks with `<Struct>::default()`.
- If the behaviour cannot be replicated with this hook, output exactly `// PORT_INFEASIBLE` and nothing else.

OUTPUT:
Return ONLY the Rust module (use line, struct, impl with the single method) — no explanations, no markdown fences.
"#,
        hook_name = req.hook_name,
        contract = req.contract,
        hook_version = req.hook_version,
        mod_id = req.mod_id,
        contract_notes = req.contract_notes,
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

/// Signature conformance: the code must implement exactly the hook method
/// from the HOOK CONTRACT. Returns the rejection reason when it does not —
/// the caller feeds it back to the model on the retry. Comparison is
/// structural: parameter names may differ (the contract allows `_host`
/// prefixes), but arity, types, order, receiver and return type may not.
pub fn signature_issue(code: &str, expected_signature: &str) -> Option<String> {
    let expected = normalize_signature(expected_signature);
    let name = expected
        .strip_prefix("fn ")
        .and_then(|rest| rest.split('(').next())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return None; // unparsable expectation: do not guess
    }
    let needle = format!("fn {name}");
    let mut found_any = false;
    let mut from = 0usize;
    while let Some(rel) = code[from..].find(&needle) {
        let abs = from + rel;
        from = abs + needle.len();
        // Identifier boundary: the preceding char must not extend the name
        // (`fn not_player_tick` must not match `fn player_tick`).
        if let Some(prev) = code[..abs].chars().next_back() {
            if prev.is_alphanumeric() || prev == '_' {
                continue;
            }
        }
        // Only whitespace between the name and the parameter list.
        let Some(open_rel) = code[from..].find('(') else {
            break;
        };
        if !code[from..from + open_rel].trim().is_empty() {
            continue;
        }
        let open = from + open_rel;
        let Some(close) = paren_span(code, open) else {
            break;
        };
        // Optional `-> Ret` up to the body brace.
        let body = code[close..]
            .find('{')
            .map(|b| close + b)
            .unwrap_or(code.len());
        let header = &code[abs..body];
        found_any = true;
        if signature_parts(header) == signature_parts(expected_signature) {
            return None;
        }
    }
    if found_any {
        Some(format!(
            "the `{name}` signature does not match the contract; copy it exactly: `{expected}`"
        ))
    } else {
        Some(format!(
            "no `fn {name}` implementation found; the output must implement the hook named in the HOOK CONTRACT"
        ))
    }
}

/// The struct named by the first `impl ... Hooks for <Struct>` header.
pub(crate) fn hooks_impl_struct(code: &str) -> Option<String> {
    let mut from = 0usize;
    while let Some(rel) = code[from..].find("Hooks for ") {
        let abs = from + rel + "Hooks for ".len();
        let name: String = code[abs..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            return Some(name);
        }
        from = abs;
    }
    None
}

/// The mod struct must be constructible with `<Struct>::default()`: the
/// launcher's generated entry point builds the mod's Hooks object that way
/// (`vec![mod_0::hooks()].into()`, `Box::new(<Struct>::default())`). Returns
/// the rejection reason when the struct carries no `Default`.
pub fn default_issue(code: &str) -> Option<String> {
    if is_infeasible(code) {
        return None; // honest skip: no struct is generated
    }
    let Some(name) = hooks_impl_struct(code) else {
        return None; // a missing impl is reported by signature_issue
    };
    let ident_boundary = |c: Option<char>| {
        c.map(|c| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(true)
    };
    // `#[derive(Default)]` (possibly among other derives) on the attribute
    // run directly above the struct declaration.
    let decl_marker = format!("struct {name}");
    let mut from = 0usize;
    while let Some(rel) = code[from..].find(&decl_marker) {
        let abs = from + rel;
        from = abs + decl_marker.len();
        if !ident_boundary(code[..abs].chars().next_back())
            || !ident_boundary(code[from..].chars().next())
        {
            continue;
        }
        let line_start = code[..abs].rfind('\n').map(|i| i + 1).unwrap_or(0);
        // Walk up over the attribute/comment block directly above the
        // declaration; the first item-like line ends the run (so a derive on
        // a sibling struct never counts for this one).
        let mut derived = false;
        for line in code[..line_start].lines().rev() {
            let t = line.trim();
            if t.is_empty() || t.starts_with("//") {
                continue;
            }
            if t.starts_with("#[") {
                if t.contains("derive") && t.contains("Default") {
                    derived = true;
                }
                continue;
            }
            break;
        }
        if derived {
            return None;
        }
    }
    // ... or a manual impl (any conventional spelling of the path).
    for path in [
        "impl Default for ",
        "impl core::default::Default for ",
        "impl std::default::Default for ",
    ] {
        let marker = format!("{path}{name}");
        if let Some(rel) = code.find(&marker) {
            let after = &code[rel + marker.len()..];
            if ident_boundary(after.chars().next()) {
                return None;
            }
        }
    }
    Some(format!(
        "struct `{name}` has no `Default`; add `#[derive(Default)]` (or `impl Default`) — the launcher constructs the mod's hooks with `<Struct>::default()`"
    ))
}

/// Unused parameters must be underscore-prefixed: the bundle's crate carries
/// `#![deny(warnings)]`, so a stub body that never reads `host`/`event` fails
/// the launcher-style build (Horizon's own `mod_api_hooks.rs` test writes
/// `_host`). Only names the body never mentions are flagged — a cheap check
/// that can miss exotic cases (the compile gate catches those) but never
/// rejects a name the body uses.
pub fn unused_param_issue(code: &str, expected_signature: &str) -> Option<String> {
    if is_infeasible(code) {
        return None; // honest skip: no method is generated
    }
    let expected = normalize_signature(expected_signature);
    let name = expected
        .strip_prefix("fn ")
        .and_then(|rest| rest.split('(').next())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return None; // unparsable expectation: do not guess
    }
    let needle = format!("fn {name}");
    let mut from = 0usize;
    while let Some(rel) = code[from..].find(&needle) {
        let abs = from + rel;
        from = abs + needle.len();
        if let Some(prev) = code[..abs].chars().next_back() {
            if prev.is_alphanumeric() || prev == '_' {
                continue;
            }
        }
        let Some(open_rel) = code[from..].find('(') else {
            break;
        };
        if !code[from..from + open_rel].trim().is_empty() {
            continue;
        }
        let open = from + open_rel;
        let Some(close) = paren_span(code, open) else {
            break;
        };
        let Some(body_open_rel) = code[close..].find('{') else {
            break;
        };
        let body_open = close + body_open_rel;
        let Some(body_close) = brace_span(code, body_open) else {
            break;
        };
        let params = &code[open + 1..close - 1];
        let body = &code[body_open..body_close];
        for part in split_top(params) {
            let Some(colon) = part.rfind(':') else {
                continue; // the `&self` receiver has no name
            };
            let raw = part[..colon].trim();
            let pname = raw
                .split_whitespace()
                .last()
                .unwrap_or("")
                .trim_start_matches('&');
            if pname.is_empty() || pname.starts_with('_') {
                continue;
            }
            if !body_contains_word(body, pname) {
                return Some(format!(
                    "parameter `{pname}` of `fn {name}` is never used; the crate builds with `#![deny(warnings)]` — prefix unused parameters with `_` (e.g. `_{pname}`)"
                ));
            }
        }
        return None; // the first well-formed occurrence is the implementation
    }
    None // no parsable method: signature_issue reports that
}

/// Index just past the `}` closing the brace group opened at `open`.
fn brace_span(code: &str, open: usize) -> Option<usize> {
    let mut depth = 0i64;
    for (i, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Word-boundary containment: `event` matches `event.key` but not `my_event`
/// or `events`.
fn body_contains_word(body: &str, word: &str) -> bool {
    let wb = |i: usize| {
        body.as_bytes()
            .get(i)
            .map(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .unwrap_or(false)
    };
    let mut from = 0usize;
    while let Some(rel) = body[from..].find(word) {
        let abs = from + rel;
        let after = abs + word.len();
        if (abs == 0 || !wb(abs - 1)) && !wb(after) {
            return true;
        }
        from = after.max(abs + 1);
    }
    false
}

/// Structural decomposition of a signature header: (fn name, parameter
/// types in order, return type or ""). Parameter NAMES are dropped (the
/// contract allows `_host`-style prefixes); everything else must match.
fn signature_parts(header: &str) -> Option<(String, Vec<String>, String)> {
    let s = normalize_signature(header);
    let name = s.strip_prefix("fn ")?.split('(').next()?.trim().to_string();
    let open = s.find('(')?;
    let close = paren_span(&s, open)?;
    let params: Vec<String> = split_top(&s[open + 1..close - 1])
        .iter()
        .map(|p| param_type(p))
        .collect();
    let rest = s[close..].trim();
    let ret = rest
        .strip_prefix("->")
        .map(|r| normalize_type(r.trim()))
        .unwrap_or_default();
    Some((name, params, ret))
}

/// Split on top-level commas only (a param like `fn() -> i32` keeps its
/// inner comma intact).
pub(crate) fn split_top(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i64;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' | '<' | '[' => depth += 1,
            ')' | '>' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(cur.clone());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

/// The declared type of one parameter: everything after the last top-level
/// `:` (names may be `_host`, `host`, anything).
fn param_type(p: &str) -> String {
    match p.rfind(':') {
        Some(i) => normalize_type(p[i + 1..].trim()),
        None => normalize_type(p.trim()),
    }
}

/// Collapse whitespace and drop a leading `pub ` so signatures compare
/// structurally.
fn normalize_signature(sig: &str) -> String {
    let s = sig.trim();
    let s = s.strip_prefix("pub ").unwrap_or(s);
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalize a type for comparison: drop the elided-lifetime form `<'_>` so
/// the contract's `ItemPickedUpEvent<'_>` matches an impl that keeps it.
fn normalize_type(t: &str) -> String {
    t.replace("<'_>", "").trim().to_string()
}

/// Index just past the `)` closing the paren group opened at `open`.
pub(crate) fn paren_span(code: &str, open: usize) -> Option<usize> {
    let mut depth = 0i64;
    for (i, c) in code[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i + 1);
                }
            }
            _ => {}
        }
    }
    None
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
            contract_notes: String::new(),
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
            contract_notes: "".into(),
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
            contract_notes: "".into(),
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
            contract_notes: "".into(),
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
            contract_notes: "".into(),
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

    /// The signature check accepts the exact contract shape (any parameter
    /// names, `pub` prefix, multi-line headers) and rejects wrong shapes.
    #[test]
    fn signature_issue_enforces_the_hook_contract() {
        let expected = "fn player_tick(&self, host: &mut dyn Host, event: &mut PlayerTickEvent)";
        let ok = "use horizon_client::mod_api::*;\n\npub struct M;\n\nimpl horizon_client::mod_api::Hooks for M {\n    /// doc\n    fn player_tick(&self, _host: &mut dyn Host, _event: &mut PlayerTickEvent) {\n        let _ = 1;\n    }\n}";
        assert_eq!(signature_issue(ok, expected), None, "exact shape passes");
        let multi_line = "pub struct M;\nimpl horizon_client::mod_api::Hooks for M {\n    fn player_tick(\n        &self,\n        host: &mut dyn Host,\n        event: &mut PlayerTickEvent,\n    ) {\n    }\n}";
        assert_eq!(
            signature_issue(multi_line, expected),
            None,
            "multi-line passes"
        );
        let wrong_params = "fn player_tick(&mut self) {}";
        assert!(
            signature_issue(wrong_params, expected)
                .unwrap()
                .contains("does not match"),
            "missing host/event params rejected"
        );
        let extra_params = "fn player_tick(&self, host: &mut dyn Host, event: &mut PlayerTickEvent, extra: u32) {}";
        assert!(
            signature_issue(extra_params, expected).is_some(),
            "extra params rejected"
        );
        let missing_flow =
            "fn item_picked_up(&self, host: &mut dyn Host, event: &mut ItemPickedUpEvent<'_>) {}";
        let flow_expected = "fn item_picked_up(&self, host: &mut dyn Host, event: &mut ItemPickedUpEvent<'_>) -> Flow";
        assert!(
            signature_issue(missing_flow, flow_expected).is_some(),
            "missing -> Flow rejected"
        );
        let renamed =
            "fn on_player_tick(&self, host: &mut dyn Host, event: &mut PlayerTickEvent) {}";
        assert!(
            signature_issue(renamed, expected)
                .unwrap()
                .contains("no `fn player_tick`"),
            "renamed method rejected"
        );
        // A longer name must not satisfy the shorter needle.
        let decoy =
            "fn not_player_tick(&self, host: &mut dyn Host, event: &mut PlayerTickEvent) {}";
        assert!(
            signature_issue(decoy, expected)
                .unwrap()
                .contains("no `fn player_tick`"),
            "identifier boundary respected"
        );
    }

    #[test]
    fn default_issue_requires_a_constructible_struct() {
        let derived = "use horizon_client::mod_api::*;\n\n#[derive(Default)]\npub struct M {\n    ticks: std::sync::atomic::AtomicU32,\n}\n\nimpl horizon_client::mod_api::Hooks for M {\n    fn player_tick(&self, _host: &mut dyn Host, _event: &mut PlayerTickEvent) {}\n}";
        assert_eq!(default_issue(derived), None, "derive(Default) passes");
        let manual = "pub struct M;\n\nimpl Default for M {\n    fn default() -> Self {\n        M\n    }\n}\n\nimpl horizon_client::mod_api::Hooks for M {\n    fn player_tick(&self, _host: &mut dyn Host, _event: &mut PlayerTickEvent) {}\n}";
        assert_eq!(default_issue(manual), None, "manual impl Default passes");
        let missing = "pub struct M {\n    ticks: u32,\n}\n\nimpl horizon_client::mod_api::Hooks for M {\n    fn player_tick(&self, _host: &mut dyn Host, _event: &mut PlayerTickEvent) {}\n}";
        let issue = default_issue(missing).expect("missing Default rejected");
        assert!(
            issue.contains("`M` has no `Default`") && issue.contains("derive(Default)"),
            "reason names the fix: {issue}"
        );
        // The struct-name scan must not accept a longer sibling identifier.
        let sibling = "pub struct M2 {\n    ticks: u32,\n}\npub struct M;\n\nimpl horizon_client::mod_api::Hooks for M {\n    fn player_tick(&self, _host: &mut dyn Host, _event: &mut PlayerTickEvent) {}\n}";
        assert!(
            default_issue(sibling).is_some(),
            "`struct M2` is not `struct M`"
        );
        // The sentinel is exempt: no struct is generated at all.
        assert_eq!(default_issue(PORT_INFEASIBLE), None, "sentinel passes");
        // Deriving on a DIFFERENT struct does not satisfy the impl's struct.
        let wrong_struct = "#[derive(Default)]\npub struct Other;\n\npub struct M;\n\nimpl horizon_client::mod_api::Hooks for M {\n    fn player_tick(&self, _host: &mut dyn Host, _event: &mut PlayerTickEvent) {}\n}";
        assert!(
            default_issue(wrong_struct).is_some(),
            "Default on another struct does not count"
        );
    }

    #[test]
    fn unused_param_issue_demands_underscore_prefixes() {
        let expected = "fn player_tick(&self, host: &mut dyn Host, event: &mut PlayerTickEvent)";
        // The deny(warnings) violation the first gate run caught: a stub body
        // that reads neither parameter.
        let bare = "use horizon_client::mod_api::*;\n\n#[derive(Default)]\npub struct M;\n\nimpl horizon_client::mod_api::Hooks for M {\n    fn player_tick(&self, host: &mut dyn Host, event: &mut PlayerTickEvent) {}\n}";
        let issue = unused_param_issue(bare, expected).expect("unused host+event rejected");
        assert!(
            issue.contains("`host`") && issue.contains("`_`"),
            "reason names the parameter and the fix: {issue}"
        );
        // Underscored stubs pass — Horizon's own mod_api_hooks.rs style.
        let underscored = bare.replace("host:", "_host:").replace("event:", "_event:");
        assert_eq!(unused_param_issue(&underscored, expected), None);
        // A parameter the body actually uses is never flagged; the sibling
        // that stays unread still is.
        let uses_event = "impl horizon_client::mod_api::Hooks for M {\n    fn player_tick(&self, host: &mut dyn Host, event: &mut PlayerTickEvent) {\n        let _ = event;\n    }\n}";
        let issue = unused_param_issue(uses_event, expected).expect("`host` still rejected");
        assert!(
            issue.contains("`host`") && !issue.contains("`event`"),
            "only the unused one is named: {issue}"
        );
        let uses_both = "impl horizon_client::mod_api::Hooks for M {\n    fn player_tick(&self, host: &mut dyn Host, event: &mut PlayerTickEvent) {\n        let _ = event;\n        let _ = &host;\n    }\n}";
        assert_eq!(unused_param_issue(uses_both, expected), None);
        // A name that merely appears inside a longer identifier is not a use.
        let decoy = "impl horizon_client::mod_api::Hooks for M {\n    fn player_tick(&self, host: &mut dyn Host, event: &mut PlayerTickEvent) {\n        let _events = 1;\n    }\n}";
        assert!(
            unused_param_issue(decoy, expected).is_some(),
            "`_events` does not use `event`"
        );
        // The sentinel is exempt.
        assert_eq!(unused_param_issue(PORT_INFEASIBLE, expected), None);
    }
}
