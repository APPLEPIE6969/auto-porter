// Template: mixin_overwrite.rs
// Generic Mixin @Overwrite: replaces the target method body through the
// horizon-mixins apply step. Use sparingly; HEAD/replace hooks are preferred
// and keep the port reviewable.
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(&mut self) {
        // {{ACTION_NOTE}}
        // {{TODO}}
    }
}
