# Limitations & environment honesty

The pipeline **never fails silently**: everything below is visible in the
`PortReport` / `VerificationReport` as errors, skipped layers or notes. This
document records what is real in this repository versus what is deferred until
Horizon-side tooling is available.

## 1. Hook catalog: committed snapshot at the pinned revision (was: seed)

`hooks/native-hooks.json` is a **snapshot** of `horizon-rust/horizon`'s
`native-hooks.json` at the pinned revision (386 hooks, NATIVE_HOOKS_API 359,
read from `crates/horizon-client/src/game/native_hooks.rs` at that commit).
The seed catalog that preceded it is gone: every hook name the mapping table
references is now resolved against the real file, and the
`confirmed: false` "proposed name" concept no longer exists.

Re-fetching when the pin moves:

```bash
# 1. pick the new commit, read NATIVE_HOOKS_API from
#    crates/horizon-client/src/game/native_hooks.rs at that commit
# 2. fetch native-hooks.json at that commit and re-annotate it
#    (native_hooks_api + source keys), commit as hooks/native-hooks.json
# 3. update porter.toml revision / hooks_api, re-run
cargo run -p xtask -- feature-mapping
```

`cargo run -p xtask -- feature-mapping` fails the build if a mapping-table
name is missing from the catalog — that is the spec's resolution gate, and
demoting a feature to Tier D + GapReport is what happens
at classify time when a name is absent.

## 2. Data carry: `built-in-fallback` carrier

Spec §4 makes `horizon-port-pack` the carrier. The porter drives the real
single-mod flow `horizon-port-pack check <jar> <out>`: it carries the jar's
content into `<out>/carry/` (a ready `horizon-mod.toml` + `assets/` + `data/`
payload, with 26.3 recipe/item upgrades applied) and the porter zips that
payload into the `.hmod`, recording `data_carry: "horizon-port-pack"`. The
tool's `import`/`inventory`/`fetch`/`carry`/`report` chain is its *modpack*
flow (`import` rejects single jars). When the binary is not installed, a
clearly-labelled `built-in-fallback` carrier packs the jar's `assets/` +
`data/` into a valid `.hmod` itself and records
`data_carry: "built-in-fallback"`; it does **not** implement the real
upgrade/carry semantics. One caveat inherited from the real tool: its
`horizon-mod.toml` omits the `schema` key (api 0.2 format), so the validator
treats a missing `schema` as a warning, not a failure.

`pack.mcmeta` is deliberately not copied: it is not an allowed top-level entry
in a `.hmod` per `docs/api/mod-files.md`.

### 2.1 Modpack archives: what the fallback does and does not do

For a Modrinth `.mrpack` or a CurseForge zip, `port-pack` with the real
carrier installed drives the upstream `horizon-port-pack import <archive>
<work>` (verified against the upstream CLI surface — there is no `port`
subcommand). Without the carrier, the manifest-driven fallback fills the gap
the mission names — mods with Java code are not ported by the launcher:

- `.mrpack` manifests list direct CDN URLs pinned by sha1: the fallback
  downloads every `mods/*.jar`, verifies the pinned sha1 (and size when
  declared) before anything touches the pipeline, and ports each mod through
  the full five-phase pipeline. On the real Fabulously Optimized pack this
  ports 49/49 code mods (~1,970 features) in ~10 s.
- **Upstream source**: for a Modrinth CDN URL the project id is resolved via
  the public Modrinth API to the `source_url` its page links; for GitHub
  repos the default-branch tarball (`codeload.github.com`, no API key) is
  fetched, extracted under `<work>/sources/<mod>/` (path-traversal guarded,
  64 MiB cap), and handed to the LLM fallback as real upstream code instead
  of bare decompiled signatures. Every outcome is recorded in
  `port-pack-summary.json` as `code_source.status`: `fetched`, `no-source-url`,
  `non-github`, `no-java-sources`, or `fetch-failed` — nothing is invented
  when a mod has no usable repo link.
