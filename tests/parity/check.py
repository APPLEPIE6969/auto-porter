#!/usr/bin/env python3
"""Fail CI when parity results contain mismatches (spec §8.1)."""
import argparse, glob, json, sys

ap = argparse.ArgumentParser()
ap.add_argument("--results", required=True)
ap.add_argument("--fail-on-mismatch", action="store_true")
args = ap.parse_args()

failed = 0
total = 0
for path in glob.glob(f"{args.results}/**/*.json", recursive=True):
    with open(path) as f:
        data = json.load(f)
    for key in ("scenarios_run", "scenarios_failed"):
        total += data.get("scenarios_run", 0) if key == "scenarios_run" else 0
    failed += data.get("scenarios_failed", 0)

print(f"parity: {total} scenario(s) run, {failed} failure(s)")
if args.fail_on_mismatch and failed:
    sys.exit(1)
