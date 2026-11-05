// Template: mixin_head.rs
// Generic hook skeleton: one catalog trait method, dispatched by the game
// through `mod_api::mods()` — no [[injection]] tables on the trait path
// (docs/limitations.md §8). The method signature is rendered from the
// extracted hook-signature table at the pinned revision.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
