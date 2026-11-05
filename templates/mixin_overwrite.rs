// Template: mixin_overwrite.rs
// Behaviour replacement: the hook whose event carries the target value
// (replace/override contract) writes the new value into the event fields.
// Use sparingly; listen/cancel hooks are preferred and keep the port
// reviewable. The method signature is rendered from the extracted
// hook-signature table.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
