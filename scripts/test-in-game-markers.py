#!/usr/bin/env python3
"""Local validation for scripts/in_game_markers.py (zip-scan + hard-fail)."""

import json
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

TMP = Path("/tmp/marker-test")
SCRIPT = Path("/home/z/my-project/auto-porter/scripts/in_game_markers.py")
shutil.rmtree(TMP, ignore_errors=True)

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

# Case 4: the wrapper's cfg(test) SmokeHost double — its `fn broadcast(` /
# `fn tell(` DEFINITIONS and the smoke's "unknown panic" fallback must not
# be mis-attributed to the body (the real leg's first honest scan showed
# exactly this false positive).
WRAPPER_LIB_RS = '''
#![deny(warnings)]
use horizon_client::mod_api::*;

pub struct PortedHooks { body: body::ChunkCourier }
impl horizon_client::mod_api::Hooks for PortedHooks {
    fn player_tick(&self, host: &mut dyn Host, event: &mut PlayerTickEvent) {
        self.body.player_tick(host, event)
    }
}

#[cfg(test)]
mod dispatch_smoke {
    use super::*;
    struct SmokeHost;
    impl horizon_client::mod_api::Host for SmokeHost {
        fn broadcast(&self, _line: horizon_client::mod_api::Text) {}
        fn tell(&mut self, _uuid: u128, _line: horizon_client::mod_api::Text) -> bool {
            false
        }
    }
    #[test]
    fn player_tick_dispatches() {
        let mods: Vec<Box<dyn horizon_client::mod_api::Hooks>> = vec![hooks()];
        let dispatch = std::panic::AssertUnwindSafe(|| {
            let mut host = SmokeHost;
            horizon_client::mod_api::dispatch_player_tick(&mut host, &mods);
        });
        if let Err(panic) = std::panic::catch_unwind(dispatch) {
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&'static str>().copied())
                .unwrap_or("unknown panic");
            assert!(message.contains("Host::"), "delegation panicked: {message}");
        }
    }
}
'''
out4 = TMP / "wrapper-double"; out4.mkdir(parents=True)
with zipfile.ZipFile(out4 / "chunkcourier.hnmod", "w") as z:
    z.writestr("horizon-native.json", "{}")
    z.writestr("mixins/chunkcourier/engine/src/lib.rs", WRAPPER_LIB_RS)
    z.writestr("mixins/chunkcourier/engine/src/body.rs", BROADCAST_BODY)
code, stdout, _ = run(out4, "wrapper-double")
j = json.loads((out4 / "j.json").read_text())
assert code == 0, "wrapper-double must succeed"
assert j["files_scanned"] == 2, j
# The body itself carries one broadcast AND one tell; the double's
# definitions must add nothing of their own.
assert j["broadcast_calls"] == 1, f"only the body's broadcast is a call site: {j}"
assert j["tell_calls"] == 1, f"exactly the body's tell, the double's fn tell excluded: {j}"
assert j["broadcast_markers"] == ["[chunkcourier] player_tick dispatched (in-game tick #"], j
assert j["tell_literals"] == ["chunkcourier: parcels ticked"], j
assert "unknown panic" not in j["tell_literals"] + j["broadcast_markers"], (
    "the smoke double's panic fallback must not appear as a marker")
print("PASS wrapper-double (definitions excluded)\n")

print("ALL MARKER-EXTRACTOR CASES PASS")
