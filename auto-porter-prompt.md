# Mission: Build `auto-porter`, a Java → Horizon Mod Porter

You are building **`auto-porter`**, a pipeline that takes a Minecraft Java Edition mod JAR
(Fabric, NeoForge or legacy Forge; written for 1.21+ up to 26.3) and produces the Horizon
equivalents with as little human intervention as the platform allows:

- a **content mod** (`.hmod`): data and assets, no code
- a **native mod** (`.hnmod`): a `horizon-mixins` source bundle, built into the game by the launcher
- a structured **`PortReport`** (JSON) saying what was ported, skipped or blocked, and why

## 0. Ground truth about Horizon (verified against the repos, read before designing anything)

Horizon is `github.com/horizon-rust/horizon`: Minecraft Java Edition **26.3** rewritten in Rust,
checked against vanilla by differential harnesses (the README says 281, and also "more than 300";
do not hard-code either number). Related repos: `horizon-mods` (registry),
`horizon-mixins` (compile-time source mixins), `horizon-native-mod-template`, `horizon-server`.

Facts the design must respect:

1. **Java mod JARs do not run in Horizon.** Horizon does not execute Java code. It carries over
   what a mod holds as *data* (blocks, items, recipes, loot tables, tags, worldgen, textures, names)
   with `horizon-port-pack`, and reads class files only to report what it cannot carry.
   Modpack import already exists in the launcher: it carries over content-only mods today and
   says before install which features are left out. Mods with Java code are not ported yet.
   That gap is exactly what `auto-porter` fills.
2. **Two mod kinds** (`docs/mods.md`):
   | Kind | File | What it is |
   |---|---|---|
   | Content mod | `.hmod` | Data and assets. No code. |
   | Native mod | `.hnmod` | A `horizon-mixins` bundle: Rust that changes the game's own source **before it is compiled**. The launcher rebuilds the game on the player's machine. It is **not sandboxed**. |
