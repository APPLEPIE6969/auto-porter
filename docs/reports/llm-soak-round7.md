# LLM soak round 7 (2026-11-06): 100/100 ported, contract v3.3 — and the
# metric it exposed

Setup: 100 runs (5 shards × 20) of the no-template chunkcourier
`PlayerTickEvent` fixture through the real NVIDIA NIM model
(`minimaxai/minimax-m3.1-flash`) on contract v3.3, run
[42247284333](https://github.com/APPLEPIE6969/auto-porter/actions/runs/42247284333),
analyzed from the five shard artifacts with `scripts/soak7_deep.py`
(machine summary: `llm-soak-round7.json`).

## Headline

- **Ported: 100/100** (third consecutive clean sheet at the 100-run scale).
- **First-try: 71/100**; the retry loop recovered the rest — 33 timeouts
  ("LLM call exceeded 180s") and 26 HTTP 429s (the in-game real legs ran
  concurrently with this soak). Every accepted call finished with `stop`.
- Latency of accepted calls: p50 ≈ 99 s, p90 ≈ 149 s, max 172 s — the
  endpoint was hot this round; the 180 s timeout is close to the tail.
- **v3.3 is visibly steering**: 89/100 bodies announce via `broadcast`
  (log-observable) and 99/100 guard on `acting_player()` — the two
  API-REALITIES behaviors the section teaches.

## The finding the round exists for

The ported rate measures LLM acceptance, not gate cleanliness. A static scan
of every emitted module for state types that fail the `Hooks` trait's `Sync`
bound found **26/100 bundles holding `Cell<...>` tick state** — `Cell` is
`Send` but not `Sync`, so all 26 would fail the compile gate with E0277,
exactly the failure the in-game real leg hit on run 42252868531 (its accepted
body used `ticks: std::cell::Cell<u32>`; the two booted-green bodies used
`AtomicU32`). No `RefCell`/`Rc`/`UnsafeCell` appeared — `Cell` is the model's
natural counter choice, and nothing in the contract said otherwise.

Effective gate-clean rate this round: **≈ 74/100** behind the 100/100 ported
rate.

## Action taken

Contract **v3.4** (`port-code/src/llm.rs`) extends API-REALITIES with the
state-type requirement: hook state must be `Send + Sync` (`Cell`, `RefCell`,
`Rc`, `UnsafeCell` fail the bound → E0277); counters belong in
`AtomicU32`/`AtomicU64`/`AtomicBool` with `fetch_add`/`load`,
`Ordering::Relaxed` — the idiom both green boots used.

## Next round's success metric

Round 8 (same 100-run scale) measures v3.4; success = Sync-risk bundles
dropping from 26 toward 0 with the ported rate held at 100. The deeper
metric — an actual compile-gate pass rate across soak bundles — needs the
gate run per bundle, which the 9.9 GB local wall still blocks; on CI with
unlimited minutes it is now an option (a follow-up workflow, not yet built).

Standing note: `soak-summary.py`'s call-level detail section reads the old
trace schema (prints zeros against current traces); `soak7_deep.py` carries
the corrected parsing and should be folded back into it.
