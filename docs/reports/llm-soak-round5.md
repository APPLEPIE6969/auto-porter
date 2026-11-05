# LLM soak round 5 — contract v3.2 (underscore rule), 2026-11-06

Protocol identical to rounds 3/4: 20 real-NVIDIA runs (5 shards × 4,
workflow run `42100487026`), the chunkcourier `player_tick` fixture, traces
per run. Round 5 is the validation round for **contract v3.2** — the
few-shot now writes `_host` (Horizon's own `mod_api_hooks.rs` style) and the
new `unused_param_issue` validator rejects unread non-underscored
parameters, both consequences of the compile gate's first swing (the crate
builds with `#![deny(warnings)]`).

## Result: 20/20 ported, 0 output rejections

| metric | round 3 (v3) | round 4 (v3.1) | round 5 (v3.2) |
|---|---|---|---|
| runs ported | 20/20 | 20/20 | **20/20** |
| accepted on the first model response | 16/20 | 20/20 | **16/20** |
| output rejections (validator → repair) | 4 (`-> Flow` invented) | 0 | **0** |
| signature conformance | 20/20 | 20/20 | **20/20** |
| underscore-conformant accepted code | n/a (gate found it) | n/a | **20/20** |
| infra retries (429/timeout, absorbed) | some | 3× 429 | **3× 429 + 1× 180 s timeout** |

The four non-first-try runs in round 5 are infrastructure, not the model:
every retry prompt would carry a `PREVIOUS ATTEMPT REJECTED` section — zero
do. The repair loop never fired. Every accepted call (20/20) carries
`_host: &mut dyn Host` — the model picked the convention up from the
few-shot alone; the new validator never had to reject anything.

Latency p50 79 s / p90 143 s (reasoning effort high; same band as rounds 2–4).

Raw numbers re-derived from the per-run traces (`llm-soak-round5.json`, per
call detail in `llm-soak-round5-calls.json/`): 24 calls — 20 `code`, 3
failed with the provider's own 429 body, 1 timed out at 180 s and was
retried successfully. Nothing invented, nothing faked.
