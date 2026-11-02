//! Tier A auto-scenario generation (spec §6.2): "craft recipe, place block,
//! loot chest" scenarios generated from the mod's own carried data.

use shared::types::{DataFile, ParityTestSpec};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

const MAX_FILE_BYTES: u64 = 64 * 1024;
const MAX_JSON_FILES: usize = 512;
const MAX_BLOCKS: usize = 4;
const MAX_RECIPES: usize = 4;

/// Carried-data summary the scenario templates are generated from.
#[derive(Debug, Default, Clone)]
pub struct TierAData {
    /// Block ids derived from `assets/<ns>/blockstates/<name>.json`.
    pub blocks: Vec<String>,
    /// block id -> simple unconditional item drop, from the block's loot
    /// table (one pool, one item entry, no conditions).
    pub block_drops: BTreeMap<String, String>,
    /// Crafting recipes with plain-item ingredients (tag ingredients are
    /// excluded: the harnesses can only `give` concrete items).
    pub recipes: Vec<CraftRecipe>,
    /// Total loot tables carried (blocks + chests + entities).
    pub loot_tables: usize,
}

#[derive(Debug, Clone)]
pub struct CraftRecipe {
    /// Datapack recipe id, e.g. `<ns>:gold_ring`.
    pub id: String,
    pub result: String,
    pub count: u32,
    /// Plain item ids, in key order (shaped) or list order (shapeless).
    pub ingredients: Vec<String>,
}

/// Read the mod's data out of the source jar. `data_files` is the
/// decomposed inventory; only the paths listed there are opened.
pub fn harvest(jar: &Path, data_files: &[DataFile]) -> TierAData {
    let mut data = TierAData::default();
    let Ok(file) = std::fs::File::open(jar) else {
        return data;
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return data;
    };

    let jsons: Vec<&DataFile> = data_files
        .iter()
        .filter(|f| f.path.ends_with(".json") && f.size <= MAX_FILE_BYTES)
        .filter(|f| {
            f.kind == "data"
                || (f.kind == "asset"
                    && f.path.starts_with("assets/")
                    && f.path.contains("/blockstates/"))
        })
        .take(MAX_JSON_FILES)
        .collect();

    for f in &jsons {
        let Ok(mut entry) = archive.by_name(&f.path) else {
            continue;
        };
        let mut buf = Vec::with_capacity(f.size.min(MAX_FILE_BYTES) as usize);
        if entry.read_to_end(&mut buf).is_err() {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf) else {
            continue;
        };
        if f.kind == "data" {
            classify_data_file(f, &v, &mut data);
        } else {
            classify_asset_file(f, &mut data);
        }
    }
    // two-half blocks: the _bottom half is the meaningful place/break target
    // (placing a lone top half is legal but never what the mod intends)
    let bottoms: Vec<String> = data
        .blocks
        .iter()
        .filter(|b| b.ends_with("_bottom"))
        .cloned()
        .collect();
    data.blocks.retain(|b| {
        !(b.ends_with("_top")
            && bottoms.contains(&format!("{}_bottom", b.trim_end_matches("_top"))))
    });
    data.blocks.truncate(MAX_BLOCKS);
    data.recipes.truncate(MAX_RECIPES);
    data
}

/// Route one parsed data JSON into the summary.
fn classify_data_file(f: &DataFile, v: &serde_json::Value, data: &mut TierAData) {
    let parts: Vec<&str> = f.path.split('/').collect();
    // data/<ns>/recipe[s]/<path>.json
    if parts.len() >= 4 && parts[0] == "data" && (parts[2] == "recipe" || parts[2] == "recipes") {
        if let Some(r) = parse_recipe(parts[1], &parts[3..], v) {
            data.recipes.push(r);
        }
        return;
    }
    // data/<ns>/loot_table[s]/... — blocks/<name>.json yields simple drops
    if parts.len() >= 4
        && parts[0] == "data"
        && (parts[2] == "loot_table" || parts[2] == "loot_tables")
    {
        data.loot_tables += 1;
        if parts.len() == 5 && (parts[3] == "blocks" || parts[3] == "block") {
            let name = parts[4].trim_end_matches(".json");
            let block_id = format!("{}:{name}", parts[1]);
            if let Some(drop) = simple_drop(v) {
                data.block_drops.insert(block_id, drop);
            }
        }
    }
}

