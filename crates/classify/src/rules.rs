//! The hand-maintained Java-side column of the feature-mapping table (spec §3.1).
//!
//! The **Horizon column is a lookup key, not a promise**: every `hook` name is
//! resolved against the pinned `native-hooks.json` by the catalog (xtask fails
//! the build if a name is missing). This table only fixes the *Java* side.

use shared::types::{FeatureKind, Tier};

pub struct MappingRule {
    /// Java concept label, as in the §3.1 table.
    pub concept: &'static str,
    /// Regex applied to the event class internal name (suffix match on the
    /// simple name works for both NeoForge events and Fabric callback
    /// interfaces, e.g. `BlockEvent$BreakEvent` / `PlayerBlockBreakEvents$Before`).
    pub event_regex: &'static str,
    /// Horizon hook name to resolve in native-hooks.json.
    pub hook: &'static str,
    pub kind: FeatureKind,
    /// Tier when the hook resolves.
    pub tier: Tier,
    /// Tier when the hook does not resolve in the pinned catalog.
    pub tier_unmapped: Tier,
    /// Template file (templates/) when one covers the pattern.
    pub template: &'static str,
}

/// Ordered: more specific regexes first (`RightClickBlock` before
/// `UseBlockCallback`, which is also the Fabric block-place callback).
pub const RULES: &[MappingRule] = &[
    MappingRule {
        concept: "PlayerInteractEvent.LeftClickBlock | AttackBlockCallback (attack block)",
        event_regex: "(LeftClickBlock|AttackBlockCallback)$",
        hook: "left_click_block",
        kind: FeatureKind::BlockBreak,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        // The catalog resolves the hook, but no template covers attack-block
        // handling — this routes to the spec §10 LLM fallback.
        template: "",
    },
    MappingRule {
        concept: "BlockEvent.BreakEvent | PlayerBlockBreakEvents.BEFORE",
        event_regex: "(BreakEvent|PlayerBlockBreakEvents\\$Before)$",
        hook: "destroy_block",
        kind: FeatureKind::BlockBreak,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "block_break_hook.rs",
    },
    MappingRule {
        concept: "PlayerInteractEvent.RightClickBlock | UseBlockCallback (right-click) | Balm UseBlockEvent",
        event_regex: "(RightClickBlock|UseBlockEvent|UseBlockCallback)$",
        hook: "right_click_block",
        kind: FeatureKind::ItemUse,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "item_use_hook.rs",
    },
    MappingRule {
        concept: "PlayerInteractEvent.RightClickItem | UseItemCallback",
        event_regex: "(RightClickItem|UseItemCallback)$",
        hook: "right_click_item",
        kind: FeatureKind::ItemUse,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "item_use_hook.rs",
    },
    MappingRule {
        concept: "BlockEvent.EntityPlaceEvent | UseBlockCallback (place check)",
        event_regex: "(EntityPlaceEvent|UseBlockCallback)$",
        hook: "place_block",
        kind: FeatureKind::BlockPlace,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "block_place_hook.rs",
    },
    MappingRule {
        // NeoForge 1.21.1 splits the modifyable incoming-damage phase into
        // `LivingDamageEvent$Pre`; Balm and older NeoForge keep the plain
        // `LivingDamageEvent` name — both must map. `$Post` is the
        // after-damage notification and stays unmapped (no catalog hook).
        concept: "LivingHurtEvent / LivingDamageEvent(/$Pre) / LivingIncomingDamageEvent | ServerLivingEntityEvents.ALLOW_DAMAGE",
        event_regex: "(LivingHurtEvent|LivingIncomingDamageEvent|LivingDamageEvent(\\$Pre)?|ServerLivingEntityEvents\\$AllowDamage)$",
        hook: "living_incoming_damage",
        kind: FeatureKind::EntityHurt,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "player_hurt_hook.rs",
    },
    MappingRule {
        concept: "LivingDeathEvent | ServerLivingEntityEvents.AFTER_DEATH",
        event_regex: "(LivingDeathEvent|ServerLivingEntityEvents\\$AfterDeath)$",
        hook: "living_death",
        kind: FeatureKind::EntityDeath,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "generic_listen_hook.rs",
    },
    MappingRule {
        concept: "EntityJoinLevelEvent (was EntityJoinWorldEvent) | ServerEntityEvents.ENTITY_LOAD",
        event_regex: "(EntityJoinLevelEvent|EntityJoinWorldEvent|ServerEntityEvents\\$EntityLoad)$",
        hook: "entity_join_level",
        kind: FeatureKind::EntitySpawn,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "entity_spawn.rs",
    },
    MappingRule {
        concept: "PlayerInteractEvent.EntityInteract | UseEntityCallback",
        event_regex: "(EntityInteract|UseEntityCallback)$",
        hook: "entity_interact",
        kind: FeatureKind::EntityInteract,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "generic_listen_hook.rs",
    },
    MappingRule {
        concept: "ItemEntityPickupEvent / PlayerEvent.ItemPickupEvent",
        event_regex: "(ItemEntityPickupEvent|ItemPickupEvent)$",
        hook: "item_pickup",
        kind: FeatureKind::ItemPickup,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "generic_listen_hook.rs",
    },
    MappingRule {
        concept: "ItemTossEvent",
        event_regex: "(ItemTossEvent)$",
        hook: "item_drop",
        kind: FeatureKind::ItemDrop,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "generic_listen_hook.rs",
    },
    MappingRule {
        concept: "KeyMapping registration | KeyBindingHelper.registerKeyBinding",
        event_regex: "(RegisterKeyMappingsEvent)$",
        hook: "register_keybind",
        kind: FeatureKind::Keybind,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "keybind.rs",
    },
    MappingRule {
        concept: "GUI layer/overlay events | HudRenderCallback",
        event_regex: "(RegisterGuiLayersEvent|RenderGuiLayerEvent|RenderGuiOverlayEvent|HudRenderCallback|RenderLayerEvent)$",
        hook: "hud_render",
        kind: FeatureKind::Hud,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "hud_overlay.rs",
    },
    MappingRule {
        // TickEvent.PlayerTickEvent (NeoForge/Forge) and Fabric's
        // ServerTickEvents.StartPlayerTick: the pre-tick phase.
        // The seed catalog resolves the hook, but no template covers the
        // pattern — this is the spec §10 LLM-fallback path.
        concept: "PlayerTickEvent (start phase) | ServerTickEvents.START_PLAYER_TICK",
        event_regex: "(PlayerTickEvent|StartPlayerTick)$",
        hook: "player_tick_pre",
        kind: FeatureKind::Unknown,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "",
    },
    MappingRule {
        concept: "PlayerTickEvent (end phase) | ServerTickEvents.END_PLAYER_TICK",
        event_regex: "(EndPlayerTick)$",
        hook: "player_tick_post",
        kind: FeatureKind::Unknown,
        tier: Tier::B,
        tier_unmapped: Tier::D,
        template: "",
    },
];

