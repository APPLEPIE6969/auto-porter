#!/usr/bin/env python3
"""Aggregate the modpack-sweep port results into one summary (spec §8.1)."""
import argparse, glob, json

ap = argparse.ArgumentParser()
ap.add_argument("--output", required=True, help="port-pack output dir")
ap.add_argument("--report", required=True)
args = ap.parse_args()

rows = []
for path in sorted(glob.glob(f"{args.output}/*-report.json")):
    with open(path) as f:
        r = json.load(f)
    rows.append({
        "mod_id": r.get("mod_id"),
        "loader": r.get("loader"),
        "overall": r.get("overall"),
        "tiers": r.get("summary", {}).get("tiers", {}),
        "errors": len(r.get("errors", [])),
        "carry": r.get("data_carry"),
    })

summary = {"mods": len(rows), "pass": sum(1 for r in rows if r["overall"] == "PASS"),
           "pass_with_warnings": sum(1 for r in rows if r["overall"] == "PASS_WITH_WARNINGS"),
           "failed": sum(1 for r in rows if r["overall"] in ("PARTIAL", "FAILED")),
           "rows": rows}
with open(args.report, "w") as f:
    json.dump(summary, f, indent=2)
print(json.dumps({k: v for k, v in summary.items() if k != "rows"}, indent=2))
