// Template: block_entity.rs
// Horizon hook: block_entity_tick (contract: listen). Maps BlockEntity with a
// ticker (TileEntity in old Forge). Container access via block_entity_container.
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(
        &mut self,
        be: &horizon_client::mod_api::BlockEntityRef,
        host: &mut dyn horizon_client::Host,
    ) {
        // {{ACTION_NOTE}}
        let _ = (be, host); // {{TODO}}
    }
}
