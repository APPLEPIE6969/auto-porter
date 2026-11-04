//! Phase 4: Port code — template-driven Rust generation with an LLM fallback
//! (spec §5). Output: a `.hnmod` zip (horizon-native.json + mixins/<id>/…).

pub mod llm;
pub mod scenarios;
pub mod sources;

use shared::catalog::{HookCatalog, HookSignature};
use shared::config::PorterConfig;
use shared::error::{ErrorType, Phase, PortError};
use shared::report::{CheckStatus, CompileResult, FeatureOutcome, FeatureStatus};
use shared::types::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct CodeGenOutcome {
    pub hnmod: Option<PathBuf>,
    pub outcomes: Vec<FeatureOutcome>,
    pub errors: Vec<PortError>,
    /// #![deny(warnings)] / unsafe / import scans on the generated sources.
    pub lint_clean: bool,
    pub compile: CompileResult,
    /// true when the pinned revision is a real SHA (not the zero placeholder).
    pub revision_verified: bool,
    pub hooks_used: Vec<HookRef>,
}

#[allow(clippy::too_many_arguments)]
pub fn port_code(
    plan: &PortPlan,
    decomposed: &DecomposedMod,
    catalog: &HookCatalog,
    cfg: &PorterConfig,
    revision_override: Option<&str>,
    out_dir: &Path,
    verbose: bool,
    sources: &sources::JavaSources,
) -> CodeGenOutcome {
    let mut outcomes = Vec::new();
    let mut errors = Vec::new();
    let mut modules: Vec<(String, String)> = Vec::new(); // (module file, content)
    let mut hooks_used: Vec<HookRef> = Vec::new();
    let mut lint_clean = true;

    let revision = revision_override
        .map(|s| s.to_string())
        .unwrap_or_else(|| cfg.horizon.revision.clone());
    let revision_verified = shared::config::is_real_sha(&revision);
    if cfg.horizon.hooks_api != 0 && cfg.horizon.hooks_api != catalog.native_hooks_api {
        errors.push(PortError::new(
            Phase::PortCode,
            ErrorType::ApiVersionMismatch,
            format!(
                "porter.toml hooks_api {} != catalog NATIVE_HOOKS_API {}",
                cfg.horizon.hooks_api, catalog.native_hooks_api
            ),
        ));
    }

    for feature in &plan.features {
        match feature.tier {
            Tier::A => outcomes.push(FeatureOutcome {
                feature_id: feature.id.clone(),
                kind: feature.kind,
                tier: feature.tier,
                status: FeatureStatus::Ported,
                detail: "data-only feature; carried by the data phase".to_string(),
            }),
            Tier::B | Tier::C => {
                if feature.tier == Tier::C {
                    outcomes.push(FeatureOutcome {
                        feature_id: feature.id.clone(),
                        kind: feature.kind,
                        tier: feature.tier,
                        status: FeatureStatus::Skipped,
                        detail:
                            "Tier C: partly automatic — human design required (see human_notes)"
                                .to_string(),
                    });
                    continue;
                }
                let Some(hook) = &feature.hook else {
                    outcomes.push(FeatureOutcome {
                        feature_id: feature.id.clone(),
                        kind: feature.kind,
                        tier: feature.tier,
                        status: FeatureStatus::Skipped,
                        detail: "no hook mapped; generation not attempted".to_string(),
                    });
                    continue;
                };

                let generated: Generated = match feature.template.as_deref() {
                    Some(template_name) => {
                        render_template(template_name, feature, hook, decomposed, verbose)
                            .map(|code| Generated::Code { code, note: None })
                            .unwrap_or(Generated::Failed)
                    }
                    None => llm_fallback(
                        feature,
                        hook,
                        decomposed,
                        catalog,
                        cfg,
                        sources,
                        &mut errors,
                    ),
                };

                match generated {
                    Generated::Code { code, note } => {
                        if verify_lint(&code, &mut errors, &feature.id) {
                            let slug = module_slug(&feature.id);
                            modules.push((format!("{slug}.rs"), code));
                            if !hooks_used.iter().any(|h| h == hook) {
                                hooks_used.push(hook.clone());
                            }
                            let mut detail = format!(
                                "generated from {}",
                                feature
                                    .template
                                    .clone()
                                    .unwrap_or_else(|| "LLM fallback".to_string())
                            );
                            if let Some(note) = note {
                                detail.push_str(&format!(" ({note})"));
                            }
                            outcomes.push(FeatureOutcome {
                                feature_id: feature.id.clone(),
                                kind: feature.kind,
                                tier: feature.tier,
                                status: FeatureStatus::Ported,
                                detail,
                            });
                        } else {
                            lint_clean = false;
                            outcomes.push(FeatureOutcome {
                                feature_id: feature.id.clone(),
                                kind: feature.kind,
                                tier: feature.tier,
                                status: FeatureStatus::Skipped,
                                detail: "generated source failed the static lint".to_string(),
                            });
                        }
                    }
                    Generated::Infeasible => {
                        outcomes.push(FeatureOutcome {
                            feature_id: feature.id.clone(),
                            kind: feature.kind,
                            tier: feature.tier,
                            status: FeatureStatus::Skipped,
                            detail: format!(
                                "model declared the feature infeasible for hook `{}` (// PORT_INFEASIBLE); honest skip — human design required",
                                hook.name
                            ),
                        });
                    }
                    Generated::Failed => outcomes.push(FeatureOutcome {
                        feature_id: feature.id.clone(),
                        kind: feature.kind,
                        tier: feature.tier,
                        status: FeatureStatus::Skipped,
                        detail: "generation failed; see errors".to_string(),
                    }),
                }
            }
            Tier::D => outcomes.push(FeatureOutcome {
                feature_id: feature.id.clone(),
                kind: feature.kind,
                tier: feature.tier,
                status: FeatureStatus::Blocked,
                detail: "Tier D: no Horizon equivalent; report only (GapReport)".to_string(),
            }),
        }
    }

    // ---- assemble the bundle (only when there is code to carry)
    let mut hnmod = None;
    if !modules.is_empty() {
        if !revision_verified {
            errors.push(
                PortError::new(
                    Phase::PortCode,
                    ErrorType::ApiVersionMismatch,
                    "the pinned Horizon revision is the zero placeholder; record a real full 40-hex commit in porter.toml or --horizon-revision",
                )
                .with_suggestion("see docs/limitations.md: pick the commit, verify it, pin it"),
            );
        }
        let mod_id = &plan.mod_id;
        let dest = out_dir.join(format!("{mod_id}.hnmod"));
        match assemble_bundle(
            &dest,
            mod_id,
            &decomposed.metadata.version,
            &revision,
            &cfg.horizon.mixins_revision,
            &hooks_used,
            catalog,
            &modules,
        ) {
            Ok(()) => hnmod = Some(dest),
            Err(e) => errors.push(e),
        }
    }

    // ---- compile gate: only the launcher-style build counts (spec §5.2.4)
    let compile = if cfg.expand(&cfg.horizon.ctl_path).is_file() || which("horizon-ctl") {
        // The launcher build path exists on this machine; the gate itself is
        // executed by the horizon-ctl driven verification layer.
        CompileResult {
            status: CheckStatus::Skipped,
            reason: Some(
                "compile gate runs through the horizon-ctl verification layer (verify --in-game); it is not part of code generation".to_string(),
            ),
        }
    } else {
        CompileResult {
            status: CheckStatus::Skipped,
            reason: Some(
                "launcher-style build unavailable in this environment: horizon-ctl not found; a standalone cargo check is not possible for native mods (docs/native-mods.md)".to_string(),
            ),
        }
    };

    CodeGenOutcome {
        hnmod,
        outcomes,
        errors,
        lint_clean,
        compile,
        revision_verified,
        hooks_used,
    }
}

