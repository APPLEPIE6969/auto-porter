# LLM soak round 8 (2026-11-06): contract v3.4 validated — Sync-risk 26 → 0

Setup: 100 runs (5 shards × 20) of the chunkcourier `PlayerTickEvent`
fixture through the real NVIDIA NIM model (`minimaxai/minimax-m3.1-flash`)
on contract v3.4 (the `Send + Sync` state requirement), run
[42258366615](https://github.com/APPLEPIE6969/auto-porter/actions/runs/42258366615),
analyzed with the upgraded `soak-summary.py` (per-run traces, first-try,
Sync-risk scan — fixed this round against round 7's schema drift).

## Headline

- **Sync-risk bundles: 0/100** (round 7, on v3.3: 26/100). The round 7
  finding — `Cell<...>` tick state failing the compile gate with E0277 — is
  extinct in one round of the contract teaching the atomics idiom.
- **Ported: 100/100** (fourth consecutive clean sheet at the 100-run scale).
- First-try: 67/100. The failure mix is pure external contention: **62
  HTTP 429s** (round 7: 26 — this round overlapped the in-game dispatch
  workflow's real NIM legs) and 1 timeout (round 7: 33). Latency when NIM
  answers: p50 ≈ 70 s, min ≈ 29 s. All 100 accepted calls finished `stop`.

## Body shape across rounds

- 65 broadcast sites / 35 tell sites (round 7: 89/11) — both channels are
  contract-legal choices, and as of today both are **asserted** in the
  in-game dispatch workflow (broadcast against the server's log section,
  tell against the receiving client's `[CHAT]` log — see the addendum in
  `in-game-real-port-2026-11-06.md`). The drift between rounds is sampling
  variance, not a contract regression; nothing to fix.
- 94/100 bodies guard on `acting_player()`.

## Where this leaves the standing directive

The prompt (contract v3.4) has no measured gap left at the 100-run scale:
acceptance 100/100, the one known gate-failure class eliminated, both
message channels observable and asserted in the booted game. The natural
next iterations, in order of value:

1. **Gate-truth in the soak loop**: run the compile gate over each soak
   bundle on CI (unlimited minutes make it viable; the 9.9 GB local wall
   still blocks it) so future rounds report gate-pass rate directly instead
   of a static proxy.
2. A soak round against a second fixture (a Flow/cancel hook or an event
   with fields) — the current 100-run scale exercises one hook shape.
3. Scheduled rounds (the workflow is `workflow_dispatch`; a cron trigger is
   a one-line addition) to watch for drift without manual dispatches.