3. **There is no WebAssembly mod path in this design.** Code mods are native Rust only.
4. **Typed native-mod API** (PR #6216, issue #5638, follow-ups in #6452): a mod implements the
   generated `horizon_client::mod_api::Hooks` trait and calls back through a generated `Host`.
   The trait is generated from `native-hooks.json` (`cargo run -p xtask -- mod-api`).
   Hook contracts are `listen`, `cancel`, `replace`, `collect`. Mixin injections still work, and a mod may use both.
5. **The hook catalog grows.** PR #6216 described 108 hooks and 78 Host helpers; issue #6452
   later counts 186 hooks in `native-hooks.json`. **Never hard-code counts or hook names.**
   Read `native-hooks.json` at the pinned revision and generate the mapping from it.
6. **A `.hnmod` is a bundle, not a bare crate.** It carries `horizon-native.json`
   (names the hook-API version it needs, e.g. `"hooks": 14`, or pins a commit),
   `horizon-mixin.toml` (injections, extra crates, post-build steps) and `mixins/<id>/…`
   (including a crate's own `data/`). Every rule (zip and path rules, top-level entries) is in
   `docs/api/mod-files.md`. **Treat that file as the source of truth for packaging.** `.hnmod` is a **zip**.
7. **Injection example** (from PR #3964): `[[injection]]` with `hook = "<name from native-hooks.json>"`,
   `hook_version`, `kind = "HEAD"`, `helper`, `bindings`. Trait injections are limited to HEAD.
8. **`horizon-ctl`** (ships with the game) is a CLI that reads the game as JSON and acts like a player;
   `horizon-ctl mcp` serves the same commands as MCP tools (named without a prefix, e.g. `break`,
   `inventory`, `screenshot`; the launcher's own MCP prefixes them `game_`). Real commands include
   `launch --world W --seed N --creative --wait`, `state`, `find-block`, `goto X Y Z --near N`,
   `break X Y Z`, `inventory`, `open-inventory`, `click-slot`, `screenshot PATH`, `block`/`blocks`.
   There is **no `horizon-ctl script` command**; our harness drives the CLI/MCP itself.
   The user can set **No AI** in the launcher, which makes `horizon-ctl` refuse to run; Agent Control must be on.
9. **Building Horizon needs** Rust 1.90+, Python 3.10+, JDK 25 and the player's **own copy** of
   Java Edition 26.3 (`horizon-prepare` never downloads it). CI that runs the game must obtain a
   legitimately licensed copy (private cache or secret, self-hosted runner) and must never publish it.
10. **Modpack formats:** `.mrpack` is **Modrinth's** format; CurseForge uses a `.zip` with a manifest.
    Horizon's own modpack format is `.hpack`. Modrinth has no Horizon loader.
11. `horizon-port-pack` has a `check` command that runs on a Java jar. It also upgrades mods from
    1.21+ versions, so the porter must accept jars older than 26.3 instead of rejecting them.

**Assumptions to verify in week 1 (do not build on them blindly):** that the exact CLI surface of
`horizon-port-pack` (flags, output layout) matches what this prompt uses; that Fabric/NeoForge builds
exist for 26.3 (needed for the Java oracle in §6); the Java-side mappings situation for Mixin targets
at 26.x (check whether Mojang ships unobfuscated jars for the version you target).

---

## 1. Architecture

Five phases, each a separate crate in a Cargo workspace. Each reads the previous phase's typed
artifact. Failures never abort silently: they become entries in the `PortReport`.

```
┌───────────┐   ┌──────────┐   ┌───────────┐   ┌───────────┐   ┌──────────┐
│  Phase 1  │   │ Phase 2  │   │  Phase 3  │   │  Phase 4  │   │ Phase 5  │
│ DECOMPOSE │──▶│ CLASSIFY │──▶│ PORT DATA │──▶│ PORT CODE │──▶│  VERIFY  │
│ (jar→AST) │   │(feat.map)│   │(port-pack)│   │(Rust gen) │   │(parity + │
│           │   │          │   │           │   │           │   │ in-game) │
└───────────┘   └──────────┘   └───────────┘   └───────────┘   └──────────┘
```

```
auto-porter/
├── Cargo.toml                    # workspace
├── porter.toml                   # config (see §11)
├── templates/                    # Rust templates with {{PLACEHOLDER}} tokens (see §5.1)
├── crates/
│   ├── shared/                   # common types, PortReport, errors
│   ├── decompose/                # Phase 1
│   ├── classify/                 # Phase 2
│   ├── port-data/                # Phase 3
│   ├── port-code/                # Phase 4
│   ├── verify/                   # Phase 5
│   ├── xtask/                    # generates docs/feature-mapping.md from native-hooks.json
│   └── cli/                      # [[bin]] name = "auto-porter"  (the binary must NOT be called "cli")
├── tests/
│   ├── fixtures/{jars,expected}/ # known Java mods + expected AST/tier snapshots
│   ├── parity/                   # differential harness + scenarios
│   └── in-game/                  # horizon-ctl driven scenarios + golden images
├── .github/workflows/{port-test,nightly-ecosystem,release}.yml
└── docs/{feature-mapping.md,limitations.md}
```

---

## 2. Phase 1: Decompose (JAR → AST)

**Input:** a `.jar` (Fabric, NeoForge or Forge).

1. Unpack. Parse metadata: `fabric.mod.json`, `META-INF/neoforge.mods.toml`, or `META-INF/mods.toml`
   (id, version, dependencies, entrypoints).
2. Parse every `.class` file into a **class-level AST** (package, name, superclass, interfaces,
   fields, methods with bytecode). Use a Rust class-file parser crate (for example `cafebabe` or
   `noak`) or an ASM-based helper; `javap -p -c` is fine for debugging, not as the parser.
   (`smali`/`jadx` are Android/Dalvik tools; do not use `smali`.)
3. Extract **Mixin classes** (`@Mixin`) and their members: `@Inject`, `@Redirect`, `@ModifyArg`,
   `@ModifyVariable`, `@ModifyConstant`, `@Overwrite`, `@Shadow`, `@Unique`, `@Accessor`,
   `@Invoker`. For each record the target class, method selector, `@At` (HEAD, TAIL/RETURN, INVOKE, …)
   and the body as pseudo-IR.
4. Extract **event listeners**:
   - NeoForge/Forge: methods annotated `@SubscribeEvent`, and `@EventBusSubscriber` classes
     (`@Mod.EventBusSubscriber` in older Forge).
   - Fabric: callback registrations such as `XCallback.EVENT.register(...)` (Fabric has no `@SubscribeEvent`).
   Record the event type and handler method.
5. Extract data files (`assets/`, `data/`, `pack.mcmeta`, locale files, models, textures, sounds).
   Phase 3's `horizon-port-pack` does the real carry; this step only inventories them.
6. Extract config schemas (`toml`/`json`).
7. Emit `DecomposedMod`:

```rust
pub struct DecomposedMod {
    pub metadata: ModMetadata,
    pub classes: Vec<ClassAst>,
    pub mixins: Vec<MixinAst>,
    pub events: Vec<EventListenerAst>,
    pub data_files: Vec<DataFile>,        // path + hash (content on demand)
    pub config: Option<ConfigSchema>,
    pub dependencies: Vec<Dependency>,
}
```

**Quality gates (CI):** every class in the JAR appears in the AST; every Mixin target resolves to a known
class (or is reported `UnknownMixinTarget`); data-file count equals the JAR's file listing.

---

## 3. Phase 2: Classify (feature map)

**Input:** `DecomposedMod`.

1. Map each Mixin target/injection and event listener to a **Horizon hook** using
   `docs/feature-mapping.md` (§3.1), a table **generated by `xtask` from `native-hooks.json`** plus a
   hand-maintained Java-side column.
2. Assign each feature a **portability tier**:

| Tier | Description | Automation |
|---|---|---|
| **A** | Pure content (blocks, items, recipes, loot, worldgen, textures, sounds) | Fully automatic (via `horizon-port-pack`) |
| **B** | Simple behaviour (tool behaviour, block break/place, keybinds, HUD, tooltips) | Mostly automatic, human review |
| **C** | Complex behaviour (machines, energy/fluid networks, mobs with AI, multiblocks, logic GUIs, network sync) | Partly automatic, human design |
| **D** | Renderer/shader overhauls, worldgen overhauls, protocol changes, or anything with no hook | Report only |

   Percentages are targets to measure, not promises; record the real numbers per release.
3. Emit a `PortPlan` per feature:

```rust
pub struct PortPlan {
    pub feature: Feature,
    pub tier: Tier,
    pub data_files: Vec<PathBuf>,
    pub hooks_needed: Vec<HookRef>,        // name + hook_version from native-hooks.json
    pub host_helpers: Vec<HelperRef>,
    pub rust_skeleton: Option<String>,     // Tier A/B
    pub human_notes: Vec<String>,          // Tier C/D
    pub parity_tests: Vec<ParityTestSpec>,
}
```

### 3.1 Feature mapping (Java concept → Horizon)

The Java-side names below are corrected. The **Horizon column is a lookup key, not a promise**:
only the names confirmed in the repos are shown as such; everything else must be resolved against
`native-hooks.json` by `xtask` (it fails the build if a name is missing).

| Java concept (NeoForge/Forge | Fabric) | Horizon target (resolve in `native-hooks.json`) | Contract |
|---|---|---|---|
| `BlockEvent.BreakEvent` | `PlayerBlockBreakEvents.BEFORE` | block-breaking hook (resolve) | cancel |
| `BlockEvent.EntityPlaceEvent` | `UseBlockCallback` + block-place check | block-placing hook (resolve) | cancel |
| `PlayerInteractEvent.RightClickItem` / `RightClickBlock` | `UseItemCallback` / `UseBlockCallback` | item-use hooks, e.g. `use_item_on` (exists, see #5638) | replace |
| `LivingHurtEvent` / `LivingDamageEvent` (`LivingIncomingDamageEvent` on newer NeoForge) | `ServerLivingEntityEvents.ALLOW_DAMAGE` | hurt hook (resolve) | cancel |
| `LivingDeathEvent` | `ServerLivingEntityEvents.AFTER_DEATH` | death hook (resolve) | listen |
| `EntityJoinLevelEvent` (was `EntityJoinWorldEvent`) | `ServerEntityEvents.ENTITY_LOAD` | spawn hook (resolve) | listen |
| `PlayerInteractEvent.EntityInteract` | `UseEntityCallback` | entity-interact hook (resolve) | cancel |
| `ItemEntityPickupEvent` / `PlayerEvent.ItemPickupEvent` | (mixin) | pickup hook (resolve) | cancel |
| `ItemTossEvent` | (mixin) | drop hook (resolve) | listen |
| `KeyMapping` registration | `KeyBindingHelper.registerKeyBinding` | native key registration via `Host` (resolve) | none |
| GUI layer/overlay events (`RegisterGuiLayersEvent`, `RenderGuiLayerEvent`; `RenderGuiOverlayEvent` on older versions) | `HudRenderCallback` | `hud` (exists, see #3323 docs), `frame`, renderer hooks | listen |
| `BlockEntity` with logic (called `TileEntity` in old Forge) | `BlockEntityType` + ticker | block-entity hooks + fluid/redstone `Host` helpers (resolve) | none |
| Custom payloads / packets | `PayloadTypeRegistry` + networking API | `net_from_client` / `net_from_server` style hooks + `Host` send helpers (resolve) | none |
| Custom recipe | data JSON | data carry via `horizon-port-pack`; recipe-type hook only for new recipe *types* | collect |
| Loot tables, worldgen JSON | data JSON | data carry | none |
| New entity/mob | entity registration + goals | entity spawn helper + tick hooks (resolve) | none |
| `Screen` / `AbstractContainerMenu` | same | menu slot-rule + renderer hooks (resolve) | none |
| Energy/fluid capabilities | `Transfer API` | fluid tank / redstone `Host` helpers (resolve) | none |
| Config (`ModConfigSpec`) | config libs | data file + a mod-publish `Host` helper (resolve) | none |
| Generic Mixin | Mixin | `horizon-mixins` injection (HEAD / and whatever kinds the pinned `horizon-mixins` supports) | none |

If a Java concept has **no** Horizon equivalent, mark the feature **Tier D** and emit a `GapReport`
(what is missing; which Horizon issue/PR would close it, or "open one").

---

## 4. Phase 3: Port data

**Input:** the mod JAR + `PortPlan` data files.

1. Run `horizon-port-pack` to carry over blocks, items, recipes, loot tables, tags, models, textures,
   structures, worldgen, names, sounds, lang files. Start with `horizon-port-pack check <jar>`
   (it reports what carries and what does not); then run the carry command (confirm its flags in week 1).
2. Validate the resulting `.hmod` in `crates/verify` against the rules in `docs/api/mod-files.md`:
   zip/path rules, every JSON parses, every referenced texture/model/sound exists, no ID clashes with vanilla,
   recipes reference existing items.
3. Tier C/D mods still get their data ported; the code part is handled separately.
4. Emit the `.hmod` and a `DataPortReport`.

**Gates:** `horizon-port-pack` exits 0; validation passes; file count matches input (no silent drops).

---

## 5. Phase 4: Port code (Rust generation)

**Input:** `PortPlan` + `DecomposedMod` IR. Strategy: **template-driven generation, LLM only as fallback.**

### 5.1 Templates

`templates/` holds one `.rs` file per common pattern with `{{PLACEHOLDER}}` tokens:
`block_break_hook.rs`, `block_place_hook.rs`, `item_use_hook.rs`, `player_hurt_hook.rs`,
`entity_spawn.rs`, `net_channel.rs`, `keybind.rs`, `hud_overlay.rs`, `block_entity.rs`, `fluid_tank.rs`,
`redstone_output.rs`, `config_load.rs`, `mixin_head.rs`, `mixin_overwrite.rs`, …
Each template targets one hook contract and is parameterised by mod id, feature name, block/item/entity
ids, conditions ("only if sneaking") and actions ("drop 2x loot"). Seed them from
`horizon-native-mod-template` and the Veinminer port it is based on.

### 5.2 Generation pipeline, per feature

1. **Match** a template; if found, fill it.
2. **Otherwise call the LLM** with: the Java IR of the relevant methods; the exact hook signature and
   `hook_version` from `native-hooks.json`; the available `Host` methods from the generated `mod_api`;
   three worked examples from `templates/`; and the instruction *"Implement the `horizon_client::mod_api::Hooks`
   trait method `<hook_name>`. Use only `Host` helpers and `mod_api` re-exports. No `unsafe`. Do not import
   `horizon_client` internals."*
3. **Assemble the bundle:** `horizon-native.json` (with `"hooks": <NATIVE_HOOKS_API>` or a pinned revision),
   `horizon-mixin.toml` (only if injections or extra crates are needed), `mixins/<id>/…` containing the
   Rust sources and helpers, and any crate `data/`. Follow `docs/api/mod-files.md` exactly.
4. **Compile check:** build the bundle the way the launcher does (`docs/native-mods.md`) against the
   pinned Horizon revision. This rebuilds the game, so it is slow: cache by (Horizon revision, mixins revision).
   If the pinned revision also supports a fast standalone `cargo check` against `horizon-client` with
   `--features mods`, use that as a first gate. On failure, feed compiler errors back to the LLM (max 3 retries).

### 5.3 Output

A `.hnmod` **zip** (source bundle + `horizon-native.json` + `horizon-mixin.toml`). The registry
(`horizon-rust/horizon-mods`) validates source-only bundles; run the same checks locally.

**Gates:** build succeeds with `#![deny(warnings)]`; `cargo clippy -- -D warnings` and `cargo fmt --check`
pass on the mod's own crates; no `unsafe`; no `horizon_client` internals (only `mod_api` re-exports and
`Host`); every hook and `hook_version` used exists in the pinned `native-hooks.json`; `"hooks"` equals the
pinned revision's `NATIVE_HOOKS_API` or is high enough for every hook used.

---

## 6. Phase 5: Verify

Inputs: `.hmod`, `.hnmod`, `ParityTestSpec` list. Three layers.

### 6.1 Static

Schema/zip rules (`docs/api/mod-files.md`); hook and `hook_version` existence; dependency resolution;
`hooks` API version vs pinned revision.

### 6.2 Parity (differential)

Run the **same scenario** on the original mod in Java and on the port in Horizon, then diff the state.

- **Java oracle:** a headless Minecraft Java **26.3** server (fetched by `tests/parity/fetch_server.py` from Mojang's
  official version manifest; never committed, EULA accepted explicitly) plus the original mod on a loader build that supports 26.3.
  If no loader build exists for the mod's loader, the scenario cannot run on the oracle side: report `OracleUnavailable`.
- **Horizon side:** the game launched via `horizon-ctl launch --world W --seed N [--creative]`, or `horizon-server` for
  server-only scenarios. Use a virtual display and software Vulkan on CI.
- **Scenario runner:** our own Python driver calling `horizon-ctl` / its MCP tools on the Horizon side and the server's
  console/RCON on the Java side. Scenario YAML (note: **survival** mode, since creative breaking drops nothing;
  iron ore drops `raw_iron`, not `iron_ore`):

```yaml
# tests/parity/scenarios/veinminer_break_iron.yaml
name: veinminer_break_iron
description: "Breaking one iron ore with Veinminer breaks connected ore"
setup:
  world: flat
  seed: 42
  player: survival
  inventory: [{ id: minecraft:iron_pickaxe, slot: 0 }]
  place_blocks:
    - { id: minecraft:iron_ore, at: [10, 64, 10] }
    - { id: minecraft:iron_ore, at: [11, 64, 10] }
    - { id: minecraft:iron_ore, at: [10, 64, 11] }
    - { id: minecraft:stone,    at: [12, 64, 10] }
actions:
  - break_block: { at: [10, 64, 10], tool: minecraft:iron_pickaxe }
expected:
  broken_blocks: [[10, 64, 10], [11, 64, 10], [10, 64, 11]]
  not_broken:    [[12, 64, 10]]
  drops:
    - { id: minecraft:raw_iron, count: 3 }
```

- **Compare:** block positions, inventory, entity positions, chat, sound and particle events. Any mismatch is a
  `ParityMismatch` with block/item/entity, expected vs actual.
- Horizon's own differential harnesses compare Horizon to **vanilla**; the parity layer here extends the idea to
  **mod behaviour**. Reuse their conventions (`docs/parity-workflow.md`), not their results.
- **Scenario generation:** Tier A auto-generates (craft recipe, place block, loot chest); Tier B has the LLM write 5–10
  per feature from the IR; Tier C has humans write the critical ones and the LLM the edge cases.
- Flaky timing: retry a failed scenario once.

### 6.3 In-game smoke (`horizon-ctl`)

A Python driver runs steps through `horizon-ctl` (or its MCP server), for example:

```yaml
# tests/in-game/smoke/veinminer.yaml   (consumed by tests/in-game/run.py, NOT by horizon-ctl)
steps:
  - launch:   { world: test_world, seed: 42, creative: false }    # horizon-ctl launch --world test_world --seed 42 --wait
  - goto:     { pos: [10, 64, 10], near: 2.5 }                    # horizon-ctl goto 10 64 10 --near 2.5
  - break:    { pos: [10, 64, 10] }                               # horizon-ctl break 10 64 10
  - assert_block: { pos: [11, 64, 10], id: minecraft:air }        # horizon-ctl block 11 64 10
  - assert_block: { pos: [12, 64, 10], id: minecraft:stone }
  - inventory: { contains: { id: minecraft:raw_iron, min_count: 3 } }
  - screenshot: { path: out/veinminer.png, compare_with: golden/veinminer_break.png, ssim_min: 0.95 }
```

Screenshot SSIM is **advisory** on CI: software-rendered frames differ across GPUs/drivers. Make it a warning
unless goldens are produced on the same runner image. Golden images live in `tests/in-game/golden/`.

### 6.4 Verification report

```json
{
  "mod_id": "veinminer",
  "tier": "B",
  "static": { "schema": "pass", "hooks": "pass", "deps": "pass" },
  "parity": {
    "scenarios_run": 12, "scenarios_passed": 11, "scenarios_failed": 1,
    "failures": [{
      "scenario": "veinminer_break_iron_diagonal",
      "expected": { "broken_blocks": [[10,64,10],[11,64,11]] },
      "actual":   { "broken_blocks": [[10,64,10]] },
      "diagnosis": "Diagonal connectivity not implemented in port"
    }]
  },
  "in_game": { "steps_run": 8, "steps_passed": 8, "screenshot_ssim": 0.97 },
  "overall": "PASS_WITH_WARNINGS",
  "human_review_needed": ["diagonal vein connectivity"]
}
```

---

## 7. CLI

```bash
# Single mod
cargo run -p cli -- port path/to/mod.jar --tier auto --horizon-revision <SHA> --output ./output/ --verbose
#   ./output/<mod-id>.hmod            content mod (if any data)
#   ./output/<mod-id>.hnmod           native mod zip (if any code)
#   ./output/<mod-id>-report.json     PortReport
#   ./output/<mod-id>-tests/          generated parity + in-game tests

# Data only (used by CI)
cargo run -p cli -- port path/to/mod.jar --data-only --output ./output/

# Modpack (a directory of JARs, a Modrinth .mrpack, or a CurseForge .zip)
cargo run -p cli -- port-pack path/to/modpack --horizon-revision <SHA> --output ./output/

# Verify only
cargo run -p cli -- verify ./output/<mod-id>/ [--static] [--compile] [--parity] [--in-game] --verbose

# Decompose + classify only. Writes decomposed.json and classified.json
cargo run -p cli -- analyze path/to/mod.jar --output ./out/<name>/
```

`<SHA>` is a **full Horizon commit hash** you pick and record in `porter.toml` and the repo variable
`HORIZON_REVISION`. (The short hash in earlier drafts could not be verified; do not copy it.)

---

## 8. GitHub Actions

Workflow-level `env` (not per job) so every job sees the pinned revision. Jobs that launch Horizon need a
licensed Minecraft 26.3 copy, so they run on a **self-hosted runner** labelled `horizon` with Xvfb and Mesa Vulkan
(lavapipe) installed. Everything else runs on `ubuntu-latest`.

### 8.1 `.github/workflows/port-test.yml`

```yaml
name: Port Test

on:
  pull_request: { branches: [main] }
  push:         { branches: [main] }

permissions:
  contents: read

env:
  HORIZON_REVISION: ${{ vars.HORIZON_REVISION }}   # full commit SHA
  CARGO_TERM_COLOR: always

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: "clippy, rustfmt" }
      - uses: Swatinem/rust-cache@v2
      - run: cargo build --workspace --release
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo fmt --all --check
      - run: cargo test --workspace
      - run: cargo test --workspace --doc
      - uses: actions/upload-artifact@v4
        with:
          name: auto-porter
          path: target/release/auto-porter

  analyze-tests:                       # Phase 1 + 2 snapshots
    runs-on: ubuntu-latest
    needs: build
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with: { name: auto-porter, path: bin/ }
      - name: Analyze fixtures and diff against snapshots
        run: |
          chmod +x bin/auto-porter
          fail=0
          for jar in tests/fixtures/jars/*.jar; do
            name=$(basename "$jar" .jar)
            bin/auto-porter analyze "$jar" --output "target/actual/$name"
            diff -r "tests/fixtures/expected/$name" "target/actual/$name" \
              || { echo "::error::snapshot mismatch: $name"; fail=1; }
          done
          exit $fail

  data-port-tests:
    runs-on: ubuntu-latest
    needs: build
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with: { name: auto-porter, path: bin/ }
      - uses: dtolnay/rust-toolchain@stable
      - name: Build horizon-port-pack at the pinned revision
        run: |
          # Confirm the package/bin name in the Horizon workspace in week 1.
          cargo install --locked --git https://github.com/horizon-rust/horizon \
            --rev "$HORIZON_REVISION" horizon-port-pack
      - name: Port data and validate
        run: |
          chmod +x bin/auto-porter
          fail=0
          for jar in tests/fixtures/jars/*.jar; do
            name=$(basename "$jar" .jar)
            bin/auto-porter port "$jar" --data-only --output "target/data/$name" || fail=1
            bin/auto-porter verify "target/data/$name" --static || fail=1
          done
          exit $fail

  code-gen-tests:
    runs-on: ubuntu-latest
    needs: build
    env:
      ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with: { name: auto-porter, path: bin/ }
      - uses: dtolnay/rust-toolchain@stable
        with: { components: "clippy, rustfmt" }
      - name: Generate native mods for Tier A/B fixtures
        run: |
          chmod +x bin/auto-porter
          for jar in tests/fixtures/jars/tier-ab/*.jar; do
            bin/auto-porter port "$jar" --horizon-revision "$HORIZON_REVISION" \
              --output "target/generated/$(basename "$jar" .jar)"
          done
      - name: Static + compile + lint every generated bundle
        run: |
          fail=0
          for dir in target/generated/*/; do
            bin/auto-porter verify "$dir" --static --compile || { echo "::error::FAIL $dir"; fail=1; }
          done
          exit $fail

  parity-tests:
    runs-on: [self-hosted, horizon]    # needs a licensed MC 26.3 copy; see §8 intro
    needs: [build, code-gen-tests]
    strategy:
      fail-fast: false
      matrix:
        shard: [0, 1, 2, 3]
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with: { name: auto-porter, path: bin/ }
      - name: Fetch Java oracle server
        run: python3 tests/parity/fetch_server.py --version 26.3 --out java-oracle   # Mojang manifest; never committed
      - name: Build Horizon at the pinned revision
        run: python3 tests/parity/build_horizon.py --rev "$HORIZON_REVISION" --out horizon-build
      - name: Run parity scenarios (shard ${{ matrix.shard }})
        run: |
          xvfb-run -a python3 tests/parity/run.py \
            --shard "${{ matrix.shard }}" --total-shards 4 \
            --java-dir java-oracle --horizon-dir horizon-build \
            --scenarios tests/parity/scenarios/ --output tests/parity/results/
      - run: python3 tests/parity/check.py --results tests/parity/results/ --fail-on-mismatch
      - if: always()
        uses: actions/upload-artifact@v4
        with:
          name: parity-results-${{ matrix.shard }}
          path: tests/parity/results/

  in-game-tests:
    runs-on: [self-hosted, horizon]
    needs: [build, code-gen-tests]
    steps:
      - uses: actions/checkout@v4
      - name: Build Horizon (client + horizon-ctl) at the pinned revision
        run: python3 tests/parity/build_horizon.py --rev "$HORIZON_REVISION" --out horizon-build
      - name: Run smoke scenarios
        run: |
          xvfb-run -a python3 tests/in-game/run.py \
            --horizon-dir horizon-build \
            --scenarios tests/in-game/smoke/ --golden tests/in-game/golden/ \
            --ssim-threshold 0.95 --ssim-advisory \
            --output tests/in-game/results/
      - run: python3 tests/in-game/check.py --results tests/in-game/results/ --fail-on-mismatch
      - if: always()
        uses: actions/upload-artifact@v4
        with: { name: in-game-results, path: tests/in-game/results/ }

  modpack-sweep:
    runs-on: [self-hosted, horizon]
    needs: [parity-tests, in-game-tests]
    if: github.event_name == 'push'
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with: { name: auto-porter, path: bin/ }
      - name: Download the reference set (20 mods, all tiers)
        run: python3 tests/modpack/download.py --output tests/modpack/jars/   # Modrinth API; jars are not committed or redistributed
      - run: |
          chmod +x bin/auto-porter
          bin/auto-porter port-pack tests/modpack/jars/ --horizon-revision "$HORIZON_REVISION" --output tests/modpack/output/
          xvfb-run -a bin/auto-porter verify tests/modpack/output/ --parity --in-game --verbose
      - run: python3 tests/modpack/summary.py --output tests/modpack/output/ --report tests/modpack/summary.json
      - uses: actions/upload-artifact@v4
        with: { name: modpack-summary, path: tests/modpack/summary.json }

  report:
    runs-on: ubuntu-latest
    needs: [analyze-tests, data-port-tests, code-gen-tests, parity-tests, in-game-tests, modpack-sweep]
    if: always()
    permissions:
      contents: read
      pull-requests: write
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with: { path: results, pattern: "*results*", merge-multiple: true }
        continue-on-error: true
      - run: python3 tests/report/aggregate.py --input results --output report.md
      - name: Post report to PR
        if: github.event_name == 'pull_request'
        uses: actions/github-script@v7
        with:
          script: |
            const fs = require('fs');
            await github.rest.issues.createComment({
              issue_number: context.issue.number,
              owner: context.repo.owner,
              repo: context.repo.repo,
              body: fs.readFileSync('report.md', 'utf8'),
            });
```

### 8.2 `.github/workflows/nightly-ecosystem.yml`

Port-only by default (static + compile). Parity/in-game need the self-hosted runner, so they are a separate opt-in input.

```yaml
name: Nightly Ecosystem Sweep

on:
  schedule: [{ cron: "0 3 * * *" }]       # 03:00 UTC
  workflow_dispatch:
    inputs:
      full_verify: { description: "Also run parity + in-game (self-hosted)", type: boolean, default: false }

permissions:
  contents: read
  issues: write

env:
  HORIZON_REVISION: ${{ vars.HORIZON_REVISION_NIGHTLY || 'main' }}   # 'main' is intentional here; record the resolved SHA in the report

jobs:
  sweep:
    runs-on: ${{ inputs.full_verify && fromJSON('["self-hosted","horizon"]') || 'ubuntu-latest' }}
    timeout-minutes: 360
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo build --workspace --release
      - name: Download top 100 mods
        env:
          CURSEFORGE_API_KEY: ${{ secrets.CURSEFORGE_API_KEY }}   # CurseForge's API requires a key
        run: python3 tests/ecosystem/download_top100.py --sources modrinth,curseforge --output tests/ecosystem/jars/
      - name: Port all
        run: ./target/release/auto-porter port-pack tests/ecosystem/jars/ --horizon-revision "$HORIZON_REVISION" --output tests/ecosystem/output/ --continue-on-error
      - name: Verify
        run: |
          flags="--static --compile"
          if [ "${{ inputs.full_verify }}" = "true" ]; then flags="$flags --parity --in-game"; fi
          xvfb-run -a ./target/release/auto-porter verify tests/ecosystem/output/ $flags --verbose --continue-on-error
      - run: python3 tests/ecosystem/report.py --output tests/ecosystem/output/ --report tests/ecosystem/report.md
      - name: Update the status issue
        uses: actions/github-script@v7
        with:
          script: |
            const fs = require('fs');
            await github.rest.issues.createComment({
              issue_number: Number('${{ vars.ECOSYSTEM_ISSUE_NUMBER }}'),   # repo variable, not a hard-coded number
              owner: context.repo.owner,
              repo: context.repo.repo,
              body: fs.readFileSync('tests/ecosystem/report.md', 'utf8'),
            });
      - uses: actions/upload-artifact@v4
        with: { name: ecosystem-report, path: tests/ecosystem/report.md, retention-days: 30 }
```

(Downloaded jars are third-party works: keep them in runner scratch space, never in artifacts or the repo.)

### 8.3 `.github/workflows/release.yml`

```yaml
name: Release

on:
  push:
    tags: ["v*"]

permissions:
  contents: write

jobs:
  release:
    strategy:
      matrix:
        include:
          - { os: ubuntu-latest,  target: x86_64-unknown-linux-gnu,  ext: tar.gz }
          - { os: macos-latest,   target: aarch64-apple-darwin,      ext: tar.gz }
          - { os: windows-latest, target: x86_64-pc-windows-msvc,    ext: zip }
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: "${{ matrix.target }}" }
      - run: cargo build --workspace --release --target ${{ matrix.target }}
      - name: Package
        shell: bash
        run: |
          name="auto-porter-${{ github.ref_name }}-${{ matrix.target }}"
          mkdir "$name"
          cp target/${{ matrix.target }}/release/auto-porter* "$name"/ 2>/dev/null || true
          if [ "${{ matrix.ext }}" = "zip" ]; then 7z a "$name.zip" "$name"; else tar czf "$name.tar.gz" "$name"; fi
      - uses: softprops/action-gh-release@v2
        with:
          files: auto-porter-*
          generate_release_notes: true
```

---

## 9. Error handling

The pipeline **never fails silently**. Every failure becomes a `PortError` in the `PortReport`:

```rust
pub struct PortError {
    pub phase: Phase,
    pub feature: Option<Feature>,
    pub error_type: ErrorType,
    pub message: String,
    pub java_source: Option<String>,
    pub suggestion: Option<String>,    // what a human could do
    pub horizon_gap: Option<String>,   // which Horizon issue/PR would fix it
}

pub enum ErrorType {
    DecompileFailed,
    UnknownMixinTarget,
    NoHookEquivalent,
    CompileError,
    ParityMismatch,
    VisualMismatch,
    MissingDependency,
    ApiVersionMismatch,     // "hooks" / NATIVE_HOOKS_API mismatch
    OracleUnavailable,      // no Java loader build for this mod on 26.3
    LlmTimeout,
}
```

Retry policy: LLM generation up to 3 retries with compiler feedback; parity failures 1 retry; a feature that exhausts
retries is `SKIPPED` and the rest of the mod is still produced.

Dependencies: port in topological order. If mod B is Tier D, features of A that need B are `BLOCKED` with an explanation.

---

## 10. LLM integration (Phase 4 fallback)

Templates should cover most cases; measure the real template-hit rate per release rather than assuming a ratio.

Prompt template (`crates/port-code/src/llm.rs`):

```
You are generating Rust code for a Horizon native mod.

CONTEXT:
- Horizon is Minecraft: Java Edition 26.3 rewritten in Rust.
- Native mods implement the generated `horizon_client::mod_api::Hooks` trait.
- The `Host` trait provides game helpers (spawn entities, play sounds, redstone, fluid tanks, network, ...).
- Hook contracts: "listen" (observe), "cancel" (veto), "replace" (override the return), "collect" (gather from all mods).

TASK:
Implement `{{hook_name}}` (contract: {{contract}}, hook_version: {{hook_version}}) for the mod `{{mod_id}}`.

JAVA BEHAVIOUR (from the original mod's bytecode):
{{java_ir}}

AVAILABLE HOOK SIGNATURE (from native-hooks.json):
{{hook_signature}}

AVAILABLE HOST HELPERS:
{{relevant_helpers}}

WORKED EXAMPLES:
{{few_shot_examples}}

CONSTRAINTS:
- Use ONLY `mod_api` re-exports and `Host` methods.
- Do NOT use `unsafe`.
- Do NOT import `horizon_client` internals.
- The method signature must match the generated trait exactly.
- Add a doc comment saying what the hook does.
- If the Java behaviour cannot be replicated with the available hooks,
  return the default (no-op) and add a `// TODO: {{reason}}` comment.

OUTPUT:
Return ONLY the Rust method (signature and body), no explanations.
```

Model: provider-configurable. Default to a current Claude model (see `porter.toml`). Do not hard-code a model
name in code. Call with a 60 s timeout; on timeout mark the feature `SKIPPED` with `LlmTimeout`.

---

## 11. `porter.toml`

```toml
[horizon]
revision        = "<FULL_COMMIT_SHA>"          # pinned Horizon commit
hooks_api       = 0                            # set from NATIVE_HOOKS_API at that revision; the build fails if it differs
client_path     = "~/.horizon/horizon-client"
ctl_path        = "~/.horizon/horizon-ctl"
port_pack_path  = "~/.cargo/bin/horizon-port-pack"

[llm]
provider     = "anthropic"                     # or "openai", "local"
model        = "claude-sonnet-5-5"             # current Sonnet; swap for claude-opus-5-5 on hard features
max_retries  = 3
timeout_secs = 60
temperature  = 0.1

[parity]
java_server_dir = "tests/parity/java-oracle/"   # filled by fetch_server.py; never committed
scenario_timeout_secs = 30
shards = 4

[in_game]
ssim_threshold = 0.95
ssim_advisory  = true
golden_dir     = "tests/in-game/golden/"
virtual_display = true

[output]
dir = "./output/"
include_tests  = true
include_report = true
```

---

## 12. Definition of done

A mod is **successfully ported** when:

1. The `.hmod` (if any) passes validation.
2. The `.hnmod` (if any) builds into the pinned Horizon with zero warnings and passes clippy/fmt on its own crates.
3. All Tier A features pass parity (100%).
4. Tier B features pass parity at ≥ 90%, with the rest documented in the report.
5. The in-game smoke test passes (all steps; SSIM is advisory unless goldens are runner-local).
6. A `PortReport` is generated and attached.
7. The mod loads in Horizon without crashing (`horizon-ctl launch --wait`, then `state` returns a valid world).

**Partially ported:** some features are `SKIPPED` or `BLOCKED`; the report lists what works.
**Not portable (Tier D):** the core functionality has no Horizon equivalent; the report explains the gap.

---

## 13. First sprint (one week each; do not try to build everything at once)

1. **Week 1:** `shared` + `decompose`; verify the "assumptions to verify" in §0; get JAR → AST on 10 fixture mods. CI: `analyze-tests` (AST part).
2. **Week 2:** `classify` + `xtask` (generate `docs/feature-mapping.md` from `native-hooks.json`). Tier assignment on the same 10 fixtures.
3. **Week 3:** `port-data` wrapping `horizon-port-pack`; produce valid `.hmod` files. CI: `data-port-tests`.
4. **Week 4:** `port-code` with templates for the ten most common patterns (block break, block place, item use, keybind, HUD, entity spawn, net channel, config, block entity, fluid tank). Tier A/B bundles build. CI: `code-gen-tests`.
5. **Week 5:** `verify` static + parity harness; Java oracle and Horizon runner on 5 scenarios. CI: `parity-tests`.
6. **Week 6:** `verify` in-game layer via `horizon-ctl`; screenshot comparison. CI: `in-game-tests`.
7. **Week 7:** CLI polish, `porter.toml`, error handling, `PortReport`. End-to-end `port mod.jar`.
8. **Week 8:** `modpack-sweep` + `nightly-ecosystem.yml`; port 20 reference mods; publish the first ecosystem report.

---

## 14. References

- Horizon: `github.com/horizon-rust/horizon` (README, `docs/mods.md`, `docs/native-mods.md`, `docs/api/mod-files.md`,
  `docs/mod-browser.md`, `docs/native-client.md` (agent control), `docs/horizon-ctl-commands.md`, `docs/parity-workflow.md`)
- Typed native-mod API: PR #6216, issue #5638, follow-ups #6452
- Hook catalog: `native-hooks.json` (at the pinned revision)
- Named hooks in bundles: PR #3964 (and `horizon-mixins#5`); hook-API versioning: PR #3323
- `horizon-rust/horizon-mixins`, `horizon-rust/horizon-native-mod-template`, `horizon-rust/horizon-mods`, `horizon-rust/horizon-server`
- Sample native mod with ticking machine, screen and trait example: `arismoko/horizon-machine-sample`
- Agent control guide: `playhorizon.dev/docs/agents`; hub and mods: `playhorizon.dev`
