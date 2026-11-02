// Template: block_break_hook.rs
// Horizon hook: destroy_block (contract: cancel). Maps BlockEvent.BreakEvent /
// PlayerBlockBreakEvents.BEFORE. NOTE: the exact generated trait signature is
// reconciled against native-hooks.json at the pinned revision; the
// launcher-style build (spec §5.2) is the compile gate that enforces it.
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
