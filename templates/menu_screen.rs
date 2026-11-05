// Template: menu_screen.rs
// Screen / AbstractContainerMenu ports: menu slot rules plus the renderer
// hooks; anchored on player_container_open (resolved in the pinned
// native-hooks.json).
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
