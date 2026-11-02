// Template: player_hurt_hook.rs
// Horizon hook: living_incoming_damage (contract: cancel). Maps
// LivingHurtEvent / LivingDamageEvent / LivingIncomingDamageEvent (newer
// NeoForge) and ServerLivingEntityEvents.ALLOW_DAMAGE on Fabric.
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(
        &mut self,
        entity: &horizon_client::mod_api::EntityRef,
        amount: &mut f64,
        canceled: &mut bool,
    ) {
        // Condition: {{CONDITION}}
        // {{ACTION_NOTE}}
        let _ = (entity, amount);
        *canceled = false; // {{TODO}}
    }
}
