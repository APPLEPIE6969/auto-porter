// Template: block_entity.rs
// Horizon hook: block_entity_tick (contract: listen). Maps BlockEntity with a
// ticker (TileEntity in old Forge). Container access via block_entity_container.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