fn verify_lint(code: &str, errors: &mut Vec<PortError>, feature_id: &str) -> bool {
    let issues = shared_lint(code);
    for (fatal, msg) in &issues {
        errors.push(
            PortError::new(Phase::PortCode, ErrorType::CompileError, msg.clone())
                .for_feature(feature_id.to_string()),
        );
        let _ = fatal;
    }
    issues.is_empty()
}

fn shared_lint(code: &str) -> Vec<(bool, String)> {
    // reuse the verify-crate lint without a dependency cycle: the checks are
    // tiny, so they are mirrored here intentionally.
    let mut out = Vec::new();
    for (i, line) in code.lines().enumerate() {
        let trimmed = line.split("//").next().unwrap_or(line);
        if trimmed.contains("unsafe") {
            out.push((true, format!("line {}: `unsafe` is forbidden", i + 1)));
        }
        for (idx, _) in trimmed.match_indices("horizon_client::") {
            let rest = &trimmed[idx + "horizon_client::".len()..];
            if !(rest.starts_with("mod_api") || rest.starts_with("Host")) {
                out.push((
                    true,
                    format!("line {}: horizon_client internals import", i + 1),
                ));
                break;
            }
        }
    }
    out
}

fn render_template(
    template_name: &str,
    feature: &Feature,
    hook: &HookRef,
    decomposed: &DecomposedMod,
    verbose: bool,
) -> Option<String> {
    let template = load_template(template_name)?;
    let java_source = feature.sources.first().cloned().unwrap_or_default();
    let action_note = feature
        .notes
        .first()
        .cloned()
        .unwrap_or_else(|| "port the original behaviour here".to_string());
    let todo = format!(
        "TODO: replicate `{java_source}` against the pinned native-hooks.json signature ({}), then let the launcher build verify it",
        hook.name
    );

    let struct_name = struct_name(&feature.id);
    let code = template
        .replace("{{MOD_ID}}", &decomposed.metadata.id)
        .replace("{{FEATURE_ID}}", &feature.id)
        .replace("{{FEATURE_TITLE}}", &feature.title)
        .replace("{{JAVA_SOURCE}}", &java_source)
        .replace("{{HOOK_NAME}}", &hook.name)
        .replace("{{HOOK_VERSION}}", &hook.hook_version.to_string())
        .replace(
            "{{CONTRACT}}",
            feature.contract.as_deref().unwrap_or("listen"),
        )
        .replace("{{CONDITION}}", "always")
        .replace("{{ACTION_NOTE}}", &action_note)
        .replace("{{TODO}}", &todo)
        .replace("{{STRUCT_NAME}}", &struct_name);
    if verbose {
        eprintln!(
            "  [port-code] rendered {template_name} -> {} ({})",
            struct_name, feature.id
        );
    }
    Some(code)
}

