//! Typed artifacts passed between the five phases (spec §2/§3).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which mod loader the JAR was written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    Fabric,
    NeoForge,
    Forge,
    Unknown,
}

impl std::fmt::Display for Loader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Loader::Fabric => "fabric",
            Loader::NeoForge => "neoforge",
            Loader::Forge => "forge",
            Loader::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModMetadata {
    pub id: String,
    pub name: String,
    pub version: String,
    pub loader: Loader,
    pub minecraft_range: Option<String>,
    pub entrypoints: Vec<String>,
    /// mixin config JSONs referenced by the mod (fabric `mixins` key or `MixinConfigs` manifest attr).
    pub mixin_configs: Vec<String>,
    pub description: Option<String>,
    pub authors: Vec<String>,
}

/// One parsed Java class file (class-level AST, spec §2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassAst {
    /// Internal name, e.g. `com/example/veinminer/VeinMiner`.
    pub name: String,
    pub super_name: Option<String>,
    pub interfaces: Vec<String>,
    pub access: u16,
    pub fields: Vec<FieldAst>,
    pub methods: Vec<MethodAst>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldAst {
    pub name: String,
    pub descriptor: String,
    pub access: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodAst {
    pub name: String,
    pub descriptor: String,
    pub access: u16,
    pub has_code: bool,
}

/// One Mixin class with its injections (spec §2.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MixinAst {
    pub class: String,
    pub target_class: Option<String>,
    pub injections: Vec<InjectionAst>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionAst {
    /// Mixin annotation kind: inject, redirect, modify_arg, modify_args, modify_variable,
    /// modify_constant, overwrite, accessor, invoker, shadow, unique.
    pub kind: String,
    pub method: String,
    /// Target method selector string from the annotation, when present.
    pub selector: Option<String>,
    /// `@At` value (HEAD, TAIL, RETURN, INVOKE, ...).
    pub at: Option<String>,
    pub cancellable: Option<bool>,
}

/// One event listener (spec §2.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventListenerAst {
    pub loader: Loader,
    /// Event class name, e.g. `net/minecraftforge/event/entity/living/LivingHurtEvent`
    /// or the Fabric callback interface `net/fabricmc/.../PlayerBlockBreakEvents$Before`.
    pub event_class: String,
    pub handler_class: String,
    pub handler_method: Option<String>,
    /// "annotation" (@SubscribeEvent) or "callback" (XCallback.EVENT.register).
    pub registration: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    /// asset | data | pack_meta | config
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSchema {
    pub path: String,
    /// toml | json
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub id: String,
    pub version_req: Option<String>,
    /// required | optional
    pub kind: String,
}

/// Phase 1 output (spec §2.7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecomposedMod {
    pub metadata: ModMetadata,
    pub classes: Vec<ClassAst>,
    pub mixins: Vec<MixinAst>,
    pub events: Vec<EventListenerAst>,
    pub data_files: Vec<DataFile>,
    pub config: Option<ConfigSchema>,
    pub dependencies: Vec<Dependency>,
    /// Structural counters used by the §2 quality gates.
    pub stats: DecomposeStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DecomposeStats {
    pub jar_entries: u32,
    pub class_entries: u32,
    pub parsed_classes: u32,
    pub data_files_in_jar: u32,
    pub mixin_injections: u32,
    pub event_listeners: u32,
    pub gate_violations: Vec<GateViolation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateViolation {
    pub gate: String,
    pub message: String,
}

/// Portability tier (spec §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tier {
    A,
    B,
    C,
    D,
}

impl Tier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::A => "A",
            Tier::B => "B",
            Tier::C => "C",
            Tier::D => "D",
        }
    }
}

/// What kind of behaviour a feature represents (drives hook mapping and templates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureKind {
    DataOnly,
    BlockBreak,
    BlockPlace,
    ItemUse,
    EntityHurt,
    EntityDeath,
    EntityInteract,
    EntitySpawn,
    ItemPickup,
    ItemDrop,
    Keybind,
    Hud,
    BlockEntity,
    Networking,
    Recipe,
    EntityBehavior,
    Gui,
    EnergyFluid,
    Renderer,
    Config,
    GenericMixin,
    Unknown,
}

impl FeatureKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            FeatureKind::DataOnly => "data_only",
            FeatureKind::BlockBreak => "block_break",
            FeatureKind::BlockPlace => "block_place",
            FeatureKind::ItemUse => "item_use",
            FeatureKind::EntityHurt => "entity_hurt",
            FeatureKind::EntityDeath => "entity_death",
            FeatureKind::EntityInteract => "entity_interact",
            FeatureKind::EntitySpawn => "entity_spawn",
            FeatureKind::ItemPickup => "item_pickup",
            FeatureKind::ItemDrop => "item_drop",
            FeatureKind::Keybind => "keybind",
            FeatureKind::Hud => "hud",
            FeatureKind::BlockEntity => "block_entity",
            FeatureKind::Networking => "networking",
            FeatureKind::Recipe => "recipe",
            FeatureKind::EntityBehavior => "entity_behavior",
            FeatureKind::Gui => "gui",
            FeatureKind::EnergyFluid => "energy_fluid",
            FeatureKind::Renderer => "renderer",
            FeatureKind::Config => "config",
            FeatureKind::GenericMixin => "generic_mixin",
            FeatureKind::Unknown => "unknown",
        }
    }
}

/// Hook reference: name + hook_version resolved from native-hooks.json (spec §5.2/§3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRef {
    pub name: String,
    pub hook_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParityTestSpec {
    pub name: String,
    /// Scenario YAML body, written to <mod-id>-tests/parity/ on port.
    pub scenario_yaml: String,
}

/// One portable feature with its classification outcome (spec §3.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feature {
    pub id: String,
    pub kind: FeatureKind,
    pub title: String,
    pub tier: Tier,
    /// Java source references (class[/method]) this feature came from.
    pub sources: Vec<String>,
    pub java_concept: Option<String>,
    pub hook: Option<HookRef>,
    pub contract: Option<String>,
    /// Template file name in templates/, when one covers this feature.
    pub template: Option<String>,
    pub notes: Vec<String>,
    pub parity_tests: Vec<ParityTestSpec>,
    pub data_files: Vec<String>,
}

/// Phase 2 output (spec §3.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortPlan {
    pub mod_id: String,
    pub features: Vec<Feature>,
    pub data_files: Vec<String>,
    pub tier_summary: BTreeMap<String, u32>,
}

/// A reported gap: a Java concept with no Horizon equivalent (spec §3.1 end).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GapReport {
    pub feature_id: String,
    pub java_concept: String,
    pub detail: String,
    pub horizon_issue: Option<String>,
}
