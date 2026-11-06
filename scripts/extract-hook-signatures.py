#!/usr/bin/env python3
"""Extract the REAL `mod_api::Hooks` trait signatures from Horizon's source.

Reads the pinned revision recorded in hooks/native-hooks.json, fetches
`crates/horizon-client/src/mod_api/{generated.rs,generated_host.rs,mod_api.rs}`
from github.com/horizon-rust/horizon at that revision, parses the trait
methods, event structs and Host helpers, and writes hooks/hook-signatures.json.

Every hook in native-hooks.json must resolve to a trait method; the report
prints coverage so a Horizon bump cannot silently desync the table.
"""
import json
import re
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RAW = "https://raw.githubusercontent.com/horizon-rust/horizon/{rev}/{path}"

GENERATED = "crates/horizon-client/src/mod_api/generated.rs"
GENERATED_HOST = "crates/horizon-client/src/mod_api/generated_host.rs"
MOD_API = "crates/horizon-client/src/mod_api.rs"


def fetch(rev: str, path: str) -> str:
    url = RAW.format(rev=rev, path=path)
    with urllib.request.urlopen(url, timeout=60) as r:
        return r.read().decode("utf-8")


def strip_comments(block: str) -> str:
    return re.sub(r"//[^\n]*", "", block)


def balanced(src: str, start: int, open_ch: str, close_ch: str) -> int:
    """Index just past the balanced open_ch..close_ch region starting at start."""
    depth = 0
    for i in range(start, len(src)):
        c = src[i]
        if c == open_ch:
            depth += 1
        elif c == close_ch:
            depth -= 1
            if depth == 0:
                return i + 1
    return -1


def parse_trait_methods(src: str, trait_name: str) -> dict:
    """name -> {"signature": single-line normalized, "doc": str, "ret": str}"""
    m = re.search(r"pub trait %s\b[^{]*\{" % trait_name, src)
    if not m:
        raise SystemExit(f"trait {trait_name} not found")
    end = balanced(src, m.end() - 1, "{", "}")
    body = src[m.end() : end - 1]
    out = {}
    for fm in re.finditer(r"\n    fn ([a-z_0-9]+)\s*\(", body):
        name = fm.group(1)
        close = balanced(body, fm.end() - 1, "(", ")")
        if close < 0:
            continue
        rest = body[close : close + 40]
        ret = ""
        rm = re.match(r"\s*->\s*([A-Za-z_0-9:<>]+)", rest)
        if rm:
            ret = rm.group(1)
        params = re.sub(r"\s+", " ", body[fm.end() - 1 : close]).strip()
        # doc comment: contiguous `    ///` lines above
        head = body[: fm.start() + 1].rstrip("\n").split("\n")
        doc_lines = []
        for line in reversed(head):
            s = line.strip()
            if s.startswith("///"):
                doc_lines.append(s[3:].strip())
            elif s.startswith("#["):
                continue
            else:
                break
        doc = " ".join(reversed(doc_lines)).strip()
        out[name] = {
            "signature": f"fn {name}{params}" + (f" -> {ret}" if ret else ""),
            "doc": doc,
            "ret": ret,
        }
    return out


def parse_event_structs(src: str) -> dict:
    """EventName -> rendered field block (pub field: TYPE, ...)."""
    out = {}
    for sm in re.finditer(r"(#\[non_exhaustive\]\s*)?pub struct ([A-Za-z_0-9]+Event[^{]*)\{", src):
        name = re.sub(r"[^A-Za-z_0-9].*$", "", sm.group(2))
        end = balanced(src, sm.end() - 1, "{", "}")
        if end < 0:
            continue
        body = strip_comments(src[sm.end() : end - 1])
        fields = []
        for line in body.split("\n"):
            line = line.strip().rstrip(",")
            if line.startswith("pub ") and ":" in line:
                fields.append(re.sub(r"\s+", " ", line))
        out[name] = "\n".join(fields)
    return out


def parse_host_methods(src: str) -> dict:
    return parse_trait_methods(src, "Host")


