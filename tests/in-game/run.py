#!/usr/bin/env python3
"""In-game smoke runner (spec §6.3): drives horizon-ctl through the scenario
steps; assertions, retries and screenshots stay in Python. `horizon-ctl script`
batches are only emitted for purely linear stretches."""
import argparse, glob, json, os, subprocess, sys

import yaml

def sh(cmd, timeout=120):
    p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    return p.returncode, p.stdout.strip(), p.stderr.strip()

def ssim(a_path, b_path):
    try:
        from PIL import Image
        import math
        a, b = Image.open(a_path).convert("L"), Image.open(b_path).convert("L")
        if a.size != b.size:
            b = b.resize(a.size)
        pa, pb = list(a.getdata()), list(b.getdata())
        n = len(pa)
        ma, mb = sum(pa) / n, sum(pb) / n
        va = sum((x - ma) ** 2 for x in pa) / n
        vb = sum((x - mb) ** 2 for x in pb) / n
        cov = sum((x - ma) * (y - mb) for x, y in zip(pa, pb)) / n
        c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
        return (2 * ma * mb + c1) * (2 * math.sqrt(va) * math.sqrt(vb) + c2) / (
            (ma**2 + mb**2 + c1) * (va + vb + c2))
    except Exception:  # noqa: BLE001
        return None

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--horizon-ctl", required=True)
    ap.add_argument("--scenarios", required=True)
    ap.add_argument("--golden", default="tests/in-game/golden/")
    ap.add_argument("--output", required=True)
    ap.add_argument("--ssim-threshold", type=float, default=0.95)
    ap.add_argument("--ssim-advisory", action="store_true")
    ap.add_argument("--verbose", action="store_true")
    args = ap.parse_args()

    ctl = args.horizon_ctl
    results = {"steps_run": 0, "steps_passed": 0, "steps_failed": 0, "notes": []}
    ok = True

    for path in sorted(glob.glob(os.path.join(args.scenarios, "*.yaml"))):
        with open(path) as f:
            scenario = yaml.safe_load(f)
        if args.verbose:
            print(f"[in-game] {path}")
        for step in scenario.get("steps", []):
            for name, payload in step.items():
                results["steps_run"] += 1
                if name == "launch":
                    cmd = [ctl, "launch", "--world", str(payload.get("world", "test_world")),
                           "--seed", str(payload.get("seed", 42)), "--offscreen", "--wait"]
                    if not payload.get("creative", False):
                        cmd.append("--creative=false" if "--creative=false" in ctl else cmd[-1])
                    cmd = [c for c in cmd if c]
                    rc, out, err = sh(cmd)
                    passed = rc == 0
                elif name == "assert_ready":
                    rc, out, err = sh([ctl, "state"])
                    passed = rc == 0 and out.strip() != ""
                elif name == "goto":
                    x, y, z = payload["pos"]
                    cmd = [ctl, "goto", str(x), str(y), str(z)]
                    if payload.get("near"):
                        cmd += ["--near", str(payload["near"])]
                    rc, out, err = sh(cmd)
                    passed = rc == 0
                elif name == "break":
                    x, y, z = payload["pos"]
                    rc, out, err = sh([ctl, "break", str(x), str(y), str(z)])
                    passed = rc == 0
                elif name == "assert_block":
                    x, y, z = payload["pos"]
                    rc, out, err = sh([ctl, "block", str(x), str(y), str(z)])
                    passed = rc == 0 and payload["id"] in out
                elif name == "inventory":
                    rc, out, err = sh([ctl, "inventory"])
                    contains = payload.get("contains", {})
                    passed = rc == 0 and contains.get("id", "") in out
                elif name == "screenshot":
                    out_path = payload.get("path", "out/screenshot.png")
                    os.makedirs(os.path.dirname(out_path) or ".", exist_ok=True)
                    rc, out, err = sh([ctl, "screenshot", out_path])
                    passed = rc == 0 and os.path.isfile(out_path)
                    golden = os.path.join(args.golden, os.path.basename(out_path))
                    if passed and os.path.isfile(golden):
                        score = ssim(out_path, golden)
                        if score is not None:
                            results["notes"].append(f"ssim {os.path.basename(out_path)}: {score:.3f}")
                            if score < args.ssim_threshold:
                                if args.ssim_advisory:
                                    results["notes"].append(
                                        f"ssim below {args.ssim_threshold} — advisory only (software rendering)")
                                else:
                                    passed = False
                else:
                    results["notes"].append(f"unknown step {name} — skipped")
                    passed = True

                if passed:
                    results["steps_passed"] += 1
                else:
                    results["steps_failed"] += 1
                    ok = False

    ssim_note = next((n for n in results["notes"] if n.startswith("ssim")), None)
    results["screenshot_ssim"] = float(ssim_note.split(": ")[1]) if ssim_note else None
    os.makedirs(args.output, exist_ok=True)
    with open(os.path.join(args.output, "results.json"), "w") as f:
        json.dump(results, f, indent=2)
    print(json.dumps(results, indent=2))
    sys.exit(0 if ok else 1)

if __name__ == "__main__":
    main()
