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
- LLM fallback (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY`).

## Configuration

`porter.toml` (spec §11) pins the Horizon revision, `NATIVE_HOOKS_API`, tool
paths, LLM settings, parity/in-game options. The two placeholder SHAs
(all-zero) must be replaced with real full 40-hex commits before you rely on
generated bundles — reports flag them with
`horizon_revision_verified: false` until then.
