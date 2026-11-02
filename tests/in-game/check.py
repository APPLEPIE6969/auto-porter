#!/usr/bin/env python3
"""Fail CI when in-game smoke results contain failures (spec §8.1)."""
import argparse, glob, json, sys

ap = argparse.ArgumentParser()
ap.add_argument("--results", required=True)
ap.add_argument("--fail-on-mismatch", action="store_true")
args = ap.parse_args()

failed = sum(json.load(open(p)).get("steps_failed", 0)
             for p in glob.glob(f"{args.results}/**/*.json", recursive=True))
print(f"in-game: {failed} failed step(s)")
if args.fail_on_mismatch and failed:
    sys.exit(1)
