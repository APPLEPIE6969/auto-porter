// Template: entity_spawn.rs
// Horizon hook: entity_join_level (contract: cancel). Maps
// EntityJoinLevelEvent (was EntityJoinWorldEvent) /
// ServerEntityEvents.ENTITY_LOAD.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
