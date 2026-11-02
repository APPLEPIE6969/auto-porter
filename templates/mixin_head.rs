// Template: mixin_head.rs
// Generic Mixin carried as a horizon-mixins injection (kind = "HEAD"; trait
// injections are limited to HEAD per PR #3964). The [[injection]] entry in
// horizon-mixin.toml is emitted by the generator; the method below implements
// the hook the injection binds to.
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(&mut self) {
        // {{ACTION_NOTE}}
        // {{TODO}}
    }
}
