// Template: item_use_hook.rs
// Horizon hooks: right_click_block / use_item (contract: cancel).
// Maps PlayerInteractEvent.RightClickBlock/RightClickItem, UseBlockCallback,
// UseItemCallback.
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(
        &mut self,
        item: &horizon_client::mod_api::ItemRef,
        result: &mut Option<horizon_client::mod_api::InteractionResult>,
    ) {
        // Condition: {{CONDITION}}
        // {{ACTION_NOTE}}
        let _ = item;
        *result = None; // {{TODO}}
    }
}
