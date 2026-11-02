// Template: item_use_hook.rs
// Horizon hooks: right_click_block / right_click_item (contract: replace).
// Maps PlayerInteractEvent.RightClickBlock/RightClickItem, UseBlockCallback,
// UseItemCallback. `use_item_on` exists on the Horizon side (issue #5638).
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
