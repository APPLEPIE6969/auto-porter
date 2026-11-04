#!/usr/bin/env python3
"""Deep per-call analysis of an LLM-soak trace (AUTO_PORTER_LLM_TRACE JSONL).

For every recorded call this reconstructs what the model actually did wrong:
response shape (fences / prose / sentinel), signature conformance against the
exact contract in hooks/hook-signatures.json, API discipline (host.* calls
and bare identifiers vs the real Host methods), truncation (finish_reason),
and retry behaviour (attempts per run). Output: a taxonomy report to stdout
plus optionally a JSON file for diffing rounds.

Usage: analyze_soak.py <trace.jsonl> [--json out.json] [--save-calls dir]
"""
import argparse
import collections
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def load_signatures():
    try:
        return json.loads((ROOT / "hooks" / "hook-signatures.json").read_text())
    except Exception:
        return {"hooks": {}, "host_methods": {}}


def norm_ws(s: str) -> str:
    return " ".join(s.split())


def strip_pub(s: str) -> str:
    return s[4:] if s.startswith("pub ") else s


def param_types(params: str):
    out, depth, cur = [], 0, ""
    for c in params:
        if c in "(<[":
            depth += 1
        elif c in ")>]":
            depth -= 1
        elif c == "," and depth == 0:
            out.append(cur)
            cur = ""
            continue
        cur += c
    if cur.strip():
        out.append(cur)
    types = []
    for p in out:
        p = p.strip()
        t = p.rsplit(":", 1)[1].strip() if ":" in p else p
        types.append(t.replace("<'_>", "").strip())
    return types


def parse_sig(sig: str):
    s = norm_ws(strip_pub(sig.strip()))
    m = re.match(r"fn ([a-z_0-9]+)\s*\(", s)
    if not m:
        return None
    name = m.group(1)
    start = m.end() - 1
    depth = 0
    close = -1
    for i in range(start, len(s)):
        if s[i] == "(":
            depth += 1
        elif s[i] == ")":
            depth -= 1
            if depth == 0:
                close = i
                break
    if close < 0:
        return None
    params = param_types(s[start + 1 : close])
    rest = s[close + 1 :].strip()
    ret = rest[2:].strip() if rest.startswith("->") else ""
    return name, params, ret.replace("<'_>", "")


def check_signature(code: str, expected_sig: str):
    exp = parse_sig(expected_sig)
    if not exp:
        return "unknown"
    name, eparams, eret = exp
    hits = [m for m in re.finditer(rf"fn {name}\s*\(", code)]
    if not hits:
        # renamed or absent
        return "missing_fn"
    for m in hits:
        start = m.end() - 1
        depth, close = 0, -1
        for i in range(start, len(code)):
            if code[i] == "(":
                depth += 1
            elif code[i] == ")":
                depth -= 1
                if depth == 0:
                    close = i
                    break
        if close < 0:
            return "unbalanced"
        body = code[close + 1 :]
        brace = body.find("{")
        head = norm_ws(code[m.start() : close + 1] + (body[:brace] if brace >= 0 else ""))
        got = parse_sig(head.replace("fn ", "fn ", 1))
        if got and got[1] == eparams and got[2] == eret:
            return "ok"
    return "wrong_shape"


def check_api_discipline(code: str, host_methods):
    issues = []
    used = set(re.findall(r"(?:host|\bhost\b)?\.([a-z_0-9]+)\s*\(", code))
    # calls on the host binding specifically
    host_calls = set(re.findall(r"\bhost\s*\.\s*([a-z_0-9]+)\s*\(", code))
    known = set(host_methods)
    for c in sorted(host_calls - known):
        issues.append(f"host.{c} not a real Host method")
    return issues


