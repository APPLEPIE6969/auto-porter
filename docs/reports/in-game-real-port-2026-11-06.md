# In-game dispatch, real-port leg: the milestone closed (2026-11-06)

## What closed

§5.2's last mile in its true final form: **a port produced by the real NVIDIA
NIM model was booted inside the pinned Horizon game on a hosted runner, the
game's own tick loop dispatched the ported `player_tick` hook, and the
assertion counted the body's own observable in the server's log.** No mock
body in the `real` leg — the mock leg runs alongside as the hermetic control.

Evidence, run [42244787748](https://github.com/APPLEPIE6969/auto-porter/actions/runs/42244787748)
(contract v3.3's first real boot, `real` leg):

- The server log carries the body's periodic announcement on a perfect 20-tick
  cadence at 20 TPS — 31 lines, `chunkcourier: parcels ticked 20` through
  `… 620` (archived at `download/in-game-real-port-2026-11-06/`).
- State polls 20 s apart: ticks 21 → 430 (Δ409 ≥ 300), world `dispatch-world`
  open, player present — the game kept simulating with the port live.
- The trace holds exactly one LLM call: 81 s, `finish_reason=stop`,
  first-try acceptance (no repair), and the accepted body (verbatim, from
  `llm-trace.jsonl`):

```rust
fn player_tick(&self, host: &mut dyn Host, _event: &mut PlayerTickEvent) {
    if host.acting_player().is_none() {
        return;
    }
    let ticks = self
        .ticks
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .wrapping_add(1);
    if ticks % 20 == 0 {
        host.broadcast(Text::literal(format!(
            "chunkcourier: parcels ticked {ticks}"
        )));
    }
}
```

Two v3.3 behaviors are visible in that body. It guards on
`host.acting_player()` — the per-player-tick idiom the pin's source verifies
(the single-player owner joins the integrated hub as its first guest and
ticks inside `as_guest`, so the slot is `Some`); the 31 delivered messages
with exactly one player in the world confirm it fired. And it announces the
periodic line via `broadcast` — under v3.2 (soak round 5) the model chose
`tell` for this same fixture, which is player-HUD-private and log-invisible;
the API-REALITIES section added in v3.3 (`broadcast` is the only Host call
the pinned build writes to the server's log) changed the choice, and that
choice is what made the in-game assertion possible at all.

Run [42248963891](https://github.com/APPLEPIE6969/auto-porter/actions/runs/42248963891)
repeated the leg after the extractor fixes below: 4 emitted sources scanned
inside the bundle, marker `chunkcourier: parcels ticked` matched 33 log
lines, the game again simulated through the polls (green). Its trace also
records the leg's resilience: two consecutive `429 Too Many Requests` from
NIM — the soak round 7 shards were hitting the endpoint in parallel — then a
successful 136 s completion that still booted green.

## Two failures on the way, both autopsied

**1. Parse-time workflow rejection (`503e2c2`, run 42243749925).** The
restructured workflow failed in the same second it was created, with zero
jobs and no logs. Root cause: the job-level `if` referenced `env` (and
`matrix`) — contexts that are not available in job-level conditions (only
`github`, `needs`, `vars`, `inputs` are), so GitHub rejected the whole file
before creating any job. Local YAML parsing never could see context
availability; **actionlint 1.7.12 reproduces the exact failure on the
pre-fix file and passes the fix** — it is now the workflow lint of record.
The fix (`4f0f498`) gates the real leg at step level, where `env` is
available, and leaves a no-key real leg as an explicit notice step plus
visible skips.

**2. A vacuous pass inside the honesty machinery (run 42244787748, fixed in
`47bdfee`).** The real leg's `markers.json` recorded `files_scanned: 0`: the
extractor rglob'd loose `.rs` files, but the pipeline emits sources as zip
members (`mixins/<id>/engine/src/*.rs` inside the `.hnmod`, per
`assemble_bundle`). The assert then fell into the "tell-only body" narrative
*without ever reading the body* — while the server log actually carried the
31 broadcast markers. The leg's green was real (the log, polls, and compile
gate were genuine) but the marker assert verified nothing. Fix: the
extractor scans `.rs` members of every `*.hnmod` in the output dir, and
**exits 2 when nothing was scanned** — "scanned nothing" is an extraction
error, not evidence of a tell-only body.

**2b. Definition-vs-call-site (`1ef07d8`, test fix `43ee901`).** Validating
the fixed extractor against a locally reproduced bundle exposed a second
false positive: the wrapper's `#[cfg(test)]` smoke module defines the
SmokeHost double (`fn broadcast(&self, …)`, `fn tell(&mut self, …)`), and
the call regex matched those definitions, attributing the smoke's next
literal — `"unknown panic"`, the panic-payload fallback — to the body
(`broadcast_calls: 2` where the body made one call). `(?<!fn )` restricts
the scan to call sites; the live log assert was never affected (the double's
literal is `cfg(test)`-only and cannot appear in a booted game's log).
`scripts/test-in-game-markers.py` pins all four shapes: broadcast-in-bundle,
tell-only, zero-scan hard fail, wrapper-double.

## State

- `main` = `43ee901`; the three push-triggered workflows are green on
  `47bdfee` (Bundle Compile, Port Test, In-Game Dispatch both legs).
- Soak round 7 (100 runs, 5 shards × 20, contract v3.3) dispatched and
  running; results land in a separate round report.
- Standing item: the PAT that made this session possible is pasted in chat
  history — rotate it once the session's push needs are done.
