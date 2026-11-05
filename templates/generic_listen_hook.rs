// Template: generic_listen_hook.rs
// Contracts: listen / cancel for the remaining single-entity hooks
// (player_died, interact_entity, item_picked_up, item_dropped). The method
// signature is rendered from the extracted hook-signature table; cancellable
// hooks return `Flow`, so veto by returning `Flow::Cancel`, never by writing
// an out-parameter.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
