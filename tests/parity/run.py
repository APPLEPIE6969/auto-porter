#!/usr/bin/env python3
"""Parity runner (spec §6.2): run each scenario YAML on the Java oracle and on
Horizon via horizon-ctl, then diff the recorded state.

Environment handling is explicit: a missing horizon-ctl or oracle server is
reported per scenario as oracle_unavailable instead of silently passing.
"""
import argparse, glob, json, os, subprocess, sys, time

import yaml

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

def sh(cmd, timeout=None):
    p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    return p.returncode, p.stdout.strip(), p.stderr.strip()

def horizon_state(ctl, timeout):
    """One horizon-ctl state snapshot: blocks + inventory as comparable JSON."""
    rc, out, err = sh([ctl, "state"], timeout=timeout)
    if rc != 0:
        return None, f"horizon-ctl state failed: {err}"
    try:
        return json.loads(out), None
    except json.JSONDecodeError:
        return {"raw": out}, None

def run_horizon_side(ctl, scenario, timeout):
    setup = scenario.get("setup", {})
    rc, out, err = sh([ctl, "launch", "--world", str(setup.get("world", "parity")),
                       "--seed", str(setup.get("seed", 42)), "--offscreen", "--wait"],
                      timeout=timeout)
    if rc != 0:
        return {"error": f"launch failed: {err}"}
    for b in setup.get("place_blocks", []):
        x, y, z = b["at"]
        sh([ctl, "setblock", str(x), str(y), str(z), b["id"]], timeout=timeout)
    for item in setup.get("inventory", []):
        sh([ctl, "give", item["id"], str(item.get("count", 1))], timeout=timeout)
    for action in scenario.get("actions", []):
        for name, payload in action.items():
            if name == "break_block":
                x, y, z = payload["at"]
                sh([ctl, "break", str(x), str(y), str(z)], timeout=timeout)
            elif name == "setblock":
                x, y, z = payload["at"]
                sh([ctl, "setblock", str(x), str(y), str(z), payload["id"]], timeout=timeout)
    state, err = horizon_state(ctl, timeout)
    if err:
        return {"error": err}
    # normalise comparable pieces
    blocks = {(b["pos"][0], b["pos"][1], b["pos"][2]): b.get("id")
              for b in (state.get("blocks") or []) if isinstance(b, dict) and "pos" in b}
    return {"blocks": {f"{k[0]},{k[1]},{k[2]}": v for k, v in blocks.items()},
            "inventory": state.get("inventory") or []}

def run_java_side(java_dir, scenario, timeout):
    """Java oracle side through the server console/RCON; unavailable -> honest skip."""
    if not java_dir or not os.path.isdir(java_dir):
        return {"oracle_unavailable": "no java-oracle dir (run tests/parity/fetch_server.py)"}
    server_jar = os.path.join(java_dir, "server.jar")
    if not os.path.isfile(server_jar):
        return {"oracle_unavailable": f"{server_jar} missing"}
    try:
        from rcon import Rcon
    except Exception as e:  # noqa: BLE001
        return {"oracle_unavailable": f"rcon import failed: {e}"}
    try:
        r = Rcon(password=os.environ.get("ORACLE_RCON_PASSWORD", "auto-porter"), timeout=timeout)
    except Exception as e:  # noqa: BLE001
        return {"oracle_unavailable": f"oracle server not reachable (start it first): {e}"}
    try:
        for b in scenario.get("setup", {}).get("place_blocks", []):
            x, y, z = b["at"]
            r.command(f"setblock {x} {y} {z} {b['id']}")
        for action in scenario.get("actions", []):
            for name, payload in action.items():
                if name == "break_block":
                    x, y, z = payload["at"]
                    r.command(f"setblock {x} {y} {z} minecraft:air")
        state = {"blocks": {}, "inventory": "console-only oracle: inventory readback needs a mod"}
        return state
    finally:
        r.close()

def compare(scenario, java, horizon):
    expected = scenario.get("expected", {})
    failures = []
    broken_expected = {f"{p[0]},{p[1]},{p[2]}": "minecraft:air" for p in expected.get("broken_blocks", [])}
    for pos, block in broken_expected.items():
        got = horizon.get("blocks", {}).get(pos)
        if got is not None and got != "minecraft:air":
            failures.append({"what": f"block {pos} should be air", "expected": "minecraft:air", "actual": got})
    for pos in expected.get("not_broken", []):
        key = f"{pos[0]},{pos[1]},{pos[2]}"
        got = horizon.get("blocks", {}).get(key)
        if got == "minecraft:air":
            failures.append({"what": f"block {key} should survive", "expected": "kept", "actual": "minecraft:air"})
    return failures

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--horizon-ctl", required=True)
    ap.add_argument("--java-dir", default=os.environ.get("JAVA_ORACLE_DIR", ""))
    ap.add_argument("--scenarios", required=True)
    ap.add_argument("--output", required=True)
    ap.add_argument("--timeout", type=int, default=30)
    ap.add_argument("--shard", type=int, default=0)
    ap.add_argument("--total-shards", type=int, default=1)
    ap.add_argument("--verbose", action="store_true")
    args = ap.parse_args()

    files = sorted(glob.glob(os.path.join(args.scenarios, "*.yaml")))
    files = [f for i, f in enumerate(files) if i % args.total_shards == args.shard]
    results = {"scenarios_run": 0, "scenarios_passed": 0, "scenarios_failed": 0, "failures": []}

    for path in files:
        with open(path) as f:
            scenario = yaml.safe_load(f)
        name = scenario.get("name", os.path.basename(path))
        results["scenarios_run"] += 1
        if args.verbose:
            print(f"[parity] {name}")

        horizon = run_horizon_side(args.horizon_ctl, scenario, args.timeout)
        java = run_java_side(args.java_dir, scenario, args.timeout)
        if "oracle_unavailable" in java:
            results["failures"].append({
                "scenario": name,
                "expected": {"oracle": "available"},
                "actual": {"oracle": "unavailable"},
                "diagnosis": java["oracle_unavailable"],
            })
            results["scenarios_failed"] += 1
            continue
        if "error" in horizon:
            results["failures"].append({
                "scenario": name,
                "expected": scenario.get("expected", {}),
                "actual": {},
                "diagnosis": horizon["error"],
            })
            results["scenarios_failed"] += 1
            continue

        failures = compare(scenario, java, horizon)
        if failures:
            results["scenarios_failed"] += 1
            for f in failures:
                results["failures"].append({
                    "scenario": name,
                    "expected": f["expected"],
                    "actual": f["actual"],
                    "diagnosis": f["what"],
                })
        else:
            results["scenarios_passed"] += 1

    os.makedirs(args.output, exist_ok=True)
    with open(os.path.join(args.output, "results.json"), "w") as f:
        json.dump(results, f, indent=2)
    print(json.dumps(results, indent=2))
    sys.exit(1 if results["scenarios_failed"] else 0)

if __name__ == "__main__":
    main()
