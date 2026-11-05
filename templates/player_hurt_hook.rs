// Template: player_hurt_hook.rs
// Horizon hook: living_incoming_damage (contract: cancel). Maps
// LivingHurtEvent / LivingDamageEvent / LivingIncomingDamageEvent (newer
// NeoForge) and ServerLivingEntityEvents.ALLOW_DAMAGE on Fabric. Veto by
// returning `Flow::Cancel` — never by writing an out-parameter.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
