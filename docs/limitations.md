# Limitations & environment honesty

The pipeline **never fails silently**: everything below is visible in the
`PortReport` / `VerificationReport` as errors, skipped layers or notes. This
document records what is real in this repository versus what is deferred until
Horizon-side tooling is available.

## 1. Hook catalog: seed until the pinned `native-hooks.json`

`hooks/native-hooks.seed.json` is a **seed**, built from the Horizon docs
(`docs/native-hooks-neoforge.md` confirmed names + PR #3964/#6216/#5638/#6452),
not the real `native-hooks.json`. Rows carry `confirmed: false` when the name
is only *proposed* (`right_click_item`, `living_incoming_damage`, ...).

Replace it before real use:

```toml
[horizon]
hooks_json = "/path/to/native-hooks.json"   # at the pinned revision
hooks_api  = <NATIVE_HOOKS_API there>
```

`cargo run -p xtask -- feature-mapping --hooks-json <real>` fails the build if
a mapping-table name is missing from the real catalog — that is the spec's
resolution gate, and demoting a feature to Tier D + GapReport is what happens
at classify time when a name is absent.

## 2. Data carry: `built-in-fallback` carrier

Spec §4 makes `horizon-port-pack` the carrier. When the binary is not
installed (any machine without a Horizon checkout, including this one), Phase 3
packs the jar's `assets/` + `data/` into a valid `.hmod` itself and records
`data_carry: "built-in-fallback"` in the report. It does **not** implement the
real upgrade/carry semantics (26.3 format rewrites, id remapping, modpack
tiering) — that stays with `horizon-port-pack`, which the porter drives
(`check` → `import` → `inventory` → `fetch` → `carry` → `report`) the moment
`port_pack_path` resolves.

`pack.mcmeta` is deliberately not copied: it is not an allowed top-level entry
in a `.hmod` per `docs/api/mod-files.md`.

## 3. Compile gate: deferred, never faked

A standalone `cargo check` of a generated mod is not possible
(`docs/native-mods.md`); the launcher-style build is the only compile gate.
Without `horizon-ctl` the gate reports `status: "skipped"` with the reason —
it is never reported as passing. Where the launcher exists, the verification
layer executes the pinned build path
(`horizon-mixins apply` → `cargo build --locked --profile release --features
horizon-client/mods -p horizon-client`).

Related honesty flag: generated bundles carry `#![deny(warnings)]` and pass the
static API lint (no `unsafe`, only `mod_api`/`Host` imports), but the lint is
*not* a compile. `verify --static` issues a `manifest_placeholder_sha` warning
while the pinned revision is the all-zero placeholder.

## 4. LLM fallback

Template-first generation needs no network. The fallback (spec §10) is wired
for `anthropic` and `openai` providers with the §10 prompt, `porter.toml`
timeout/retries, and **distinct** failure semantics:

- `LlmTimeout` — real call that timed out or errored,
- `LlmUnavailable` — **additive** `ErrorType` (not in the §9 sketch): no API
  key configured or unknown provider. Reporting that as a timeout would lie.

## 5. Parity & in-game layers

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
- `hooks/native-hooks.seed.json` and `mixins_revision` in `porter.toml` are
  new config surface, documented in the file itself.
