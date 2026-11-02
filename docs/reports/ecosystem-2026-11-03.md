# Ecosystem report — 2026-11-03 (first sweep)

Pilot run of the nightly-ecosystem pipeline (spec §8.2), executed manually:

- **Source**: Modrinth top-20 search, `project_type:mod`, game version 26.3 (public API, no key)
- **Carrier**: `built-in-fallback` (horizon-port-pack is not installed in the execution environment; CI's data-port-tests covers the real carrier)
- **Verification**: static layer only (parity / in-game need a licensed copy + runner)
- Jars are third-party works: kept in scratch space, never committed or redistributed. This report carries report metadata only.



Mods processed: **20**

| mod | loader | overall | tiers | errors |
|---|---|---|---|---|
| appleskin | fabric | PASS_WITH_WARNINGS | A:1, B:11, C:1 | 2 |
| architectury | neoforge | PASS_WITH_WARNINGS | A:1, B:26, C:3, D:117 | 2 |
| cloth_config | neoforge | PASS_WITH_WARNINGS | A:1, C:1 | 1 |
| entity_model_features | neoforge | PASS_WITH_WARNINGS | A:1, B:66 | 8 |
| entity_texture_features | neoforge | PASS_WITH_WARNINGS | A:1, B:53, C:7 | 2 |
| entityculling | neoforge | PASS_WITH_WARNINGS | A:1, B:5 | 1 |
| fabric-api | fabric | PASS | A:1 | 0 |
| fabric-language-kotlin | fabric | PASS | A:1 | 0 |
| ferritecore | fabric | PARTIAL | B:15 | 0 |
| immediatelyfast | neoforge | PASS_WITH_WARNINGS | A:1, B:29, D:1 | 2 |
| iris | fabric | PASS_WITH_WARNINGS | A:1, B:220, C:6 | 29 |
| lithium | neoforge | PASS_WITH_WARNINGS | A:1, B:347, C:12 | 0 |
| modmenu | fabric | PASS_WITH_WARNINGS | A:1, B:6, C:2 | 0 |
| notenoughanimations | neoforge | PASS_WITH_WARNINGS | A:1, B:16 | 1 |
| sodium-extra | fabric | PASS_WITH_WARNINGS | A:1, B:56, D:1 | 3 |
| sodium | fabric | PASS_WITH_WARNINGS | A:1, B:87, C:2 | 0 |
| veinminer | neoforge | PASS_WITH_WARNINGS | A:1, B:1 | 0 |
| xaerominimap | forge | PASS_WITH_WARNINGS | A:1, B:29, C:1 | 2 |
| xaeroworldmap | forge | PASS_WITH_WARNINGS | A:1, B:17 | 1 |
| yet_another_config_lib_v3 | neoforge | PASS_WITH_WARNINGS | A:1, B:3, C:2 | 1 |
