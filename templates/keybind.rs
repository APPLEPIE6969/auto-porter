// Template: keybind.rs
// Horizon hook: key (client). Maps KeyMapping registration /
// KeyBindingHelper.registerKeyBinding. Fires through the keybind press hook.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
