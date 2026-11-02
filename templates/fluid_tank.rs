// Template: fluid_tank.rs
// Energy/fluid capabilities (Forge/NeoForge capabilities or the Fabric
// Transfer API) carried as block-entity logic; fluid/energy reads and writes
// go through the `Host` helpers (read_energy / write_energy / read_fluid /
// write_fluid), anchored on block_entity_tick.
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
    /// {{FEATURE_TITLE}} — ported from {{JAVA_SOURCE}}.
    fn {{HOOK_NAME}}(
        &mut self,
        be: &horizon_client::mod_api::BlockEntityRef,
        host: &mut dyn horizon_client::Host,
    ) {
        // {{ACTION_NOTE}}
        let _ = (be, host); // {{TODO}}
    }
}
