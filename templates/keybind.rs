// Template: keybind.rs
// Horizon hook: key (client). Maps KeyMapping registration /
// KeyBindingHelper.registerKeyBinding. Fires through the keybind press hook.
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(&mut self, key: &horizon_client::mod_api::KeyRef) {
        // {{ACTION_NOTE}}
        let _ = key; // {{TODO}}
    }
}
