// Template: entity_spawn.rs
// Horizon hook: entity_join_level (contract: cancel). Maps
// EntityJoinLevelEvent (was EntityJoinWorldEvent) /
// ServerEntityEvents.ENTITY_LOAD.
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(&mut self, entity: &horizon_client::mod_api::EntityRef) {
        // Condition: {{CONDITION}}
        // {{ACTION_NOTE}}
        let _ = entity; // {{TODO}}
    }
}