/// What one feature's code generation produced.
enum Generated {
    /// A candidate that passed the structural validation and the shared
    /// lint (template output skips the LLM-specific checks).
    Code { code: String, note: Option<String> },
    /// The model invoked the infeasible sentinel: an honest skip, recorded
    /// without an error entry.
    Infeasible,
    /// Generation failed; `errors` carries the reasons.
    Failed,
}

fn llm_fallback(
    feature: &Feature,
    hook: &HookRef,
    decomposed: &DecomposedMod,
    catalog: &HookCatalog,
    cfg: &PorterConfig,
    sources: &sources::JavaSources,
    errors: &mut Vec<PortError>,
) -> Generated {
    let java_ir = java_ir_for(feature, decomposed);
    // When the mod's real upstream source was fetched from its GitHub repo,
    // hand the model the actual code of the feature's classes — decompiled
    // signatures alone lose the behaviour the port must replicate.
    let java_ir = upstream_source_context(&java_ir, feature, sources);
    // Ground truth when the signature table is attached: the EXACT trait
    // method shape from the pinned Horizon revision. Without it, fall back
    // to the catalog metadata block (hook name, contract, version).
    let sig = catalog.signature(&hook.name);
    let hook_signature = match sig {
        Some(s) => s.signature.clone(),
        None => catalog
            .get(&hook.name)
            .map(|d| {
                format!(
                    "hook: {}\ncontract: {}\nhook_version: {}\ntarget: {}\nneoforge: {}\nside: {}",
                    d.name,
                    d.contract,
                    d.hook_version,
                    d.target.clone().unwrap_or_default(),
                    d.neoforge.clone().unwrap_or_default(),
                    d.side.clone().unwrap_or_default()
                )
            })
            .unwrap_or_default(),
    };
    let contract_notes = sig.map(contract_notes_text).unwrap_or_default();
    // The real catalog carries one global `helpers` list, not per-hook
    // host_helpers — give the model the full registry so it can pick, with
    // full signatures when known (names alone invite invented APIs).
    let helpers = if catalog.host_methods.is_empty() {
        catalog.helper_names().join(", ")
    } else {
        catalog.helper_signatures_text()
    };
    let examples = llm::FEW_SHOT.to_string();

    let max_attempts = cfg.llm.effective_max_attempts();
    let mut attempt = 0;
    // Validator reason fed back into the prompt on the next attempt, so a
    // rejected candidate gets repaired instead of re-rolling the dice on an
    // identical prompt.
    let mut rejection: Option<String> = None;
    let mut rejected_attempts = 0usize;
    while attempt < max_attempts {
        let req = llm::LlmRequest {
            mod_id: decomposed.metadata.id.clone(),
            hook_name: hook.name.clone(),
            contract: feature
                .contract
                .clone()
                .unwrap_or_else(|| "listen".to_string()),
            hook_version: hook.hook_version,
            java_ir: java_ir.clone(),
            hook_signature: hook_signature.clone(),
            contract_notes: contract_notes.clone(),
            relevant_helpers: helpers.clone(),
            few_shot_examples: examples.clone(),
            rejection: rejection.clone(),
        };
        match llm::call(&req, cfg) {
            llm::LlmOutcome::Code(code) => {
                // The sentinel is only honest when it is the WHOLE response;
                // `call` already normalized, so an exact match here means the
                // model deliberately declined the feature.
                if llm::is_infeasible(&code) {
                    return Generated::Infeasible;
                }
                // Structural validation first (fences, brace balance,
                // sentinel discipline), then the exact-signature check, then
                // the shared lint; the first reason is what the model sees
                // on the retry.
                let mut issues = llm::structural_issues(&code);
                if let Some(expected) = sig.map(|s| s.signature.as_str()) {
                    if let Some(reason) = llm::signature_issue(&code, expected) {
                        issues.push((true, reason));
                    }
                }
                issues.extend(shared_lint(&code));
                if issues.is_empty() {
                    let note = if rejected_attempts > 0 {
                        Some(format!(
                            "repaired after {rejected_attempts} rejected attempt(s)"
                        ))
                    } else {
                        None
                    };
                    return Generated::Code {
                        code: format!(
                            "// LLM-generated for {} (hook `{}`), reviewed by the static lint only\n{}",
                            feature.id, hook.name, code
                        ),
                        note,
                    };
                }
                rejected_attempts += 1;
                let reason = issues[0].1.clone();
                errors.push(
                    PortError::new(
                        Phase::PortCode,
                        ErrorType::CompileError,
                        format!("LLM output rejected: {reason}"),
                    )
                    .for_feature(feature.id.clone()),
                );
                rejection = Some(reason);
            }
            llm::LlmOutcome::Unavailable(e) => {
                errors.push(e.for_feature(feature.id.clone()));
                return Generated::Failed;
            }
            // Auth / bad-model / malformed-request: retrying cannot heal it.
            llm::LlmOutcome::Fatal(e) => {
                errors.push(e.for_feature(feature.id.clone()));
                return Generated::Failed;
            }
            // Transport / rate-limit / empty body: not the model's fault, so
            // any pending rejection feedback stays for the next attempt.
            llm::LlmOutcome::Timeout(e) | llm::LlmOutcome::Failed(e) => {
                errors.push(e.for_feature(feature.id.clone()));
            }
        }
        attempt += 1;
        if attempt < max_attempts {
            // Back off between attempts so rate-limited providers (NVIDIA's
            // free tier in particular) get a fair poll within the budget.
            std::thread::sleep(Duration::from_millis(cfg.llm.retry_delay_ms));
        }
    }
    Generated::Failed
}