- **Cross-mod mixin targets**: the pack builds a class index over all staged
  sibling jars, so a compat mixin targeting another pack mod's API class
  resolves instead of false-failing the `mixin_targets_resolvable` gate
  (single-jar ports still flag it). `org/lwjgl/*` is a known runtime prefix —
  LWJGL ships with Minecraft itself.
- `resourcepacks/*.zip` and other non-`mods/*.jar` entries are recorded with
  their kind and **not** pushed through the Java pipeline (an earlier build
  misported the pack's resource packs as one-feature "mods" — fixed).
- CurseForge manifests name files only as `(projectID, fileID)` pairs that
  need the key-gated CF API: each pair is recorded
  `requires-curseforge-api`, never guessed into a URL.
- All three override roots (`overrides/`, `client-overrides/`,
  `server-overrides/`) are carried as data.

The API endpoints can be redirected for offline tests via
`AUTO_PORTER_MODRINTH_API` and `AUTO_PORTER_GITHUB_CODELOAD`.

The fallback does not implement the real tool's `inventory`/`fetch`/`carry`
chain (Modrinth lookups, 26.3 upgrades, resource-pack staging) — a
`"carrier": "built-in-fallback"` entry in `port-pack-summary.json` marks
exactly that.

## 3. Compile gate: implemented — the launcher-style build, executed headlessly

A standalone `cargo check` of a generated mod is not possible: the mod
compiles only inside a Horizon workspace. The gate therefore runs the
launcher's own pipeline (`launcher/src/native_mods.rs`, verified line-by-line
against `horizon@6beacfe9`), driven headlessly by `verify::compile`:

1. unpack the `.hnmod`, parse `horizon-native.json` + the mixin manifest,
2. stage the pristine pinned source (`AUTO_PORTER_HORIZON_SOURCE`, the
   `[horizon].source_dir` config, or the pinned codeload archive fetched once
   into a cache dir — the launcher's "once per commit" behaviour; symlinks
   are dropped the way its staging drops them),
3. when any bundle carries mixin content (any table beyond
   `mod`/`compatibility`/`api`/`dependencies` — the launcher's
   `mixins_content` predicate): strip launcher-only tables and run the real
   `horizon-mixins apply`; otherwise (the trait-style bundle auto-porter
   emits) plain-sync, because mixins refuses content-less manifests,
4. the launcher's `api::stage`: `[api] path` → `native-api/<id>/`, its
   `horizon-client` dependency rewritten to the staged tree with
   `default-features = false`, the generated `horizon-native-game`
   executable package (whose `main` calls `mod_i::hooks()` and
   `horizon_client::entry::main_with`), workspace members extended,
