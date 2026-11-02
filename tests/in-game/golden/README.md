# In-game golden baselines

Baseline screenshots for the in-game smoke scenarios (spec §6.3) are
**recorded on the self-hosted `horizon` runner**, not committed: they are
binary PNGs tied to a licensed Minecraft 26.3 copy, and redistributing them
in this repository is not appropriate.

To (re)record baselines on the runner:

```sh
python3 tests/in-game/run.py --horizon-ctl ~/.horizon/horizon-ctl \
    --scenarios tests/in-game/smoke/ --golden tests/in-game/golden/ \
    --output tests/in-game/results/ --record-golden
```

The `--ssim-threshold` comparison in `run.py` only fires when a baseline for
the same scenario exists; with this directory empty, screenshot similarity is
reported as advisory only rather than failing the run.
