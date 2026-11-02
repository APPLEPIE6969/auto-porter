// Template: menu_screen.rs
// Screen / AbstractContainerMenu ports: menu slot rules plus the renderer
// hooks; anchored on player_container_open (resolved in the pinned
// native-hooks.json).
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(&mut self, menu: &horizon_client::mod_api::MenuRef) {
        // {{ACTION_NOTE}}
        let _ = menu; // {{TODO}}
    }
}
