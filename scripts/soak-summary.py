#!/usr/bin/env python3
"""Aggregate an LLM-soak directory.

Inputs (produced by .github/workflows/llm-soak.yml):
  <dir>/<run>/trace.jsonl       one JSON line per LLM call per run
                                (AUTO_PORTER_LLM_TRACE; a single top-level
                                <dir>/trace.jsonl is still accepted)
  <dir>/<run>/...-report.json   the port report of each run
  <dir>/<run>/*.hnmod           the emitted bundle (scanned for state types
                                that fail the Hooks Sync bound)

Prints the ported rate, first-try rate, per-call outcome classes, latency
profile and the Sync-risk count — the evidence base for prompt improvements.
"""
import argparse
import collections
import glob
import json
import os
import re
import sys
import zipfile

UNSYNC_RE = re.compile(r"\b(Cell|RefCell|Rc|UnsafeCell)\s*<")


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
    runs_with_trace = 0
    first_try = 0
    # Per-run traces are the workflow's layout (AUTO_PORTER_LLM_TRACE points
    # at <run>/trace.jsonl); a single top-level file is the old shape.
    trace_files = sorted(glob.glob(os.path.join(d, "*", "trace.jsonl")))
    top = os.path.join(d, "trace.jsonl")
    if os.path.exists(top):
        trace_files.append(top)
    for tf in trace_files:
        entries = []
        with open(tf) as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    v = json.loads(line)
                except Exception:  # noqa: BLE001
                    continue
                entries.append(v)
        runs_with_trace += 1
        calls += len(entries)
        ok = [e for e in entries if e.get("outcome") == "code"]
        if ok and not [e for e in entries if e.get("outcome") != "code"]:
            first_try += 1
        for v in entries:
            outcomes[v.get("outcome")] += 1
            lat = v.get("latency_ms")
            if isinstance(lat, (int, float)):
                latencies.append(lat)
            fr = v.get("finish_reason")
            if isinstance(fr, str):
                finish[fr] += 1
            if v.get("error"):
                errors[str(v["error"])[:140]] += 1
            body = str(v.get("body", ""))
            m = re.search(r'"status":\s*(\d+)', body)
            if m:
                errors[f"HTTP {m.group(1)}"] += 1

    # Static Sync-bound scan: the round-7 finding (26/100 bundles held
    # Cell<...> tick state, all failing the compile gate with E0277).
    sync_risk = []
    for hnmod in sorted(glob.glob(os.path.join(d, "*", "*.hnmod"))):
        try:
            with zipfile.ZipFile(hnmod) as z:
                text = "".join(
                    z.read(n).decode("utf-8", errors="replace")
                    for n in z.namelist()
                    if n.endswith(".rs") and "engine/src/lib.rs" not in n
                )
        except Exception:  # noqa: BLE001
            continue
        m = UNSYNC_RE.search(text)
        if m:
            sync_risk.append(os.path.basename(os.path.dirname(hnmod)))

    def pct(x: int, total: int) -> str:
        return f"{100.0 * x / total:.1f}%" if total else "n/a"

    print("\n===== LLM SOAK SUMMARY =====")
    print(f"runs: {runs}   tick statuses: {dict(statuses)}")
    print(f"runs with traces: {runs_with_trace}   first-try: {first_try}")
    print(f"llm calls: {calls}   outcomes: {dict(outcomes)}")
    print(f"finish reasons: {dict(finish)}")
    print(f"Sync-risk bundles (Cell/RefCell/Rc/UnsafeCell): {len(sync_risk)}")
    for n in sync_risk[:25]:
        print(f"  {n}")
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
