// Template: config_load.rs
// Config (ModConfigSpec / config libs) carried as a data file; wiring a live
// config listener needs a mod-publish `Host` helper (resolve in the pinned
// mod_api). Anchored on data_pack_files (contract: collect) meanwhile.
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(&mut self, files: &horizon_client::mod_api::DataFilesRef) {
        // {{ACTION_NOTE}}
        let _ = files; // {{TODO}}
    }
}