/// Ground truth printed under the signature in the HOOK CONTRACT: Horizon's
/// own doc for the hook, the event struct's fields, and the Flow semantics —
/// everything the model needs to use the hook without inventing anything.
fn contract_notes_text(s: &HookSignature) -> String {
    let mut out = String::new();
    if !s.doc.is_empty() {
        out.push_str("Horizon docs for this hook: ");
        out.push_str(&s.doc);
        out.push('\n');
    }
    if !s.event.is_empty() {
        if s.event_fields.is_empty() {
            out.push_str(&format!(
                "Event struct `{}` has no fields — read nothing from `event`.\n",
                s.event
            ));
        } else {
            out.push_str(&format!(
                "Event struct `{}` (#[non_exhaustive]; read fields, never construct):\n{}\n",
                s.event, s.event_fields
            ));
        }
    }
    if s.returns_flow {
        out.push_str(
            "Return `Flow::Continue` normally; return `Flow::Cancel` to veto the game's default action and skip later mods.\n",
        );
    }
    out
}

/// Pseudo-IR of the Java methods a feature came from (class/method inventory
/// from the Phase 1 AST).
fn java_ir_for(feature: &Feature, decomposed: &DecomposedMod) -> String {
    let mut out = Vec::new();
    for source in &feature.sources {
        let (class, method) = match source.split_once("::") {
            Some((c, m)) => (c, Some(m)),
            None => (source.as_str(), None),
        };
        if let Some(c) = decomposed.classes.iter().find(|c| c.name == class) {
            match method {
                Some(m) => {
                    for me in &c.methods {
                        if me.name == m {
                            out.push(format!(
                                "class {} -> method {}{} (has_code: {})",
                                c.name, me.name, me.descriptor, me.has_code
                            ));
                        }
                    }
                }
                None => {
                    out.push(format!(
                        "class {} extends {} ({} methods, {} fields)",
                        c.name,
                        c.super_name.clone().unwrap_or_default(),
                        c.methods.len(),
                        c.fields.len()
                    ));
                }
            }
        } else {
            out.push(format!("reference {source} (class not in this jar)"));
        }
    }
    if out.is_empty() {
        out.push("(no Java IR captured for this feature)".to_string());
    }
    out.join("\n")
}

