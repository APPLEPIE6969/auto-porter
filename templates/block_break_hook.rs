// Template: block_break_hook.rs
// Horizon hook: destroy_block (contract: cancel). Maps BlockEvent.BreakEvent /
// PlayerBlockBreakEvents.BEFORE. The method signature is rendered from the
// extracted hook-signature table at the pinned revision; the launcher-style
// build (spec §5.2) remains the compile gate.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
