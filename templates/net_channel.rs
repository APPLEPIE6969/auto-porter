// Template: net_channel.rs
// Horizon hooks: net_from_client / net_from_server. Maps custom payloads /
// packets (PayloadTypeRegistry + Fabric networking API). Sends go through the
// Host send helpers.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
