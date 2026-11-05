# Compile gate: real compile-and-load validation (2026-11-06)

Spec §5.2.4 / docs/limitations.md §3, deferred since the launcher-format work
(`a43eb0b`), now implemented. Everything below was re-derived from the actual
runs, not from memory.

## Ground truth (read from the repos, not assumed)

- `horizon-rust/horizon@6beacfe9` `launcher/src/native_mods.rs` +
  `native_mods/{api,crates,inject}.rs`: the launcher's build pipeline is
  pristine source → `horizon-mixins apply` **only when any bundle carries
  mixin content** (`needs_mixins`, native_mods.rs:1874 — an API-only bundle
  takes a plain sync; mixins itself refuses content-less manifests with
  `E_MANIFEST`) → `inject::stage` → `api::stage` (`native-api/<id>/`, the
  dependency rewritten to `default-features = false`, the generated
  `horizon-native-game` package whose `main` calls `mod_i::hooks()` →
  `horizon_client::entry::main_with`) → `cargo fetch --locked` +
  `cargo metadata --offline` → `cargo build --locked --profile <p> -p
  horizon-native-game`.
- The pinned revision has **no `mods` feature** in `horizon-client` any more
  (the runtime folded into the crate proper): the launcher's
  `has_mods_feature` gate decides, and the gate mirrors it.
- `horizon-rust/horizon-mixins@ddd9700` CLI: `plan`/`apply`/`check` with
  `--workspace/--mods/--out`; `--mods` scans one directory level for
  `horizon-mixin.toml` folders; the schema is
  `mod/compatibility/injection/bridge/module/state/crate/post_build` —
  `[api]`/`[dependencies]` are launcher-only and must be stripped before
  mixins sees a manifest.

## The gate's first swing caught three real bugs

A mock-LLM port of `chunkcourier-neoforge-2.0.0.jar` (hermetic), staged and
compiled exactly as above. All static checks and the bindability validator
passed; the real compiler failed the crate with 6 errors + 4 more after the
first fix:

1. **Feature modules had no `use horizon_client::mod_api::*;`** — the glob
   in `lib.rs` does not extend into child modules; every wrapper module
   names `Host`/event structs/`Flow`. Fixed centrally in `render_template`
   (LLM modules already carry the import; the contract mandates it).
2. **Template skeletons returned `()` where the trait wants a value** —
   100 of 384 catalog hooks return `Flow`, 22 return `Option<...>`. Method
   bodies now satisfy the signature: `Flow::Continue`, `None`, nothing.
3. **Non-underscored stub parameters die under `#![deny(warnings)]`** — the
   few-shot itself taught the violation (`fn player_tick(&self, host: ...)`
   with an empty body). Horizon's own `mod_api_hooks.rs` test writes
   `_host`; the few-shot now does too, and a new `unused_param_issue`
   validator rejects unread non-underscored parameters with the fix in the
   rejection reason (contract v3.2).

After the fixes the same pipeline compiles clean:

```
Checking horizon-mod-chunkcourier v0.1.0 (…/native-api/chunkcourier)
Checking horizon-native-game v0.1.0 (…/crates/native-game)
Finished `fast` profile [optimized] target(s) in 0.32s
```

## What shipped (`this commit`)

- `crates/verify/src/compile.rs` — the gate: source resolution (config →
  env → pinned codeload archive with the launcher's symlink-dropping),
  `mixins_content` branch, `api::stage` port, lock completion, `cargo
  check`/`build` with `AUTO_PORTER_GATE_TARGET`/`AUTO_PORTER_GATE_KEEP`/
  `AUTO_PORTER_GATE_BUILD` knobs. Honest verdicts; a skipped gate is a
  warning, a failed gate fails the report.
- `porter verify --compile` wired to the gate (was a stub pointing at a
  non-existent horizon-ctl layer).
- `.github/workflows/bundle-compile.yml` — self-hosted `horizon` runner,
  cached pinned source + target dir, hermetic mock-LLM port, gate must pass.
- Contract v3.2 + validator + few-shot fix; config surface
  (`mixins_path`, `source_dir`); tests: 2 unit (launcher predicates), 1 e2e
  (skipped gate names the missing prerequisite), 1 validator unit.
- Locally proven end-to-end twice: through the archive-fetch path
  (`compile: pass`, "launcher-style pipeline on horizon@6beacfe9") and
  through a git checkout (which correctly trips the symlink refusal the
  launcher has — the archive path is the supported one).

## Remaining deferred

Booting the game with the bundle installed and watching hooks dispatch
(`horizon-ctl`, Agent Control) — the last mile of §5.2. Contract v3.2's
prompt change is validated by a dedicated soak round (see the LLM soak
reports).

## Addendum: CI moved to a hosted runner

There is no self-hosted machine in this setup, so the queued
`[self-hosted, horizon]` job could never start. `bundle-compile.yml` now
runs on GitHub-hosted `ubuntu-latest` — the gate is fully headless (no
game client, no display, no licensed assets), so nothing else changes:
same mock-LLM port, same launcher-style pipeline, same honest verdicts.
Cold-start cost is handled with caches (pinned source archive, mixins
tool, cargo target) plus a free-disk-space step for the multi-GB Horizon
check cache. The game-client jobs in port-test.yml (parity / in-game /
modpack sweep) keep their `HORIZON_SELFHOSTED` guard: they genuinely
need a licensed MC copy and stay skipped.

First hosted run (commit `699f90e`, run 42113058855): job green in
9.3 min cold — pinned-archive fetch, mixins built from source, hermetic
mock-LLM port, then the real `cargo check` of `horizon-native-game` +
`horizon-mod-chunkcourier` against horizon@6beacfe9 (`Finished fast
profile [optimized] target(s) in 2m 19s`), report verdict
`compile: pass` ("launcher-style pipeline on horizon@6beacfe9 (API-only,
plain stage: horizon-native-game, cargo check)"). The two stuck
self-hosted runs from before the move were cancelled.
