# auto-porter

**Java → Horizon mod porter**: takes a Minecraft Java Edition mod JAR (Fabric,
NeoForge or legacy Forge) and produces the Horizon equivalents — a content mod
(`.hmod`), a native mod (`.hnmod`) and a structured `PortReport` (JSON) saying
what was ported, skipped or blocked, and why.

The mission brief this tool implements lives in
[`auto-porter-prompt.md`](auto-porter-prompt.md).

```
┌───────────┐   ┌──────────┐   ┌───────────┐   ┌───────────┐   ┌──────────┐
│  Phase 1  │   │ Phase 2  │   │  Phase 3  │   │  Phase 4  │   │ Phase 5  │
│ DECOMPOSE │──▶│ CLASSIFY │──▶│ PORT DATA │──▶│ PORT CODE │──▶│  VERIFY  │
│ (jar→AST) │   │(feat.map)│   │(port-pack)│   │(Rust gen) │   │(parity + │
│           │   │          │   │           │   │           │   │ in-game) │
└───────────┘   └──────────┘   └───────────┘   └───────────┘   └──────────┘
```

## Quickstart

```bash
cargo build --workspace

# Decompose + classify only. Writes decomposed.json and classified.json
cargo run -p cli -- analyze tests/fixtures/jars/veinminer-fabric-1.0.0.jar --output ./out/veinminer/

# Full port: .hmod + .hnmod + <mod-id>-report.json + <mod-id>-tests/
cargo run -p cli -- port tests/fixtures/jars/veinminer-fabric-1.0.0.jar \
  --horizon-revision <FULL_40_HEX_SHA> --output ./output/ --verbose

# Complex fixture: spans all four tiers, incl. the §10 LLM fallback
# (player-tick listener -> player_tick_pre hook, no template) and a
# deliberate Tier D gap (command registration). Provider/model come from
# porter.toml ([llm]); override per-run with global flags:
NVIDIA_API_KEY=nvapi-... cargo run -p cli -- --llm-provider nvidia \
  --llm-model minimaxai/minimax-m3.1-flash \
  port tests/fixtures/jars/chunkcourier-neoforge-2.0.0.jar --output ./output/ --verbose
# without a key the same run degrades honestly: llm_unavailable on the
# tick feature, everything template-covered still ports

# Data only (used by CI)
cargo run -p cli -- port tests/fixtures/jars/moreassets-fabric-2.1.0.jar --data-only --output ./output/

# Modpack: a directory of jars, a Modrinth .mrpack, or a CurseForge zip
cargo run -p cli -- port-pack path/to/modpack --horizon-revision <SHA> --output ./output/

# Verify: --static / --compile / --parity / --in-game
cargo run -p cli -- verify ./output/veinminer/ --static --verbose

# Regenerate docs/feature-mapping.md from the hook catalog (fails the build
# when a referenced hook is missing from native-hooks.json)
cargo run -p xtask -- feature-mapping
```

## Layout

| Path | What |
|---|---|
| `crates/shared/` | common types, PortReport, errors, `porter.toml` config, hook-catalog loader |
| `crates/decompose/` | Phase 1: JAR → class-level AST (own class-file parser, Mixin + event extraction) |
| `crates/classify/` | Phase 2: Java concept → Horizon hook mapping, Tier A–D assignment, GapReports |
| `crates/port-data/` | Phase 3: `.hmod` via `horizon-port-pack` (documented offline fallback included) |
| `crates/port-code/` | Phase 4: template-driven Rust generation + LLM fallback → `.hnmod` bundle |
| `crates/verify/` | Phase 5: static gates (mod-files rules, hook resolution, source lint) + parity/in-game orchestration |
| `crates/xtask/` | generates `docs/feature-mapping.md` from `native-hooks.json` |
| `crates/cli/` | the `auto-porter` binary (`port`, `port-pack`, `verify`, `analyze`) |
| `templates/` | one `.rs` template per common pattern, `{{PLACEHOLDER}}` tokens |
| `hooks/native-hooks.seed.json` | offline seed catalog — replace with the pinned `native-hooks.json` |
| `tests/fixtures/` | three fixture mod jars (Fabric mixin+callback, NeoForge events+BE+shader, pure data) + committed analyze snapshots |
| `tests/parity/`, `tests/in-game/` | differential + horizon-ctl smoke harnesses (Python) |
| `.github/workflows/` | `port-test.yml`, `nightly-ecosystem.yml`, `release.yml` |

