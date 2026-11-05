// Template: config_load.rs
// Config (ModConfigSpec / config libs) carried as a data file; wiring a live
// config listener needs a mod-publish `Host` helper (resolve in the pinned
// mod_api). Anchored on data_pack_files (contract: collect) meanwhile.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
