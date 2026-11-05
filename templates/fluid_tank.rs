// Template: fluid_tank.rs
// Energy/fluid capabilities (Forge/NeoForge capabilities or the Fabric
// Transfer API) carried as block-entity logic; fluid/energy reads and writes
// go through the `Host` helpers (read_energy / write_energy / read_fluid /
// write_fluid), anchored on block_entity_tick.
#[derive(Default)]
pub struct {{STRUCT_NAME}};

impl horizon_client::mod_api::Hooks for {{STRUCT_NAME}} {
{{HOOK_METHOD_BLOCK}}
}
