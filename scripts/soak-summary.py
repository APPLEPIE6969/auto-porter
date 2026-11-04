#!/usr/bin/env python3
"""Aggregate an LLM-soak directory.

Inputs (produced by .github/workflows/llm-soak.yml):
  <dir>/trace.jsonl         one JSON line per LLM call (AUTO_PORTER_LLM_TRACE)
  <dir>/<run>/...-report.json   the port report of each run

Prints the ported rate, repair rate, per-call outcome classes and latency
profile — the evidence base for prompt improvements.
"""
import argparse
import collections
import glob
import json
import os
import sys


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", default="soak")
    args = ap.parse_args()
    d = args.dir

    statuses: collections.Counter = collections.Counter()
    repairs: collections.Counter = collections.Counter()
    failures: collections.Counter = collections.Counter()
    runs = 0
    for report in sorted(glob.glob(os.path.join(d, "*", "*-report.json"))):
        runs += 1
        try:
            with open(report) as fh:
                r = json.load(fh)
        except Exception as e:  # noqa: BLE001
            print(f"unreadable report {report}: {e}")
            continue
        tick = [
            f
            for f in r.get("features", [])
            if "playertickevent" in f.get("feature_id", "")
        ]
        if not tick:
            statuses["MISSING"] += 1
            continue
        t = tick[0]
        statuses[t["status"]] += 1
        if "repaired after" in t.get("detail", ""):
            repairs[t["status"]] += 1
        if t["status"] != "ported":
            for e in r.get("errors", []):
                if "tickevent" in str(e.get("feature", "")):
                    failures[f"{e.get('error_type')}: {str(e.get('message'))[:100]}"] += 1

    outcomes: collections.Counter = collections.Counter()
    errors: collections.Counter = collections.Counter()
    finish: collections.Counter = collections.Counter()
    latencies = []
    calls = 0
    trace = os.path.join(d, "trace.jsonl")
    if os.path.exists(trace):
        with open(trace) as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    v = json.loads(line)
                except Exception:  # noqa: BLE001
                    continue
                calls += 1
                outcomes[v.get("outcome")] += 1
                lat = v.get("latency_ms")
                if isinstance(lat, (int, float)):
                    latencies.append(lat)
                fr = v.get("finish_reason")
                if isinstance(fr, str):
                    finish[fr] += 1
                if v.get("error"):
                    errors[str(v["error"])[:140]] += 1

    def pct(x: int, total: int) -> str:
        return f"{100.0 * x / total:.1f}%" if total else "n/a"

    print("\n===== LLM SOAK SUMMARY =====")
    print(f"runs: {runs}   tick statuses: {dict(statuses)}")
    print(f"first-try repairs seen in details: {dict(repairs)}")
    print(f"llm calls: {calls}   outcomes: {dict(outcomes)}")
    print(f"finish reasons: {dict(finish)}")
    if latencies:
        latencies.sort()
        p = lambda q: latencies[min(len(latencies) - 1, int(len(latencies) * q))]
        print(
            f"latency ms: p50={p(0.5)} p90={p(0.9)} p99={p(0.99)} max={latencies[-1]}"
        )
    for e, c in errors.most_common(10):
        print(f"  call error {c}x: {e}")
    for e, c in failures.most_common(10):
        print(f"  run failure {c}x: {e}")
    ok = statuses.get("ported", 0)
    print(f"PORTED RATE: {ok}/{runs} = {pct(ok, runs)}")
    if runs == 0 and calls == 0:
        print("no data collected")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