5. `cargo fetch --locked` + `cargo metadata --offline` to complete the lock
   with the staged path packages (the launcher's "Resolving trait mod
   crates"),
6. `cargo check --locked --profile fast --no-default-features -p
   horizon-native-game` — `--features horizon-client/mods` only while the
   revision still declares it (the pinned one does not), `--no-default-
   features` being the launcher's headless engine target (no ALSA/libudev).
   `AUTO_PORTER_GATE_BUILD=1` upgrades the step to a full `cargo build`
   (proves linking; needs `cc` and either the dev libraries or the headless
   target),
7. the §5.2 dispatch smoke: the emitted api crate carries a `#[cfg(test)]
   dispatch_smoke` module with one test per wired hook whose real
   `dispatch_*` signature is synthesizable outside the game (exactly
   `host: &mut dyn Host, mods: &[Box<dyn Hooks>]` — ground truth is the
   `dispatch_args` column the extractor now captures per hook; events are
   `#[non_exhaustive]`, so these free functions are the only dispatch path
   from outside, and the game's own hook points call exactly them). After a
   clean check the gate runs `cargo test --locked --profile fast
   --no-default-features -p <api crate> --lib` on the staged tree: the
   pinned Horizon build's real dispatch machinery executes the emitted
   delegation. A smoke failure fails the gate; a bundle with no
   synthesizable hook reports `no dispatch smoke` honestly instead of
   inventing coverage.

The verdict is honest by construction: `Pass` only when the pinned package
type-checks (reason carries the pipeline summary), `Fail` with the compiler
output tail, `Skipped` naming the missing prerequisite (no source, no cargo,
no horizon-mixins) — a skipped gate counts as a warning, never as a pass,
and a failing one fails the verification report. The gate is exercised on
every push by `.github/workflows/bundle-compile.yml` on a GitHub-hosted
`ubuntu-latest` runner (the gate is fully headless; pinned source, mixins
tool and cargo target are cached between runs): a mock-LLM port of the
fixture jar, compiled against real Horizon.
Its first swing caught three real emitter bugs the static lint could never
see (feature modules missing `use horizon_client::mod_api::*;`, template
skeletons returning `()` where the trait wants `Flow`/`Option`, and
non-underscored stub parameters dying under `#![deny(warnings)]`) — the
reason this layer exists.

Related honesty flag: generated bundles carry `#![deny(warnings)]` and pass the
static API lint (no `unsafe`, only `mod_api`/`Host` imports), but the lint is
*not* a compile. `verify --static` issues a `manifest_placeholder_sha` warning
only while the pinned revision is the all-zero placeholder (porter.toml now
pins the real Horizon commit, so normal runs are warning-free).

## 4. LLM fallback

Template-first generation needs no network. The fallback (spec §10) is wired
for `anthropic`, `openai`, and `nvidia` providers (NVIDIA NIM is
OpenAI-compatible: `NVIDIA_API_KEY`, request goes to
`https://integrate.api.nvidia.com/v1/chat/completions`, `max_tokens` 16384,
`reasoning_effort: high` — the maximum documented effort — and retryable
failures get up to 100 attempts per request, while auth/4xx errors fail
fast), with the §10 prompt, `porter.toml` timeout/retries, and **distinct**
failure semantics:

- `LlmTimeout` — a real call that timed out,
- `LlmUnavailable` — **additive** `ErrorType` (not in the §9 sketch): no API
  key configured or unknown provider. Reporting that as a timeout would lie,
- `LlmFailed` — **additive** `ErrorType`: the provider answered but rejected
  the call (HTTP error status, e.g. an invalid key → 401 with the provider's
  own body surfaced verbatim) or returned nothing usable.

Retries follow §10: initial attempt plus `max_retries` (default 3), each
surfaced as its own error entry in the report.

The tier-B features whose mapping rule carries no template (currently the
player-tick pre/post hooks) are the live LLM-fallback path; Tier C stays
human-design and Tier D stays report-only regardless of the provider.

## 5. Parity & in-game layers

### 5.1 Scenario generation (spec §6.2, Tier A)

`port` auto-generates the Tier A scenarios from the mod's own carried data
(`crates/port-code/src/scenarios.rs`): **place-block** parity scenarios
(blockstates-derived ids, `_bottom` halves preferred, place → break →
`broken_blocks`), **in-game smoke** content steps (`setblock` →
`assert_block` → `break` → `assert_block` air → `inventory` contains the
loot drop, plus `give` → `inventory` for recipe results), and **craft
recipe** scenarios in `-tests/parity-manual/`. Drops are asserted only when
the block's loot table is a single unconditional `minecraft:item` pool
(vanilla `survives_explosion`/`explosion_decay`/`set_count`/`apply_bonus`
boilerplate tolerated; silk-touch dual paths are not asserted).

Two runner gaps are documented rather than faked:

- **crafting** needs player-model actions: the console-only Java oracle
  cannot craft and horizon-ctl has no craft command (§0.8), so craft
  scenarios are complete YAML parked outside the parity runner's glob until
  both harnesses gain a craft verb — they are never counted as passing,
- **container/mob loot** cannot be probed (neither harness can attach a loot
  table to a chest); carried loot tables are counted in the smoke header.

Generated scenarios only contain verbs the runners execute today — a
scenario the runner would count as passing without checking anything would
be a lie.

### 5.2 Environment handling

`tests/parity/run.py` and `tests/in-game/run.py` are real drivers (YAML
scenarios, `horizon-ctl` subprocess integration, minimal RCON client for the
Java oracle, advisory SSIM), but they need:

- `horizon-ctl` (a built Horizon; Agent Control enabled — the launcher's
  "No AI" switch blocks it),
- a licensed Minecraft 26.3 server for the Java oracle
  (`tests/parity/fetch_server.py --accept-eula`; jar never committed),
- for screenshots: software Vulkan / Xvfb on CI (SSIM advisory unless the
  goldens were produced on the same runner image).

Without them, `verify --parity --in-game` reports the layers as
`skipped` with the reason and `OracleUnavailable` error entries — and the CLI
still exits successfully for `--static` runs.

## 6. Fixtures

`tests/fixtures/jars/*.jar` are synthetic mods built by
`scripts/build_fixtures.sh` (portable JDK 17): real compiled class files with
real `RuntimeVisibleAnnotations` (Mixin/Inject/SubscribeEvent) and real
constant-pool callback references, plus assets/data trees. They exist so the
Phase 1 parser, Phase 2 mapping and the packaging validators are exercised
deterministically; the committed snapshots under `tests/fixtures/expected/`
pin that behaviour. The fixture set does not include a "20 real mods"
reference set — that is the modpack-sweep's job on CI.

## 7. Known add-ons relative to the spec text

- `ErrorType::LlmUnavailable` (additive, see §4 above).
- `PortReport.data_carry` records which carrier ran.
- `VerificationReport.compile.status` is `skipped` with a reason instead of
  pretending to run without the launcher.
- `hooks/native-hooks.json` (the committed catalog snapshot) and
  `mixins_revision` in `porter.toml` are new config surface, documented in
  the file itself.

## 8. Bundle bindability (resolved — emitted shape matches the launcher)

The original gap, verified against `horizon-rust/horizon@6beacfe9`
(launcher `native_mods/{api,crates,inject}.rs`) and the
`horizon-rust/horizon-mods` registry: auto-porter emitted `[[injection]]`
tables with no `id` (which the launcher's `bundle()` silently drops), no
`[api]` table, and no `hooks()` export — valid Rust that could never load.
Catalog trait hooks, moreover, need no injections at all: the game dispatches
them through `mod_api::mods()` (`native_player_tick` calls
`mod_api::dispatch_player_tick(self, mod_api::mods())`), and `trait_method`
injections are only for non-catalog targets.

What the pipeline emits now (the trait-style mod path, the launcher's
`api::stage` contract):

- `mixins/<id>/horizon-mixin.toml` carries exactly `[mod]` (id, version),
  `[compatibility]` (horizon_revision, consistent with the
  `horizon-native.json` pin) and `[api] path = "engine"` — no injections.
- `mixins/<id>/engine/` is a crate (edition 2024, `horizon-client` path
  dependency, which the launcher rewrites to the staged game) whose `lib.rs`
  declares the feature modules, a `#[derive(Default)] pub struct PortedHooks`
  holding every feature module's `Hooks` struct, `impl mod_api::Hooks for
  PortedHooks` delegating each catalog hook in `hooks_used` to the feature
  modules that implement it (all implementers run in feature order;
  `Flow::Cancel` short-circuits; value hooks take the first implementer), and
  the `pub fn hooks() -> Box<dyn mod_api::Hooks>` entry point the launcher's
  generated `main` calls (`mod_i::hooks()`).
- The LLM contract (v3.2) additionally requires `#[derive(Default)]` (or
  `impl Default`) on the generated struct — the launcher constructs the mod's
  hooks with `<Struct>::default()` — and underscore-prefixed unused
  parameters (`_host`, matching Horizon's own `mod_api_hooks.rs` test),
  because the crate builds with `#![deny(warnings)]`; both are enforced by
  the validator and repaired like any other rejection. Templates render the
  REAL trait signature from
  `hooks/hook-signatures.json` (`{{HOOK_METHOD_BLOCK}}`; `_`-prefixed
  parameter names because the skeleton body reads none), bodies that satisfy
  the return type (`Flow::Continue` / `None` / nothing), and derive
  `Default`.
- `verify --hnmod` mirrors the launcher's load-time rules: `[mod]` id/version
  present, id charset (`api::valid_id`), the `[api]` path points at a crate in
  the bundle with a `horizon-client` dependency, and `pub fn hooks()` exists.

Honest fallbacks, not fabrications: a hook without an extracted signature
(2 of 386, render-crate) renders no method at all — the trait's default no-op
keeps the crate buildable and the hook is reported as unwired; the bundle
assembler returns the same warnings into the report. Compiling the bundle is
no longer deferred — the launcher-style build runs headlessly in
`porter verify --compile` (§3) and in ci (bundle-compile.yml) — and the
dispatch half of the §5.2 last mile is now covered by the gate's dispatch
smoke: the pinned build's real `dispatch_*` machinery executes the emitted
delegation inside the same staged workspace the launcher builds.

The in-game half is now covered too, and it needed no license. The earlier
recon had stopped at "resources come from your own Minecraft installation" —
that wall is not load-bearing. `horizon-prepare` takes each pinned input
individually (`--version-json`, `--client-jar`, `--libraries-dir`,
`--assets-dir`, `--server-jar`; each "replaces what --minecraft-dir would
give") and verifies it against research-lock.json's sha1 pins; every one of
those files is publicly served by Mojang with no authentication
(piston-meta version JSONs, piston-data client and server jars,
libraries.minecraft.net). "Horizon never downloads Mojang's files" is the
EULA posture of its launcher, which downloads them for a signed-in owner —
it is not a technical restriction of the preparer or the game. The game
itself boots with no session ("A session the game can't read is reported on
stderr; the game still starts" — its launch guide §6), and offline names are
a first-class mode.

So the §5.2 last mile runs in CI on a hosted runner, accountless
(`.github/workflows/in-game-dispatch.yml`): the fetcher
(`scripts/fetch_mojang_inputs.py`) pulls the pinned 26.3 version JSON by its
content address (stable against Mojang's republishes, which change only the
assetIndex), the client and server jars and all 114 libraries, verifying
every sha1 against the pins; `horizon-prepare` is built at the pin and
prepares the resources offline (sounds, languages, panorama, packs, fonts
are optional extras — the minimal resource set boots a silent game);
`verify --compile` builds the staged `horizon-native-game` (the gate's
`AUTO_PORTER_GATE_BUILD` mode, the launcher's no-ALSA engine target); and
the game boots under Xvfb with `--offscreen` through `horizon-ctl launch
--game`, no session, headless. The ported hook body calls `Host::broadcast`
("a system line in every player's chat and the server's log"), so the
booted game's server log carries one marker line per live dispatch — the
job fails unless the log has them, and a hook that never fires cannot fake
them. The gate's dispatch smoke double absorbs `broadcast` for the same
reason: a real ported body may call it from any hook.

What stays guarded: port-test.yml's game-client tier (parity, in-game
content and modpack sweeps) still needs a full resource set and interactive
scenario drivers, and stays off `vars.HORIZON_SELFHOSTED` — honest, not
blocked-by-license. Nothing in §5.2 itself remains deferred: the emitted
delegation is proven by the gate's smoke, and the game's own tick loop is
proven calling it in the booted game. `trait_method` injections for
non-catalog targets are still not generated.
