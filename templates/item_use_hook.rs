// Template: item_use_hook.rs
// Horizon hooks: right_click_block / use_item (contract: cancel).
// Maps PlayerInteractEvent.RightClickBlock/RightClickItem, UseBlockCallback,
// UseItemCallback. The replacement value, when the hook carries one, lives in
// the event struct — never in an out-parameter.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
