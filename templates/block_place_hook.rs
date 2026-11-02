// Template: block_place_hook.rs
// Horizon hook: place_block (contract: cancel). Maps BlockEvent.EntityPlaceEvent /
// UseBlockCallback (place check). Signature reconciled against the pinned
// native-hooks.json; the launcher-style build is the compile gate.
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(
        &mut self,
        block: &horizon_client::mod_api::BlockRef,
        canceled: &mut bool,
    ) {
        // Condition: {{CONDITION}}
        // {{ACTION_NOTE}}
        let _ = block;
        *canceled = false; // {{TODO}}
    }
}