def parse_dispatch_args(src: str) -> dict:
    """`dispatch_<name>` free functions -> normalized list of parameter strings.

    The hook POINTS in the game call these (never the trait methods
    directly: events are `#[non_exhaustive]`, so only `dispatch_*` can
    construct them). The arg list is what a dispatch-level smoke test must
    synthesize; only the `(host, mods)`-only shape is safely callable from
    outside without inventing field values.
    """
    out = {}
    for dm in re.finditer(r"\npub fn dispatch_([a-z_0-9]+)\s*\(", src):
        name = dm.group(1)
        close = balanced(src, dm.end() - 1, "(", ")")
        if close < 0:
            continue
        params = re.sub(r"\s+", " ", src[dm.end() : close - 1].strip())
        out[name] = [a for a in (p.strip() for p in params.split(",")) if a] if params else []
    return out


def main() -> int:
    catalog = json.loads((ROOT / "hooks" / "native-hooks.json").read_text())
    src_line = catalog.get("source", "")
    rev_m = re.search(r"revision ([0-9a-f]{40})", src_line)
    if not rev_m:
        raise SystemExit("cannot parse the pinned revision from native-hooks.json source")
    rev = rev_m.group(1)
    print(f"Horizon pinned revision: {rev}")

    generated = fetch(rev, GENERATED)
    host_src = fetch(rev, GENERATED_HOST)
    mod_api = fetch(rev, MOD_API)

    hooks = parse_trait_methods(generated, "Hooks")
    events = parse_event_structs(generated)
    host_methods = parse_host_methods(host_src)
    dispatch_args = parse_dispatch_args(generated)
    flow_m = re.search(r"pub enum Flow \{[^}]*\}", mod_api, re.S)
    flow_def = re.sub(r"\s+", " ", flow_m.group(0)) if flow_m else ""
    SMOKE_ARGS = ["host: &mut dyn Host", "mods: &[Box<dyn Hooks>]"]

    catalog_hooks = [h["name"] for h in catalog["hooks"]]
    missing = [h for h in catalog_hooks if h not in hooks]
    covered = [h for h in catalog_hooks if h in hooks]
    print(f"catalog hooks: {len(catalog_hooks)}  trait methods: {len(hooks)}")
    print(f"covered: {len(covered)}  missing from trait: {len(missing)}")
    if missing:
        print("missing:", ", ".join(missing[:20]), "..." if len(missing) > 20 else "")

    table = {
        "source": f"horizon-rust/horizon@{rev} {GENERATED}",
        "flow": flow_def,
        "hooks": {},
        "host_methods": {k: v["signature"] for k, v in sorted(host_methods.items())},
    }
    unmatched_events = set()
    for name in covered:
        sig = hooks[name]
        event_name = ""
        em = re.search(r"event: &mut ([A-Za-z_0-9]+)", sig["signature"])
        if em:
            event_name = em.group(1)
        if event_name and event_name not in events:
            unmatched_events.add(event_name)
        table["hooks"][name] = {
            "signature": sig["signature"],
            "returns_flow": sig["ret"] == "Flow",
            "doc": sig["doc"],
            "event": event_name,
            "event_fields": events.get(event_name, ""),
            "dispatch_args": dispatch_args.get(name, []),
        }
    smoke_class = [n for n in covered if table["hooks"][n]["dispatch_args"] == SMOKE_ARGS]
    print(f"dispatch smoke class (dispatch_(host, mods) only): {len(smoke_class)}")
    print("  ", ", ".join(sorted(smoke_class)))
    if unmatched_events:
        print("WARNING event structs not parsed:", ", ".join(sorted(unmatched_events)))

    out = ROOT / "hooks" / "hook-signatures.json"
    out.write_text(json.dumps(table, indent=1, sort_keys=True) + "\n")
    print(f"wrote {out} ({out.stat().st_size} bytes)")
    host_missing = [h["name"] for h in catalog.get("helpers", []) if h["name"] not in host_methods]
    print(f"host methods: {len(host_methods)}  catalog helpers missing from Host: {len(host_missing)}")
    if host_missing:
        print("missing helpers:", ", ".join(host_missing))
    return 0


if __name__ == "__main__":
    sys.exit(main())