/// Structural rules applied to class hierarchies / constant-pool refs.
pub const STRUCTURAL: &[StructuralRule] = &[
    StructuralRule {
        concept: "BlockEntity with logic (TileEntity in old Forge)",
        matcher: ClassMatch::SuperContains("BlockEntity"),
        hook: "block_entity_tick",
        kind: FeatureKind::BlockEntity,
        tier: Tier::C,
        template: "block_entity.rs",
    },
    StructuralRule {
        concept: "Custom payloads / packets | PayloadTypeRegistry + networking API",
        matcher: ClassMatch::OwnerContains("PayloadTypeRegistry|CustomPayload|fabric/api/networking"),
        hook: "net_from_client",
        kind: FeatureKind::Networking,
        tier: Tier::C,
        template: "net_channel.rs",
    },
    StructuralRule {
        concept: "Energy/fluid capabilities | Transfer API",
        matcher: ClassMatch::OwnerContains("neoforge/energy|minecraftforge/energy|neoforge/fluid|minecraftforge/fluid|FluidTank|EnergyStorage"),
        hook: "block_entity_tick",
        kind: FeatureKind::EnergyFluid,
        tier: Tier::C,
        template: "fluid_tank.rs",
    },
    StructuralRule {
        concept: "New entity/mob: entity registration + goals",
        matcher: ClassMatch::SuperContains("/world/entity/Mob|/world/entity/Entity"),
        hook: "entity_join_level",
        kind: FeatureKind::EntityBehavior,
        tier: Tier::C,
        template: "entity_behavior.rs",
    },
    StructuralRule {
        concept: "Screen / AbstractContainerMenu",
        matcher: ClassMatch::SuperContains("/client/gui/screens/Screen|/inventory/AbstractContainerMenu|/inventory/MenuType"),
        hook: "container_open",
        kind: FeatureKind::Gui,
        tier: Tier::C,
        template: "menu_screen.rs",
    },
    StructuralRule {
        concept: "Renderer/shader overhaul (RenderType, PostChain, ShaderInstance, GameRenderer overrides)",
        matcher: ClassMatch::SuperContains("PostChain|ShaderInstance|GameRenderer|RenderTarget"),
        hook: "hud_render",
        kind: FeatureKind::Renderer,
        tier: Tier::D,
        template: "hud_overlay.rs",
    },
];

pub enum ClassMatch {
    SuperContains(&'static str),
    OwnerContains(&'static str),
}

pub struct StructuralRule {
    pub concept: &'static str,
    pub matcher: ClassMatch,
    pub hook: &'static str,
    pub kind: FeatureKind,
    pub tier: Tier,
    pub template: &'static str,
}