/// Block ids from `assets/<ns>/blockstates/<name>.json` — the blockstates
/// file is the registry mapping (one file per registered block id, however
/// many render models the blockstate fans out to).
fn classify_asset_file(f: &DataFile, data: &mut TierAData) {
    let parts: Vec<&str> = f.path.split('/').collect();
    if parts.len() == 4 && parts[0] == "assets" && parts[2] == "blockstates" {
        let name = parts[3].trim_end_matches(".json");
        if !name.is_empty() {
            data.blocks.push(format!("{}:{name}", parts[1]));
        }
    }
}

/// A "simple" drop: one pool, one `minecraft:item` entry, no conditional
/// identity. Vanilla boilerplate is tolerated: `survives_explosion` /
/// `block_state_property` pool conditions, and `explosion_decay` /
/// `set_count` / `apply_bonus` entry functions (they change stack size, not
/// what the item is — the smoke asserts presence, not counts). Functions
/// with their own conditions (silk-touch dual paths) disqualify.
fn simple_drop(v: &serde_json::Value) -> Option<String> {
    let pools = v.get("pools")?.as_array()?;
    if pools.len() != 1 || v.get("conditions").is_some() {
        return None;
    }
    let pool = &pools[0];
    const POOL_CONDS: [&str; 2] = [
        "minecraft:survives_explosion",
        "minecraft:block_state_property",
    ];
    let empty: Vec<serde_json::Value> = Vec::new();
    for c in pool
        .get("conditions")
        .and_then(|c| c.as_array())
        .unwrap_or(&empty)
    {
        let cond = c.get("condition")?.as_str()?;
        if !POOL_CONDS.contains(&cond) {
            return None;
        }
    }
    let entries = pool.get("entries")?.as_array()?;
    if entries.len() != 1 {
        return None;
    }
    let e = &entries[0];
    if e.get("type")?.as_str()? != "minecraft:item" || e.get("conditions").is_some() {
        return None;
    }
    const FNS: [&str; 3] = [
        "minecraft:explosion_decay",
        "minecraft:set_count",
        "minecraft:apply_bonus",
    ];
    for f in e
        .get("functions")
        .and_then(|f| f.as_array())
        .unwrap_or(&empty)
    {
        let name = f.get("function")?.as_str()?;
        if !FNS.contains(&name) || f.get("conditions").is_some() {
            return None;
        }
    }
    e.get("name")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
}

/// Parse a crafting_shaped / crafting_shapeless recipe; tag ingredients and
/// non-crafting types are excluded (returns None).
fn parse_recipe(ns: &str, path: &[&str], v: &serde_json::Value) -> Option<CraftRecipe> {
    let r#type = v.get("type")?.as_str()?;
    if r#type != "minecraft:crafting_shaped" && r#type != "minecraft:crafting_shapeless" {
        return None;
    }
    let result_obj = v.get("result")?;
    let result = result_obj
        .get("id")
        .or_else(|| result_obj.get("item"))
        .and_then(|x| x.as_str())?
        .to_string();
    if result.starts_with('#') {
        return None;
    }
    let count = result_obj
        .get("count")
        .and_then(|c| c.as_u64())
        .unwrap_or(1)
        .min(u32::MAX as u64) as u32;

    let mut ingredients: Vec<String> = Vec::new();
    if r#type == "minecraft:crafting_shaped" {
        let pattern = v.get("pattern")?.as_array()?;
        let key = v.get("key")?.as_object()?;
        // every symbol used in the pattern must map to a plain item
        for row in pattern {
            for c in row.as_str()?.chars() {
                if c == ' ' {
                    continue;
                }
                let ing = key.get(c.to_string().as_str())?;
                let item = ingredient_item(ing)?;
                ingredients.push(item);
            }
        }
    } else {
        for ing in v.get("ingredients")?.as_array()? {
            ingredients.push(ingredient_item(ing)?);
        }
    }
    if ingredients.is_empty() {
        return None;
    }
    let id = format!("{ns}:{}", path.join("/").trim_end_matches(".json"));
    Some(CraftRecipe {
        id,
        result,
        count,
        ingredients,
    })
}

/// One ingredient slot: `{"item": "a:b"}` (pre-1.21), `{"id": "a:b"}`
/// (1.21+), a bare string, or a tag — tags make the recipe unusable here.
fn ingredient_item(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return (!s.starts_with('#')).then(|| s.to_string());
    }
    let item = v
        .get("id")
        .or_else(|| v.get("item"))
        .and_then(|x| x.as_str())?;
    (!item.starts_with('#')).then(|| item.to_string())
}