/// Budget for real upstream source text handed to the LLM per feature (the
/// request must stay inside the provider context window even for large
/// classes; multiple source classes share the budget in feature order).
const UPSTREAM_SOURCE_BUDGET: usize = 6_000;

/// Append the real upstream source of the feature's classes to the Java IR
/// when it was fetched. Classes without a fetched source keep the inventory
/// line only — nothing is invented.
fn upstream_source_context(
    java_ir: &str,
    feature: &Feature,
    sources: &sources::JavaSources,
) -> String {
    if sources.is_empty() {
        return java_ir.to_string();
    }
    let mut snippets = Vec::new();
    let mut budget = UPSTREAM_SOURCE_BUDGET;
    for source in &feature.sources {
        if budget == 0 {
            break;
        }
        let class = source.split("::").next().unwrap_or(source);
        let Some(code) = sources.snippet_for(class, budget) else {
            continue;
        };
        budget = budget.saturating_sub(code.chars().count());
        let attribution = sources
            .repo()
            .map(|r| format!(" (github.com/{r})"))
            .unwrap_or_default();
        snippets.push(format!(
            "// real upstream source of {class}{attribution}\n{code}"
        ));
    }
    if snippets.is_empty() {
        return java_ir.to_string();
    }
    format!(
        "{java_ir}\n\nreal upstream source code:\n{}",
        snippets.join("\n\n")
    )
}

