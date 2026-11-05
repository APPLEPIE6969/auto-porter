// Template: hud_overlay.rs
// Horizon hook: gui_layer_post (client, contract: listen). Maps
// RegisterGuiLayersEvent / RenderGuiLayerEvent (older: RenderGuiOverlayEvent)
// and HudRenderCallback.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
