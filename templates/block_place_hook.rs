// Template: block_place_hook.rs
// Horizon hook: place_block (contract: listen). Maps BlockEvent.EntityPlaceEvent /
// UseBlockCallback (place check). The method signature is rendered from the
// extracted hook-signature table at the pinned revision.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
