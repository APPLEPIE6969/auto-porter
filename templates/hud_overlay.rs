// Template: hud_overlay.rs
// Horizon hook: hud_render (client, contract: listen). Maps
// RegisterGuiLayersEvent / RenderGuiLayerEvent (older: RenderGuiOverlayEvent)
// and HudRenderCallback.
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(&mut self, ctx: &horizon_client::mod_api::GuiCtx) {
        // {{ACTION_NOTE}}
        let _ = ctx; // {{TODO}}
    }
}
