#!/usr/bin/env python3
"""Local validation for scripts/in_game_markers.py (zip-scan + hard-fail)."""

import json
import subprocess
import sys
import zipfile
from pathlib import Path

TMP = Path("/tmp/marker-test")
SCRIPT = Path("/home/z/my-project/auto-porter/scripts/in_game_markers.py")

BROADCAST_BODY = """
pub struct Mod;
impl Hooks for Mod {
    fn player_tick(&self, host: &mut dyn Host, _e: &mut PlayerTickEvent) {
        host.broadcast(format!("[chunkcourier] player_tick dispatched (in-game tick #{tick})").into());
        host.tell(player.uuid, Text::from("chunkcourier: parcels ticked"));
    }
}
"""
TELL_ONLY_BODY = """
impl Hooks for Mod {
    fn player_tick(&self, host: &mut dyn Host, _e: &mut PlayerTickEvent) {
        host.tell(player.uuid, Text::from("chunkcourier: parcels ticked"));
    }
}
"""


def make_hnmod(dest: Path, body: str) -> None:
    with zipfile.ZipFile(dest, "w") as z:
        z.writestr("horizon-native.json", "{}")
        z.writestr("mixins/chunkcourier/horizon-mixin.toml", "[api]")
        z.writestr("mixins/chunkcourier/lib.rs", body)


def run(out: Path, tag: str) -> tuple[int, str, str]:
    r = subprocess.run(
        [sys.executable, str(SCRIPT), str(out), "--markers", str(out / "m.txt"), "--json", str(out / "j.json")],
        capture_output=True, text=True,
    )
    print(f"--- {tag}: exit {r.returncode}")
    print(r.stdout.strip() or "(no stdout)")
    if r.stderr.strip():
        print("stderr:", r.stderr.strip())
    return r.returncode, r.stdout, r.stderr


# Case 1: bundle with a broadcast body — marker must be found inside the zip.
out1 = TMP / "broadcast"; out1.mkdir(parents=True)
make_hnmod(out1 / "chunkcourier.hnmod", BROADCAST_BODY)
code, stdout, _ = run(out1, "broadcast-in-hnmod")
j = json.loads((out1 / "j.json").read_text())
assert code == 0, "broadcast-in-hnmod must succeed"
assert j["files_scanned"] == 1 and j["broadcast_calls"] == 1 and j["tell_calls"] == 1, j
assert j["broadcast_markers"] == ["[chunkcourier] player_tick dispatched (in-game tick #"], j
assert j["tell_literals"] == ["chunkcourier: parcels ticked"], j
assert (out1 / "m.txt").read_text().strip().startswith("[chunkcourier] player_tick dispatched"), "markers.txt"
print("PASS broadcast-in-hnmod\n")

# Case 2: tell-only body — honest empty markers, scanned > 0.
out2 = TMP / "tellonly"; out2.mkdir(parents=True)
make_hnmod(out2 / "chunkcourier.hnmod", TELL_ONLY_BODY)
code, stdout, _ = run(out2, "tell-only")
j = json.loads((out2 / "j.json").read_text())
assert code == 0 and j["files_scanned"] == 1 and j["log_observable"] is False and j["broadcast_markers"] == [], j
assert "tell-only" in stdout, stdout
print("PASS tell-only\n")

# Case 3: empty dir — hard fail, exit 2, no vacuous evidence.
out3 = TMP / "empty"; out3.mkdir(parents=True)
code, _, stderr = run(out3, "zero-scan")
assert code == 2, f"zero-scan must exit 2, got {code}"
assert "nothing was scanned" in stderr, stderr
assert not (out3 / "j.json").exists(), "no json must be written on failure"
print("PASS zero-scan (hard fail)\n")

print("ALL MARKER-EXTRACTOR CASES PASS")