// ---- templates -------------------------------------------------------------

fn slug(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Place-block parity scenarios: fully executable by `tests/parity/run.py`
/// on both sides (block-state comparison only).
pub fn parity_specs(mod_id: &str, data: &TierAData) -> Vec<ParityTestSpec> {
    let mut out = Vec::new();
    for block in &data.blocks {
        let name = format!("{mod_id}_place_{}_parity", slug(block));
        let yaml = format!(
            "# auto-generated from the mod's carried data (spec §6.2 Tier A); survival mode\n\
             name: {name}\n\
             description: \"Tier A place-block: {block} places and breaks. Drop checks live in the\n\
             \x20 in-game smoke (the console-only Java oracle cannot read inventory).\"\n\
             setup:\n\
             \x20 world: flat\n\
             \x20 seed: 42\n\
             \x20 player: survival\n\
             \x20 place_blocks:\n\
             \x20   - {{ at: [10, 64, 10], id: \"{block}\" }}\n\
             actions:\n\
             \x20 - break_block: {{ at: [10, 64, 10] }}\n\
             expected:\n\
             \x20 broken_blocks: [[10, 64, 10]]\n",
            name = name,
            block = block,
        );
        out.push(ParityTestSpec {
            name,
            scenario_yaml: yaml,
        });
    }
    out
}

/// Craft-recipe scenarios, parked in `parity-manual/` (outside the parity
/// runner's glob) until a craft verb exists on both harnesses.
pub fn craft_specs(mod_id: &str, data: &TierAData) -> Vec<ParityTestSpec> {
    let mut out = Vec::new();
    for r in &data.recipes {
        let short = r.id.split(':').nth(1).unwrap_or(&r.id).replace('/', "_");
        let name = format!("{mod_id}_craft_{}", slug(&short));
        // compress identical slots for display: "glowstone_dust x4 + stick"
        let mut grouped: Vec<(String, usize)> = Vec::new();
        for i in &r.ingredients {
            match grouped.last_mut() {
                Some((item, n)) if item == i => *n += 1,
                _ => grouped.push((i.clone(), 1)),
            }
        }
        let ingredients = grouped
            .iter()
            .map(|(item, n)| {
                if *n > 1 {
                    format!("{item} x{n}")
                } else {
                    item.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" + ");
        let yaml = format!(
            "# runner-gap: crafting needs player-model actions — the console-only Java\n\
             # oracle cannot craft and horizon-ctl has no craft command (spec §0.8).\n\
             # Complete scenario; executable once both harnesses gain a craft verb.\n\
             name: {name}\n\
             description: \"Tier A craft recipe {id}: {ingredients} -> {result} x{count}\"\n\
             setup:\n\
             \x20 world: flat\n\
             \x20 seed: 42\n\
             \x20 player: survival\n\
             actions:\n\
             \x20 - craft: {{ recipe: \"{id}\", result: {{ id: \"{result}\", count: {count} }} }}\n\
             expected:\n\
             \x20 inventory:\n\
             \x20   - {{ id: \"{result}\", min_count: {count} }}\n",
            id = r.id,
            ingredients = ingredients,
            result = r.result,
            count = r.count,
        );
        out.push(ParityTestSpec {
            name,
            scenario_yaml: yaml,
        });
    }
    out
}

/// The in-game smoke: ready check + executable content steps for the first
/// blocks with simple drops and recipe results.
pub fn smoke_yaml(mod_id: &str, data: &TierAData) -> String {
    let mut steps = format!(
        "# in-game smoke (consumed by tests/in-game/run.py, NOT by horizon-ctl)\n\
         # Tier A content checks auto-generated from the carried data;\n\
         # {} loot table(s) carried — container/mob loot probing is a documented\n\
         # runner gap (docs/limitations.md §5).\n\
         steps:\n\
         \x20 - launch: {{ world: {mod_id}_smoke, seed: 42, creative: false }}\n\
         \x20 - assert_ready: {{}}\n",
        data.loot_tables,
        mod_id = mod_id,
    );
    for block in data.blocks.iter().take(2) {
        let Some(drop) = data.block_drops.get(block) else {
            continue;
        };
        steps.push_str(&format!(
            "  - setblock: {{ pos: [10, 64, 10], id: \"{block}\" }}\n\
             \x20 - assert_block: {{ pos: [10, 64, 10], id: \"{block}\" }}\n\
             \x20 - break: {{ pos: [10, 64, 10] }}\n\
             \x20 - assert_block: {{ pos: [10, 64, 10], id: minecraft:air }}\n\
             \x20 - inventory: {{ contains: {{ id: \"{drop}\" }} }}\n",
            block = block,
            drop = drop,
        ));
    }
    for r in data.recipes.iter().take(2) {
        steps.push_str(&format!(
            "  - give: {{ id: \"{}\", count: 1 }}\n\
             \x20 - inventory: {{ contains: {{ id: \"{}\" }} }}\n",
            r.result, r.result
        ));
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a synthetic jar with the data shapes the generator parses.
    fn write_test_jar(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        let mut add = |name: &str, body: &str| {
            z.start_file(name, opts).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        };
        add("assets/am/blockstates/glowing_block.json", "{}");
        add(
            "data/am/recipe/glowing.json",
            r#"{"type":"minecraft:crafting_shaped","pattern":["GG","GG"],
                "key":{"G":{"id":"minecraft:glowstone_dust"}},
                "result":{"id":"am:glowing_block","count":1}}"#,
        );
        // tag ingredient -> excluded from craft scenarios
        add(
            "data/am/recipe/tagged.json",
            r#"{"type":"minecraft:crafting_shapeless",
                "ingredients":[{"tag":"minecraft:planks"}],
                "result":{"id":"am:glowing_block"}}"#,
        );
        // smelting -> excluded (furnace flows need other runner support)
        add(
            "data/am/recipe/smelting.json",
            r#"{"type":"minecraft:smelting","ingredient":{"id":"minecraft:sand"},
                "result":{"id":"minecraft:glass"}}"#,
        );
        add(
            "data/am/loot_table/blocks/glowing_block.json",
            // realistic shape: vanilla boilerplate conditions + functions
            r#"{"type":"minecraft:block","pools":[{"rolls":1,
                "conditions":[{"condition":"minecraft:survives_explosion"}],
                "entries":[{"type":"minecraft:item","name":"am:glowing_block",
                  "functions":[{"function":"minecraft:explosion_decay"},
                               {"function":"minecraft:apply_bonus",
                                "enchantment":"minecraft:fortune","formula":"minecraft:ore_drops"}]}]}]}"#,
        );
        // silk-touch dual path (conditional identity) -> no simple drop
        add(
            "data/am/loot_table/blocks/shifter_block.json",
            r#"{"type":"minecraft:block","pools":[{"rolls":1,
                "entries":[{"type":"minecraft:item","name":"am:shifter_block",
                  "functions":[{"function":"minecraft:set_name","name":"fancy",
                    "conditions":[{"condition":"minecraft:match_tool",
                      "predicate":{"predicates":{"minecraft:enchantments":{"minecraft:silk_touch":{}}}}}]}]}]}]}"#,
        );
        // blockstates are the registry source; the _top/_bottom dedupe
        // still guards mods that ship half ids as separate blocks
        add("assets/am/blockstates/tall_thing_bottom.json", "{}");
        add("assets/am/blockstates/tall_thing_top.json", "{}");
        add(
            "data/am/loot_table/chests/treasure.json",
            r#"{"type":"minecraft:chest","pools":[{"rolls":1,
                "entries":[{"type":"minecraft:item","name":"minecraft:diamond"}]}]}"#,
        );
        add("data/am/tags/item/my_tag.json", "{\"values\":[]}");
        z.finish().unwrap();
    }

    fn df(path: &str, kind: &str, size: u64) -> DataFile {
        DataFile {
            path: path.to_string(),
            sha256: String::new(),
            size,
            kind: kind.to_string(),
        }
    }

    #[test]
    fn harvest_parses_recipes_blocks_and_drops() {
        let dir = std::env::temp_dir().join("auto-porter-scenarios-test");
        std::fs::create_dir_all(&dir).unwrap();
        let jar = dir.join("testmod.jar");
        write_test_jar(&jar);

        let files = vec![
            df("assets/am/blockstates/glowing_block.json", "asset", 2),
            df("assets/am/blockstates/tall_thing_bottom.json", "asset", 2),
            df("assets/am/blockstates/tall_thing_top.json", "asset", 2),
            df("data/am/recipe/glowing.json", "data", 160),
            df("data/am/recipe/tagged.json", "data", 130),
            df("data/am/recipe/smelting.json", "data", 120),
            df("data/am/loot_table/blocks/glowing_block.json", "data", 380),
            df("data/am/loot_table/blocks/shifter_block.json", "data", 300),
            df("data/am/loot_table/chests/treasure.json", "data", 120),
        ];
        let data = harvest(&jar, &files);

        // the _top half of a two-half pair is dropped
        assert_eq!(
            data.blocks,
            vec![
                "am:glowing_block".to_string(),
                "am:tall_thing_bottom".to_string()
            ]
        );
        // realistic table (survives_explosion + apply_bonus) yields a drop;
        // the silk-touch dual path does not
        assert_eq!(
            data.block_drops.get("am:glowing_block").map(|s| s.as_str()),
            Some("am:glowing_block")
        );
        assert!(!data.block_drops.contains_key("am:shifter_block"));
        assert_eq!(data.loot_tables, 3);
        // only the shaped plain-item recipe survives
        assert_eq!(data.recipes.len(), 1);
        let r = &data.recipes[0];
        assert_eq!(r.id, "am:glowing");
        assert_eq!(r.result, "am:glowing_block");
        // one entry per pattern slot: 2x2 of G = 4 glowstone dust
        assert_eq!(r.ingredients, vec!["minecraft:glowstone_dust"; 4]);
    }

    #[test]
    fn generated_yaml_uses_only_executable_verbs_and_parses() {
        let data = TierAData {
            blocks: vec!["am:glowing_block".to_string()],
            block_drops: BTreeMap::from([(
                "am:glowing_block".to_string(),
                "am:glowing_block".to_string(),
            )]),
            recipes: vec![CraftRecipe {
                id: "am:glowing".to_string(),
                result: "am:glowing_block".to_string(),
                count: 1,
                ingredients: vec!["minecraft:glowstone_dust".to_string()],
            }],
            loot_tables: 2,
        };

        let parity = parity_specs("am", &data);
        assert_eq!(parity.len(), 1);
        assert!(parity[0].scenario_yaml.contains("place_blocks:"));
        assert!(parity[0].scenario_yaml.contains("break_block:"));
        assert!(parity[0]
            .scenario_yaml
            .contains("broken_blocks: [[10, 64, 10]]"));
        // no invented verbs in the executable parity scenario
        assert!(!parity[0].scenario_yaml.contains("craft"));

        let craft = craft_specs("am", &data);
        assert_eq!(craft.len(), 1);
        assert!(craft[0].scenario_yaml.contains("runner-gap"));

        let smoke = smoke_yaml("am", &data);
        for verb in [
            "setblock:",
            "assert_block:",
            "break:",
            "inventory:",
            "give:",
        ] {
            assert!(smoke.contains(verb), "smoke missing {verb}");
        }

        // YAML structural sanity for all three templates: the runners
        // safe_load these, so indentation must be clean multiples of 2 and
        // the key anchors must be present.
        for yaml in [
            parity[0].scenario_yaml.as_str(),
            craft[0].scenario_yaml.as_str(),
            smoke.as_str(),
        ] {
            for (i, line) in yaml.lines().enumerate() {
                let indent = line.len() - line.trim_start().len();
                assert_eq!(indent % 2, 0, "odd indent at line {i}: {line:?}");
            }
        }
    }

    #[test]
    fn hostile_inputs_are_bounded() {
        let dir = std::env::temp_dir().join("auto-porter-scenarios-test2");
        std::fs::create_dir_all(&dir).unwrap();
        let jar = dir.join("empty.jar");
        let file = std::fs::File::create(&jar).unwrap();
        let mut z = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        z.start_file("data/am/recipe/broken.json", opts).unwrap();
        z.write_all(b"{ not json").unwrap();
        z.finish().unwrap();

        let data = harvest(&jar, &[df("data/am/recipe/broken.json", "data", 9)]);
        assert!(data.recipes.is_empty());
        assert!(data.blocks.is_empty());
        assert_eq!(data.loot_tables, 0);

        // missing jar -> empty summary, no panic
        let missing = harvest(Path::new("/nonexistent/nope.jar"), &[]);
        assert!(missing.blocks.is_empty());
    }
}