## Status: what runs where

Everything here is exercised in CI and locally:

- full pipeline on the fixture jars (`cargo test --workspace`, e2e in `crates/cli/tests/e2e.rs`),
- deterministic analyze snapshots committed under `tests/fixtures/expected/`,
- static verification of both bundle formats against the mod-files rules,
- the xtask mapping gate.

What needs Horizon-side tooling (documented in
[docs/limitations.md](docs/limitations.md)):

- the real data carry (`horizon-port-pack`) — until installed, a clearly
  labelled `built-in-fallback` carrier packs `assets/` + `data/` into a valid
  `.hmod` so the pipeline stays exercisable,
- the compile gate (launcher-style build; a standalone `cargo check` is not
  possible for native mods),
- parity + in-game layers (`horizon-ctl` + a licensed MC 26.3 copy on a
  self-hosted runner),
- LLM fallback (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `OPENROUTER_API_KEY` /
  `NVIDIA_API_KEY`; default provider `openrouter` with model
  `thinkingmachines/inkling-small:free`). **Note:** OpenRouter gates that
  `:free` sku behind registered "agentic harnesses" — requests with a valid
  key get HTTP 403 `...only available on agentic harnesses...`. Use
  `--llm-provider nvidia --llm-model minimaxai/minimax-m3.1-flash` (or another
  model) for a working free path.
- Retry policy: providers get `max_retries` (default 3) extra attempts on
  retryable failures (408/429/5xx/transport/empty), with `retry_delay_ms`
  between attempts. **NVIDIA NIM always gets the documented ceiling of 100
  attempts per request** (its free tier rate-limits hard); auth/4xx client
  errors are fatal and never retried. `reasoning_effort` (default `high`) is
  sent to NVIDIA. `base_url` overrides the endpoint for self-hosted NIM or
  gateways — `scripts/mock_llm_server.py` uses it to test the retry policy
  without a key.

## CI

`.github/workflows/port-test.yml` runs the hosted, Horizon-independent set on
every push/PR: build + clippy + fmt + tests, the analyze snapshot diff, the
data-only ports with static verification, full Tier A/B code generation with
`verify --static --compile` (the compile layer reports `skipped` without the
launcher), and the xtask mapping-drift gate.

The jobs that need a pinned Horizon revision (`data-port-tests` via
`cargo install --rev`, and the parity / in-game / modpack jobs on the
self-hosted `horizon` runner) are gated on the `HORIZON_REVISION` repository
variable — they stay marked *skipped* until you set it (which also signals
that a labeled runner is registered). `nightly-ecosystem.yml` runs the
top-100 sweep nightly on hosted runners; its full-verify mode and the status
issue comment are likewise gated on repo variables.

## Real-mod testing

`scripts/test_real_mods.sh` runs the pipeline end-to-end against two pinned
Modrinth downloads (nothing third-party is committed — jars go to a temp dir):

- **Waystones** (neoforge 1.21.1, 380 classes) — a Balm-wrapped event surface:
  no `@SubscribeEvent` anywhere, so decompose follows the concrete
  `balm/api/event/*` types through handler constant pools. Expect 32 features;
  the Balm `LivingDamageEvent` / `LivingDeathEvent` / `UseBlockEvent` listeners
  map to `living_incoming_damage` / `living_death` / `right_click_block`, the
  login / server-lifecycle / screen events become honest Tier D GapReports.
- **Corail Tombstone** (neoforge 1.21.1, 590 classes) — direct API, 112
  features: NeoForge 1.21.1's phase-split `LivingDamageEvent$Pre` maps, and
  `PlayerInteractEvent$LeftClickBlock` resolves `left_click_block` with no
  template → the spec §10 LLM fallback. Without `OPENROUTER_API_KEY` set the
  script asserts the honest `llm_unavailable` report; with a key, generation
  runs for real.

The static verify on both bundles is `PASS_WITH_WARNINGS` (only the pinned-
revision placeholder warnings). The script also guards vanilla-model texture
semantics: `#var` references and `minecraft:`-namespace textures resolve at
runtime, so they must not fail the cross-check.

## Configuration

`porter.toml` (spec §11) pins the Horizon revision, `NATIVE_HOOKS_API`, tool
paths, LLM settings, parity/in-game options. The two placeholder SHAs
(all-zero) must be replaced with real full 40-hex commits before you rely on
generated bundles — reports flag them with
`horizon_revision_verified: false` until then.
