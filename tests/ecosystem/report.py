#!/usr/bin/env python3
"""Turn the nightly sweep output into the markdown status report (spec §8.2)."""
import argparse, glob, json

ap = argparse.ArgumentParser()
ap.add_argument("--output", required=True, help="sweep output dir with *-report.json")
ap.add_argument("--report", required=True)
args = ap.parse_args()

rows = []
for path in sorted(glob.glob(f"{args.output}/*-report.json")):
    r = json.load(open(path))
    rows.append(r)

lines = ["# Nightly ecosystem sweep", "",
         f"Mods processed: **{len(rows)}**", "",
         "| mod | loader | overall | tiers | errors |", "|---|---|---|---|---|"]
for r in rows:
    tiers = ", ".join(f"{k}:{v}" for k, v in r.get("summary", {}).get("tiers", {}).items())
    lines.append(f"| {r.get('mod_id')} | {r.get('loader')} | {r.get('overall')} | {tiers or '-'} | {len(r.get('errors', []))} |")

with open(args.report, "w") as f:
    f.write("\n".join(lines) + "\n")
print(f"wrote {args.report} ({len(rows)} mods)")
