//! Mixin extraction (spec §2.3): `@Mixin` classes and their member annotations.

use crate::classfile::{AnnValue, Annotation, RawClass};
use shared::types::{InjectionAst, MixinAst};

const MIXIN_PKG: &str = "org/spongepowered/asm/mixin";

/// annotation internal name -> kind string used in the AST and reports.
fn kind_for(type_name: &str) -> Option<&'static str> {
    Some(match type_name {
        "org/spongepowered/asm/mixin/injection/Inject" => "inject",
        "org/spongepowered/asm/mixin/injection/Redirect" => "redirect",
        "org/spongepowered/asm/mixin/injection/ModifyArg" => "modify_arg",
        "org/spongepowered/asm/mixin/injection/ModifyArgs" => "modify_args",
        "org/spongepowered/asm/mixin/injection/ModifyVariable" => "modify_variable",
        "org/spongepowered/asm/mixin/injection/ModifyConstant" => "modify_constant",
        "org/spongepowered/asm/mixin/Overwrite" => "overwrite",
        "org/spongepowered/asm/mixin/Shadow" => "shadow",
        "org/spongepowered/asm/mixin/Unique" => "unique",
        "org/spongepowered/asm/mixin/Accessor" => "accessor",
        "org/spongepowered/asm/mixin/Invoker" => "invoker",
        _ => return None,
    })
}

/// `@At` value: usually the enum `At.Value.HEAD`, sometimes a plain string.
fn at_value(at: &Annotation) -> Option<String> {
    match at.value("value") {
        Some(AnnValue::Enum { const_name, .. }) => Some(const_name.clone()),
        Some(AnnValue::Prim(crate::classfile::Primitive::Utf8(s))) => Some(s.clone()),
        _ => None,
    }
}

fn selector_from(ann: &Annotation) -> Option<String> {
    match ann.value("method") {
        Some(AnnValue::Prim(crate::classfile::Primitive::Utf8(s))) => Some(s.clone()),
        Some(AnnValue::Array(vals)) => vals.first().map(|v| match v {
            AnnValue::Prim(crate::classfile::Primitive::Utf8(s)) => s.clone(),
            _ => String::new(),
        }),
        _ => None,
    }
}

pub fn extract_mixins(classes: &[RawClass]) -> Vec<MixinAst> {
    let mut out = Vec::new();
    for class in classes {
        let mixin_ann = class
            .class_annotations
            .iter()
            .find(|a| a.type_name == format!("{MIXIN_PKG}/Mixin"));
        let Some(mixin_ann) = mixin_ann else { continue };

        // Targets: `value` = Class[] or Class, `targets` = String[] or String.
        let mut target: Option<String> = None;
        let class_targets = mixin_ann.class_values("value");
        if let Some(first) = class_targets.first() {
            target = Some(first.clone());
        }
        if target.is_none() {
            if let Some(AnnValue::Array(vals)) = mixin_ann.value("targets") {
                target = vals.first().map(|v| match v {
                    AnnValue::Prim(crate::classfile::Primitive::Utf8(s)) => s.replace('.', "/"),
                    _ => String::new(),
                });
            } else if let Some(AnnValue::Prim(crate::classfile::Primitive::Utf8(s))) =
                mixin_ann.value("targets")
            {
                target = Some(s.replace('.', "/"));
            }
        }

        let mut injections = Vec::new();
        for method in &class.methods {
            for ann in &method.annotations {
                let Some(kind) = kind_for(&ann.type_name) else {
                    continue;
                };
                // Resolve a nested `@At` annotation on injection members.
                let at = ann.value("at").and_then(|v| match v {
                    AnnValue::Nested(n) => at_value(n),
                    _ => None,
                });
                injections.push(InjectionAst {
                    kind: kind.to_string(),
                    method: method.name.clone(),
                    selector: selector_from(ann),
                    at,
                    cancellable: ann.bool_value("cancellable"),
                });
            }
        }
        // De-duplicate identical rows (e.g. repeated @Unique on synthetic members).
        injections.sort_by(|a, b| {
            (&a.kind, &a.method, &a.selector).cmp(&(&b.kind, &b.method, &b.selector))
        });
        injections
            .dedup_by(|a, b| a.kind == b.kind && a.method == b.method && a.selector == b.selector);

        out.push(MixinAst {
            class: class.this_class.clone(),
            target_class: target,
            injections,
        });
    }
    out.sort_by(|a, b| a.class.cmp(&b.class));
    out
}
