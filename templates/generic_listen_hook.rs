// Template: generic_listen_hook.rs
// Contracts: listen / cancel for the remaining single-entity hooks
// (player_died, interact_entity, item_picked_up, item_dropped).
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(
        &mut self,
        entity: &horizon_client::mod_api::EntityRef,
        canceled: &mut bool,
    ) {
        // Contract: {{CONTRACT}}. Condition: {{CONDITION}}
        // {{ACTION_NOTE}}
        let _ = entity;
        *canceled = false; // {{TODO}}
    }
}
