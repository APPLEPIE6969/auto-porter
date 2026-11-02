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
