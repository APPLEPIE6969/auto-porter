# In-game dispatch: the §5.2 last mile, closed without a license (2026-11-06)

Run [42222447944](https://github.com/APPLEPIE6969/auto-porter/actions/runs/42222447944)
(commit `3be7bc82`) — every step green. The game booted headless on a
GitHub-hosted `ubuntu-latest` runner with resources prepared from Mojang's
public distribution endpoints, no Minecraft account anywhere, and the
booted game's own tick loop dispatched the ported mod's hook, observed in
the server log:

```
[13:21:29] [Server thread/INFO]: [chunkcourier] player_tick dispatched (in-game tick #0)
[13:21:34] [Server thread/INFO]: [chunkcourier] player_tick dispatched (in-game tick #100)
[13:21:39] [Server thread/INFO]: [chunkcourier] player_tick dispatched (in-game tick #200)
[13:21:44] [Server thread/INFO]: [chunkcourier] player_tick dispatched (in-game tick #300)
[13:21:49] [Server thread/INFO]: [chunkcourier] player_tick dispatched (in-game tick #400)
[13:21:54] [Server thread/INFO]: [chunkcourier] player_tick dispatched (in-game tick #500)
[13:21:59] [Server thread/INFO]: [chunkcourier] player_tick dispatched (in-game tick #600)
```

Seven markers, exactly 100 ticks (5 s) apart — the game ran at full 20 TPS
under software rendering. `state.json` shows a live player in a creative
overworld; `ready.json` shows loaded chunks and a running server thread.
The dispatch is the real game loop calling the real `dispatch_player_tick`
of the staged `horizon-native-game` build — the same call path Horizon's
own hook point uses — not a test double.

## What made it possible: the "license wall" was not load-bearing

The earlier recon had stopped at "resources come from your own Minecraft
installation". Reading `horizon-prepare` at the pin showed it takes each
pinned input individually (`--version-json`, `--client-jar`,
`--libraries-dir`, `--server-jar`; each "replaces what --minecraft-dir
would give") and verifies sha1s against `research-lock.json`. Every one of
those files is publicly served by Mojang with no authentication. "Horizon
never downloads Mojang's files" is the EULA posture of its launcher, which
downloads them for a signed-in owner — not a technical restriction of the
preparer or the game. And the game boots with no session by design ("A
session the game can't read is reported on stderr; the game still starts"
— its launch guide §6).

## The pipeline `in-game-dispatch.yml` runs

1. Port the fixture hermetically (mock LLM; the body broadcasts a marker
   every 100 ticks — a real implementation, as the LLM writes for this
   fixture).
2. `verify --compile` with `AUTO_PORTER_GATE_BUILD=1`: launcher-style
   staging and `cargo build --locked --profile fast --no-default-features
   -p horizon-native-game` of the staged tree (cached; also the dispatch
   smoke).
3. `scripts/fetch_mojang_inputs.py`: the pinned 26.3 version JSON by its
   content address (stable against Mojang's republishes), client jar,
   server jar, all 114 libraries — 220.4 MB, every sha1 verified against
   the pins.
4. `horizon-prepare` (built at the pin) prepares the resources offline:
   exporters compiled with a JDK 25 and run against the pinned jars;
   sounds/languages/panorama/packs/fonts are optional extras the minimal
   set omits (the game boots silent). `--check` passes with only
   extras-gaps.
5. `horizon-ctl` (built in the staged tree) launches the staged binary:
   `launch --game <bin> --resources <prepared> --offscreen --game-dir ...
   --home ...` under Xvfb, no session; then `open dispatch-world 20261106
   creative`, `wait-ready`, 30 s of live ticks, `state`, `quit`.
6. Assert ≥ 3 marker lines in `instance/logs/latest.log`.

## What the boot log proves

- `resources loaded in 2.3s; 35723 block states, 1658 items, 67 biomes` —
  the offline-prepared resources are accepted in full.
- `no sounds ... (built without the 'audio' feature)` — the launcher's
  no-ALSA engine target, as intended.
- `control socket ...` — ctl's scripting channel live.
- The marker lines — hook dispatch in the booted game.

## Debugging trail (each found by evidence, fixed, re-run)

1. The staged package is `horizon-native-game` but its **binary** is named
   `horizon-client` (the launcher launches it as the client).
2. The cached source archive's root **is** the repo (a `/horizon` suffix
   path failed).
3. `--check` gap parsing: bash `case` alternatives after the first needed
   their own `*` prefix.
4. `horizon-ctl` is the bin target of the `horizon-client-lib` package, not
   a package of its own (`-p horizon-client-lib --bin horizon-ctl`).
5. YAML folds a trailing backslash literally — one-line cargo invocation.
6. The launched game needs `--resources` explicitly (an explicit `--game`
   has no prepared install to infer them from).
7. winit's X11 event loop needs `libxkbcommon-x11-0` on the runner.

## Artifacts

- Evidence dataset: `download/in-game-dispatch-2026-11-06/` (`latest.log`
  with the markers, `state.json`, `ready.json`, `launch.json`, `check.txt`,
  `prepare.jsonl`, `chunkcourier-verification.json`).
- CI artifact `in-game-dispatch-evidence` on run 42222447944.
