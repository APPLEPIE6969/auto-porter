#!/usr/bin/env python3
"""Merge every downloaded CI result artifact into report.md (spec §8.1 report job)."""
import argparse, glob, json, os

ap = argparse.ArgumentParser()
ap.add_argument("--input", required=True)
ap.add_argument("--output", required=True)
args = ap.parse_args()

sections = []
for path in sorted(glob.glob(f"{args.input}/**/*.json", recursive=True)):
    name = os.path.relpath(path, args.input)
    try:
        data = json.load(open(path))
    except Exception as e:  # noqa: BLE001
        sections.append(f"## {name}\n\n(unreadable: {e})\n")
        continue
    sections.append(f"## {name}\n\n```json\n{json.dumps(data, indent=2)[:4000]}\n```\n")

with open(args.output, "w") as f:
    f.write("# auto-porter CI report\n\n" + "\n".join(sections))
print(f"wrote {args.output} from {len(sections)} result file(s)")
