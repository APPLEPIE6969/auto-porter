# §5.2 dispatch smoke + last-mile recon (2026-11-06)

Follow-up to `compile-gate-2026-11-06.md`: the compile gate proved the
emitted bundle type-checks against real Horizon; this round closes the
dispatch half of the §5.2 last mile as far as an unlicensed environment
allows, and pins the exact blocker for the rest.

## Last-mile recon (`horizon@6beacfe9`)

- The game's run target for a trait mod is the launcher-staged
  `horizon-native-game` bin (`main` → `mod_i::hooks()` →
  `horizon_client::entry::main_with`).
- The client is headless-capable: `--offscreen`, `HORIZON_NO_AUDIO=1`,
  `--dedicated` (same binary as a server), `horizon-ctl launch --control`
  scripts a running game. `horizon-ctl` is a bin of `horizon-client-lib`.
- The hard blocker is resources, not display or auth plumbing:
  `horizon-prepare` derives them "from your own Minecraft Java Edition"
  (`--minecraft-dir` / `--client-jar` / `--assets-dir`), writing
  `.local/horizon-client/<version>/horizon-resources.json`; without
  prepared resources the game stops on an error once its folders exist
  (upstream's `launch_api` test uses exactly this as its error fixture).
- `player_tick` fires "once a tick for every player in the world" — a
  live player is required even in dedicated mode.

Conclusion: the in-game step needs a legitimately licensed MC Java
installation. Nothing else about it is cloud-infeasible. Documented in
§8 as blocked-on-input, not hand-waved.

## Dispatch smoke (what replaces the un-runnable boot test)

`mod_api` events are `#[non_exhaustive]`: outside the game, the only way
to dispatch is the `dispatch_*` free functions — exactly what the game's
own hook points call (each is one line, e.g. `native_player_tick` →
`mod_api::dispatch_player_tick(self, mod_api::mods())`).

- `scripts/extract-hook-signatures.py` now captures each
  `dispatch_<name>` parameter list into `dispatch_args`
  (hooks/hook-signatures.json regenerated at the pin). Ground truth
  matters: 12 hooks have empty event structs but only **6** have a
  dispatch callable as `(host, mods)` — `block_receive_game`, for one,
  takes five extra args despite its empty event.
- The emitter writes a `#[cfg(test)] mod dispatch_smoke` into every
  engine crate: one test per WIRED hook in that class, building the real
  `hooks()` list and running it through the real
  `horizon_client::mod_api::dispatch_*` (`SmokeHost` implements `Host`,
  all-default like upstream's `mod_api_hooks.rs` test).
- The gate (§3 step 7) runs `cargo test --locked --profile fast
  --no-default-features -p <api crate> --lib` on the staged tree after a
  clean check; a smoke failure fails the gate, and a bundle with no
  synthesizable hook says `no dispatch smoke` instead of inventing
  coverage.
- New knob `AUTO_PORTER_GATE_WORK`: pins the gate's work dir so repeated
  runs reuse cargo fingerprints (they key on absolute paths; a mktemp
  stage re-checks the whole tree every time).

For the chunkcourier fixture the emitted module is one test,
`player_tick_dispatches` — cfg(test)-only, invisible to the launcher's
compiled crate.

## LLM soak round 6 (contract v3.2, 100 runs)

20/shard × 5 shards, real NVIDIA NIM, same fixture. **100/100 ported,
0 output rejections, 0 repair-loop activations** — 86/100 accepted on
the first response; the other 14 needed only infra retries (11× 429,
4× timeouts, all absorbed by the per-request attempt budget; latency
p50 92 s / p90 151 s). Notable: with the fixture Java source supplied,
the model writes real implementations (acting player, periodic messages)
rather than empty stubs, so bare `host` appears where the body uses it —
the underscore rule (v3.2) correctly applies only to unused params, and
the validator passed all 100. Evidence: `soak-round-6/` (per-run traces
+ reports), run 42121484021.

## Known limitation of this proof

The sandbox this ran in has a 9.9 GB rootfs; the warm pinned-tree check
needs ~6.5 GB of target dir on top of the toolchain and registry, which
no longer fits alongside it (the full gate passed locally last session
on a fresher sandbox). The dispatch smoke's execution proof therefore
runs where the compile gate lives anyway: `bundle-compile.yml` on
GitHub-hosted `ubuntu-latest`, first push after this commit. The
emission side is unit-tested (19 port-code tests incl. 4 new:
class-membership, empty-module cases, cfg(test)-containment, unwired
warnings).