fn load_template(name: &str) -> Option<String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var("AUTO_PORTER_TEMPLATES") {
        candidates.push(PathBuf::from(dir));
    }
    candidates.push(PathBuf::from("templates"));
    // walk up from cwd so sub-crates resolve the workspace templates dir
    if let Ok(mut cwd) = std::env::current_dir() {
        loop {
            candidates.push(cwd.join("templates"));
            if !cwd.pop() {
                break;
            }
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates"));

    for dir in candidates {
        let p = dir.join(name);
        if p.is_file() {
            return std::fs::read_to_string(&p).ok();
        }
    }
    eprintln!("warning: template {name} not found (set AUTO_PORTER_TEMPLATES)");
    None
}

fn struct_name(feature_id: &str) -> String {
    let tail = feature_id.rsplit(':').next().unwrap_or(feature_id);
    let mut out = String::from("Port");
    for part in tail.split(['-', '_']) {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

fn module_slug(feature_id: &str) -> String {
    let tail = feature_id.rsplit(':').next().unwrap_or(feature_id);
    let slug: String = tail
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if slug
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(true)
    {
        format!("f{slug}")
    } else {
        slug
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::result_large_err)]
fn assemble_bundle(
    dest: &Path,
    mod_id: &str,
    version: &str,
    horizon_rev: &str,
    mixins_rev: &str,
    hooks_used: &[HookRef],
    catalog: &HookCatalog,
    modules: &[(String, String)],
) -> Result<(), PortError> {
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::File::create(dest).map_err(|e| {
        PortError::new(
            Phase::PortCode,
            ErrorType::CompileError,
            format!("cannot write {}: {e}", dest.display()),
        )
    })?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // horizon-native.json (spec §0.6)
    let mut manifest = serde_json::json!({
        "schema": 1,
        "id": mod_id,
        "version": version,
        "game": "26.3",
        "horizon": horizon_rev,
        "mixins": mixins_rev,
    });
    if !hooks_used.is_empty() {
        manifest["hooks"] = serde_json::json!(catalog.native_hooks_api);
    }
    zip.start_file("horizon-native.json", options)
        .map_err(|e| {
            PortError::new(
                Phase::PortCode,
                ErrorType::CompileError,
                format!("zip: {e}"),
            )
        })?;
    std::io::Write::write_all(
        &mut zip,
        serde_json::to_string_pretty(&manifest).unwrap().as_bytes(),
    )
    .map_err(|e| {
        PortError::new(
            Phase::PortCode,
            ErrorType::CompileError,
            format!("zip: {e}"),
        )
    })?;

    // mixins/<id>/horizon-mixin.toml
    let mut mixin_toml = String::new();
    // mod_id comes from mod metadata (third-party text): escape it.
    mixin_toml.push_str(&format!("id = \"{}\"\n", shared::util::toml_escape(mod_id)));
    for hook in hooks_used {
        mixin_toml.push_str(&format!(
            "\n[[injection]]\nhook = \"{}\"\nhook_version = {}\nkind = \"HEAD\"\n",
            shared::util::toml_escape(&hook.name),
            hook.hook_version
        ));
    }
    zip.start_file(format!("mixins/{mod_id}/horizon-mixin.toml"), options)
        .map_err(|e| {
            PortError::new(
                Phase::PortCode,
                ErrorType::CompileError,
                format!("zip: {e}"),
            )
        })?;
    std::io::Write::write_all(&mut zip, mixin_toml.as_bytes()).map_err(|e| {
        PortError::new(
            Phase::PortCode,
            ErrorType::CompileError,
            format!("zip: {e}"),
        )
    })?;

    // mixins/<id>/src/lib.rs + modules
    let mut lib = String::from(
        "#![deny(warnings)]\n// Generated by auto-porter — implements the horizon_client::mod_api::Hooks trait.\n",
    );
    for (file_name, _) in modules {
        let module = file_name.trim_end_matches(".rs");
        lib.push_str(&format!("pub mod {module};\n"));
    }
    zip.start_file(format!("mixins/{mod_id}/src/lib.rs"), options)
        .map_err(|e| {
            PortError::new(
                Phase::PortCode,
                ErrorType::CompileError,
                format!("zip: {e}"),
            )
        })?;
    std::io::Write::write_all(&mut zip, lib.as_bytes()).map_err(|e| {
        PortError::new(
            Phase::PortCode,
            ErrorType::CompileError,
            format!("zip: {e}"),
        )
    })?;

    for (file_name, content) in modules {
        zip.start_file(format!("mixins/{mod_id}/src/{file_name}"), options)
            .map_err(|e| {
                PortError::new(
                    Phase::PortCode,
                    ErrorType::CompileError,
                    format!("zip: {e}"),
                )
            })?;
        std::io::Write::write_all(&mut zip, content.as_bytes()).map_err(|e| {
            PortError::new(
                Phase::PortCode,
                ErrorType::CompileError,
                format!("zip: {e}"),
            )
        })?;
    }

    zip.finish().map_err(|e| {
        PortError::new(
            Phase::PortCode,
            ErrorType::CompileError,
            format!("zip: {e}"),
        )
    })?;
    Ok(())
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}
