// Template: net_channel.rs
// Horizon hooks: net_from_client / net_from_server. Maps custom payloads /
// packets (PayloadTypeRegistry + Fabric networking API). Sends go through the
// Host send helpers.
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(&mut self, payload: &horizon_client::mod_api::PayloadRef) {
        // {{ACTION_NOTE}}
        let _ = payload; // {{TODO}}
    }
}