def response_shape(raw: str):
    if not raw or not raw.strip():
        return "empty"
    if "```" in raw:
        return "fenced"
    if "PORT_INFEASIBLE" in raw and norm_ws(raw).strip() != "// PORT_INFEASIBLE":
        return "sentinel_mixed"
    if norm_ws(raw).strip() == "// PORT_INFEASIBLE":
        return "sentinel"
    first = raw.strip().splitlines()[0].strip()
    if re.match(r"^(use |pub struct|struct |impl |// !|//|#!)", first):
        return "clean"
    return "prose_wrapped"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("trace")
    ap.add_argument("--json", dest="out_json")
    ap.add_argument("--save-calls", dest="save_calls")
    args = ap.parse_args()
    sigs = load_signatures()

    shapes = collections.Counter()
    outcomes = collections.Counter()
    sig_results = collections.Counter()
    api_issues = collections.Counter()
    finish = collections.Counter()
    errors = collections.Counter()
    lat = []
    calls = []
    runs = collections.Counter()  # prompt-key -> attempt count
    per_run_calls = collections.defaultdict(list)

    for line in open(args.trace):
        line = line.strip()
        if not line:
            continue
        try:
            v = json.loads(line)
        except Exception:
            continue
        calls.append(v)
        outcomes[v.get("outcome")] += 1
        lat.append(v.get("latency_ms") or 0)
        fr = v.get("finish_reason")
        if isinstance(fr, str):
            finish[fr] += 1
        if v.get("error"):
            errors[norm_ws(str(v["error"]))[:110]] += 1
        raw = v.get("raw_text") or ""
        shapes[response_shape(raw)] += 1
        prompt = v.get("prompt") or ""
        m = re.search(r"Implement `([a-z_0-9]+)`", prompt)
        hook = m.group(1) if m else "?"
        key = f"{hook}:{abs(hash(norm_ws(prompt))) % 10_000_000}"
        per_run_calls[key].append(v)
        if v.get("outcome") == "code" and raw:
            exp = sigs.get("hooks", {}).get(hook, {}).get("signature")
            if exp:
                res = check_signature(raw, exp)
            else:
                res = "no_table"
            sig_results[res] += 1
            for issue in check_api_discipline(raw, sigs.get("host_methods", {})):
                api_issues[issue] += 1

    repaired = sum(1 for k, v in per_run_calls.items() if len(v) > 1)
    first_try = sum(1 for k, v in per_run_calls.items() if len(v) == 1 and v[0].get("outcome") == "code")

    def pct(x, total):
        return f"{100.0 * x / total:.1f}%" if total else "n/a"

    print("===== LLM SOAK DEEP ANALYSIS =====")
    print(f"calls: {len(calls)}   outcomes: {dict(outcomes)}")
    print(f"latency ms: p50={sorted(lat)[len(lat)//2] if lat else '-'} "
          f"p90={sorted(lat)[int(len(lat)*0.9)] if lat else '-'} max={max(lat) if lat else '-'}")
    print(f"response shapes: {dict(shapes)}")
    print(f"finish reasons: {dict(finish)}")
    print(f"signature conformance (code calls): {dict(sig_results)}")
    print(f"host-API discipline issues: {dict(api_issues) or 'none'}")
    print(f"runs (distinct prompts): {len(per_run_calls)}  "
          f"first-try ok: {first_try} ({pct(first_try, len(per_run_calls))})  "
          f"needed retry: {repaired} ({pct(repaired, len(per_run_calls))})")
    for e, c in errors.most_common(8):
        print(f"  error {c}x: {e}")
    if args.save_calls:
        out = Path(args.save_calls)
        out.mkdir(parents=True, exist_ok=True)
        for i, v in enumerate(calls):
            (out / f"call-{i:04d}.json").write_text(json.dumps(v, indent=1))
        print(f"saved per-call files under {out}/")
    if args.out_json:
        Path(args.out_json).write_text(json.dumps({
            "calls": len(calls),
            "outcomes": dict(outcomes),
            "shapes": dict(shapes),
            "finish": dict(finish),
            "signature": dict(sig_results),
            "api_issues": dict(api_issues),
            "errors": dict(errors),
            "runs": len(per_run_calls),
            "first_try_ok": first_try,
            "retried": repaired,
            "latency_p50": sorted(lat)[len(lat) // 2] if lat else None,
            "latency_p90": sorted(lat)[int(len(lat) * 0.9)] if lat else None,
        }, indent=1))
        print(f"wrote {args.out_json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
