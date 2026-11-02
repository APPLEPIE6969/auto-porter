//! Phase 2: Classify — map the decomposed mod onto Horizon hooks and assign
//! portability tiers (spec §3).

pub mod rules;

use regex::Regex;
use shared::catalog::HookCatalog;
use shared::types::*;
use std::collections::BTreeMap;

pub struct ClassifyOutput {
    pub plan: PortPlan,
    pub gaps: Vec<GapReport>,
    /// Unresolved hook names hit while mapping (xtask reports these as build
    /// failures when the real catalog is pinned).
    pub unresolved_hooks: Vec<String>,
}

pub fn classify(decomposed: &DecomposedMod, catalog: &HookCatalog) -> ClassifyOutput {
    let mut features: Vec<Feature> = Vec::new();
    let mut gaps: Vec<GapReport> = Vec::new();
    let mut unresolved_hooks: Vec<String> = Vec::new();

    let mod_id = decomposed.metadata.id.clone();

    // ---- 1) event listeners -> mapping rules (spec §3.1)
    let compiled: Vec<(usize, Regex)> = rules::RULES
        .iter()
        .enumerate()
        .filter_map(|(i, r)| Regex::new(r.event_regex).ok().map(|re| (i, re)))
        .collect();

    for ev in &decomposed.events {
        let simple = ev.event_class.rsplit('/').next().unwrap_or(&ev.event_class);
        let hit = compiled
            .iter()
            .find(|(_i, re)| re.is_match(simple))
            .map(|(i, _)| *i);
        let (kind, concept, hook_name, tier, tier_unmapped, template) = match hit {
            Some(i) => {
                let r = &rules::RULES[i];
                (
                    r.kind,
                    r.concept.to_string(),
                    Some(r.hook.to_string()),
                    r.tier,
                    r.tier_unmapped,
                    Some(r.template.to_string()),
                )
            }
            None => (
                FeatureKind::Unknown,
                format!("event listener on {}", ev.event_class),
                None,
                Tier::D,
                Tier::D,
                None,
            ),
        };

        let mut notes = Vec::new();
        let mut hook = None;
        let mut contract = None;
        let mut tier_final = tier;
        if let Some(hook_name) = &hook_name {
            match catalog.resolve(hook_name) {
                Ok(def) => {
                    hook = Some(HookRef {
                        name: def.name.clone(),
                        hook_version: def.hook_version,
                    });
                    contract = Some(def.contract.clone());
                    if def.confirmed == Some(false) {
                        notes.push(format!(
                            "hook `{}` is a proposed name (not yet confirmed against the pinned native-hooks.json)",
                            def.name
                        ));
                    }
                }
                Err(_) => {
                    unresolved_hooks.push(hook_name.clone());
                    tier_final = tier_unmapped;
                    notes.push(format!(
                        "hook `{hook_name}` missing from the pinned catalog; feature demoted"
                    ));
                    gaps.push(GapReport {
                        feature_id: String::new(), // filled below
                        java_concept: concept.clone(),
                        detail: format!(
                            "Horizon hook `{hook_name}` not present in native-hooks.json"
                        ),
                        horizon_issue: Some("open one".to_string()),
                    });
                }
            }
        }

        features.push(
            Feature {
                id: String::new(),
                kind,
                title: format!(
                    "{} -> {}",
                    simple,
                    hook.as_ref()
                        .map(|h| h.name.as_str())
                        .unwrap_or("(no hook)")
                ),
                tier: tier_final,
                sources: vec![format!(
                    "{}{}",
                    ev.handler_class,
                    ev.handler_method
                        .as_ref()
                        .map(|m| format!("::{m}"))
                        .unwrap_or_default()
                )],
                java_concept: Some(concept),
                hook,
                contract,
                template,
                notes,
                parity_tests: Vec::new(),
                data_files: Vec::new(),
            }
            .with_id(
                &mod_id,
                "event",
                &ev.event_class,
                ev.handler_method.as_deref(),
            ),
        );
    }

    // ---- 2) structural classes (block entities, entities, gui, renderers, networking...)
    for class in &decomposed.classes {
        if let Some(rule) = rules::STRUCTURAL.iter().find(|r| match &r.matcher {
            rules::ClassMatch::SuperContains(pat) => class
                .super_name
                .as_deref()
                .map(|s| glob_like_contains(s, pat))
                .unwrap_or(false),
            rules::ClassMatch::OwnerContains(_) => false, // handled below
        }) {
            push_structural(
                &mut features,
                &mut gaps,
                &mut unresolved_hooks,
                catalog,
                &mod_id,
                rule,
                std::slice::from_ref(&class.name),
            );
        }
    }
    // owner-contains rules need the constants, which live in the raw classes —
    // approximate from the class ASTs' superclass plus class names here, and
    // use the mixin/event sources for networking/energy detection.
    let structural_names: Vec<String> = decomposed.classes.iter().map(|c| c.name.clone()).collect();
    for class_name in &structural_names {
        for rule in rules::STRUCTURAL {
            if let rules::ClassMatch::OwnerContains(pat) = rule.matcher {
                if glob_like_contains(class_name, pat) {
                    push_structural(
                        &mut features,
                        &mut gaps,
                        &mut unresolved_hooks,
                        catalog,
                        &mod_id,
                        rule,
                        std::slice::from_ref(class_name),
                    );
                }
            }
        }
    }

    // ---- 3) generic mixins (injections not already covered by an event rule)
    for mixin in &decomposed.mixins {
        for inj in &mixin.injections {
            if matches!(
                inj.kind.as_str(),
                "shadow" | "unique" | "accessor" | "invoker"
            ) {
                continue; // plumbing, not behaviour
            }
            let (template, tier) = if inj.kind == "overwrite" {
                ("mixin_overwrite.rs", Tier::B)
            } else {
                ("mixin_head.rs", Tier::B)
            };
            features.push(
                Feature {
                    id: String::new(),
                    kind: FeatureKind::GenericMixin,
                    title: format!(
                        "mixin {} @ {}",
                        inj.kind,
                        inj.selector.clone().unwrap_or_default()
                    ),
                    tier,
                    sources: vec![format!("{}::{}", mixin.class, inj.method)],
                    java_concept: Some("Generic Mixin | Mixin".to_string()),
                    hook: None,
                    contract: None,
                    template: Some(template.to_string()),
                    notes: vec![format!(
                        "carried as a horizon-mixins {} injection on {} ({})",
                        if inj.kind == "overwrite" {
                            "overwrite"
                        } else {
                            "HEAD"
                        },
                        mixin.target_class.clone().unwrap_or_default(),
                        inj.at.clone().unwrap_or_else(|| "HEAD".to_string())
                    )],
                    parity_tests: Vec::new(),
                    data_files: Vec::new(),
                }
                .with_id(&mod_id, "mixin", &mixin.class, Some(&inj.method)),
            );
        }
    }

    // ---- 4) config feature (simple: data file + a mod-publish Host helper)
    if let Some(cfg) = &decomposed.config {
        features.push(
            Feature {
                id: String::new(),
                kind: FeatureKind::Config,
                title: format!("config file {}", cfg.path),
                tier: Tier::B,
                sources: vec![cfg.path.clone()],
                java_concept: Some("Config (ModConfigSpec) | config libs".to_string()),
                hook: None,
                contract: None,
                template: Some("config_load.rs".to_string()),
                notes: vec![
                    "carried as a data file; wiring needs a mod-publish `Host` helper once available".to_string(),
                ],
                parity_tests: Vec::new(),
                data_files: vec![cfg.path.clone()],
            }
            .with_id(&mod_id, "config", &cfg.path, None),
        );
    }

    // ---- 5) data-only feature (Tier A, fully automatic via horizon-port-pack)
    if !decomposed.data_files.is_empty() {
        let mut categories = std::collections::BTreeSet::new();
        for f in &decomposed.data_files {
            let cat = f.path.split('/').nth(1).unwrap_or("other").to_string();
            categories.insert(cat);
        }
        let data_paths: Vec<String> = decomposed
            .data_files
            .iter()
            .map(|f| f.path.clone())
            .collect();
        features.push(
            Feature {
                id: String::new(),
                kind: FeatureKind::DataOnly,
                title: format!(
                    "content data: {}",
                    categories.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
                tier: Tier::A,
                sources: Vec::new(),
                java_concept: Some(
                    "Loot tables, worldgen JSON, recipes | data JSON -> data carry".to_string(),
                ),
                hook: None,
                contract: None,
                template: None,
                notes: vec![
                    "carried by horizon-port-pack (or the offline fallback carrier)".to_string(),
                ],
                parity_tests: Vec::new(),
                data_files: data_paths,
            }
            .with_id(&mod_id, "data", "content", None),
        );
    }

    // ---- parity test stubs for automatable tiers
    for f in features.iter_mut() {
        if matches!(f.tier, Tier::B | Tier::C) && !f.id.is_empty() {
            f.parity_tests.push(parity_stub(&mod_id, f));
        }
    }

    // ---- dedupe features by id (structural scan can double-hit)
    features.sort_by(|a, b| a.id.cmp(&b.id));
    features.dedup_by(|a, b| a.id == b.id);

    // ---- tier summary
    let mut tier_summary: BTreeMap<String, u32> = BTreeMap::new();
    for f in &features {
        *tier_summary.entry(f.tier.as_str().to_string()).or_insert(0) += 1;
    }

    let data_files: Vec<String> = decomposed
        .data_files
        .iter()
        .map(|f| f.path.clone())
        .collect();

    ClassifyOutput {
        plan: PortPlan {
            mod_id,
            features,
            data_files,
            tier_summary,
        },
        gaps,
        unresolved_hooks,
    }
}

fn push_structural(
    features: &mut Vec<Feature>,
    gaps: &mut Vec<GapReport>,
    unresolved: &mut Vec<String>,
    catalog: &HookCatalog,
    mod_id: &str,
    rule: &rules::StructuralRule,
    sources: &[String],
) {
    let mut notes = Vec::new();
    let mut hook = None;
    let mut contract = None;
    let tier = rule.tier;
    match catalog.resolve(rule.hook) {
        Ok(def) => {
            hook = Some(HookRef {
                name: def.name.clone(),
                hook_version: def.hook_version,
            });
            contract = Some(def.contract.clone());
            if def.confirmed == Some(false) {
                notes.push(format!(
                    "hook `{}` is a proposed name (re-check against the pinned native-hooks.json)",
                    def.name
                ));
            }
        }
        Err(_) => {
            unresolved.push(rule.hook.to_string());
            notes.push(format!(
                "hook `{}` missing from the pinned catalog; feature demoted to report-only",
                rule.hook
            ));
            gaps.push(GapReport {
                feature_id: String::new(),
                java_concept: rule.concept.to_string(),
                detail: format!(
                    "Horizon hook `{}` not present in native-hooks.json",
                    rule.hook
                ),
                horizon_issue: Some("open one".to_string()),
            });
        }
    }
    if tier == Tier::C {
        notes.push("Tier C: partly automatic, human design required".to_string());
    }
    if tier == Tier::D {
        notes.push("Tier D: report-only, no Horizon equivalent".to_string());
    }
    features.push(
        Feature {
            id: String::new(),
            kind: rule.kind,
            title: rule.concept.to_string(),
            tier,
            sources: sources.to_vec(),
            java_concept: Some(rule.concept.to_string()),
            hook,
            contract,
            template: Some(rule.template.to_string()),
            notes,
            parity_tests: Vec::new(),
            data_files: Vec::new(),
        }
        .with_id(
            mod_id,
            "struct",
            rule.concept,
            sources.first().map(|s| s.as_str()),
        ),
    );
}

trait WithId: Sized {
    fn with_id(self, mod_id: &str, group: &str, key: &str, sub: Option<&str>) -> Self;
}

impl WithId for Feature {
    fn with_id(mut self, mod_id: &str, group: &str, key: &str, sub: Option<&str>) -> Self {
        fn slug(s: &str) -> String {
            s.rsplit('/')
                .next()
                .unwrap_or(s)
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect::<String>()
                .trim_matches('-')
                .to_ascii_lowercase()
        }
        let mut id = format!("{group}-{}", slug(key));
        if let Some(sub) = sub {
            let s = slug(sub);
            if !s.is_empty() {
                id.push_str(&format!("-{s}"));
            }
        }
        // avoid pathological ids
        if id.len() > 80 {
            id.truncate(80);
        }
        self.id = format!("{mod_id}:{id}");
        self
    }
}

fn parity_stub(mod_id: &str, feature: &Feature) -> ParityTestSpec {
    let name = format!(
        "{}_{}_parity",
        mod_id,
        feature
            .id
            .split(':')
            .nth(1)
            .unwrap_or("feature")
            .replace('-', "_")
    );
    let scenario_yaml = format!(
        "# auto-generated from the PortPlan (spec §6.2); survival mode, real drops\n\
         name: {name}\n\
         description: \"Parity scenario for {} ({}): {}\"\n\
         setup:\n  \
         world: flat\n  \
         seed: 42\n  \
         player: survival\n\
         actions: []            # filled by the Phase 5 scenario generator / LLM\n\
         expected:\n  \
         broken_blocks: []\n",
        feature.kind.as_str(),
        feature.tier.as_str(),
        feature.title.replace('"', "'")
    );
    ParityTestSpec {
        name,
        scenario_yaml,
    }
}

/// `a|b` alternation contains-check without full regex dependency here.
fn glob_like_contains(haystack: &str, pattern: &str) -> bool {
    pattern.split('|').any(|p| haystack.contains(p))
}
