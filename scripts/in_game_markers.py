#!/usr/bin/env python3
"""Extract the in-game observables of an emitted port (scripts/in_game_markers.py).

The in-game dispatch job boots the real game with a ported mod and wants to
assert that the game's tick loop really dispatches the mod's hooks. The
evidence channel is the mod's own Host calls:

- `host.broadcast(...)` — the pinned build writes every broadcast line to the
  server's log (`log::info!` in native_server.rs), so the literal a broadcast
  carries is greppable in `logs/latest.log`. These are the log-observable
  markers.
- `host.tell(uuid, ...)` — one player's chat HUD only; nothing lands in the
  log. Reported as HUD-only (evidence stays with the compile gate's dispatch
  smoke and the state-poll assertions).

This script scans the port output's emitted Rust sources for those calls and
writes the greppable prefixes (format strings cut at the first `{`) to
`--markers` (one per line) and a JSON summary to `--json`. The emitted code
lives inside the `.hnmod` bundle (a zip: `horizon-native.json` +
`mixins/<id>/...`), so the scan covers `.rs` members of every `*.hnmod` in
the output dir as well as any loose `*.rs` files.

Honesty contract: an empty marker set is honest ONLY after a real scan (a
genuinely tell-only body); a zero-source scan is an extraction error, not
evidence of anything, and this script exits nonzero rather than let a caller
mistake "scanned nothing" for "the body is tell-only". The caller decides
what to require beyond that.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import zipfile
from pathlib import Path

CALL_RE = re.compile(r"(?<!fn )\b(?:broadcast|tell)\s*\(")
# `(?<!fn )` skips the SmokeHost double's METHOD DEFINITIONS in the wrapper's
# cfg(test) smoke module ("fn broadcast(&self, ...)", "fn tell(&mut self,
# ...)") — otherwise the double's next literal (the smoke's "unknown panic"
# fallback) is mis-attributed to the body as a broadcast/tell message. Only
# call sites (host.broadcast(...), host.tell(...)) are the body's
# observables.
# A Rust string literal: no bare newlines, `\"` escapes; good enough for the
# emitted hook bodies the pipeline produces.
LITERAL_RE = re.compile(r'"((?:[^"\\\n]|\\.)*)"')


def unescape(lit: str) -> str:
    """Undo the Rust escapes that survive in emitted literals."""
    for esc, char in (("\\n", ""), ("\\t", ""), ('\\"', '"'), ("\\\\", "\\")):
        lit = lit.replace(esc, char)
    return lit


def stable_prefix(literal: str) -> str:
    """The greppable part of a literal: a format string is cut at the first
    `{` (placeholder), otherwise the whole literal stands."""
    return unescape(literal).split("{", 1)[0].strip()


def scan_text(text: str) -> tuple[list[str], list[str]]:
    """Return (broadcast_literals, tell_literals) raw, in source order."""
    broadcast: list[str] = []
    tell: list[str] = []
    for call in CALL_RE.finditer(text):
        kind = "broadcast" if text[call.start() :].startswith("broadcast") else "tell"
        # First string literal after the call's open paren: the message the
        # body sends (seen shapes: format!("..."), Text::from("..."), "..."
        # .into()).
        rest = text[call.end() : call.end() + 2000]
        literal = LITERAL_RE.search(rest)
        if not literal:
            continue
        (broadcast if kind == "broadcast" else tell).append(literal.group(1))
    return broadcast, tell


def collect_rs_sources(output: Path) -> list[tuple[str, str]]:
    """Every emitted .rs source as (label, text): loose files under `output`
    (as the port pipeline may stage), then `.rs` members inside `*.hnmod`
    bundles in `output` (the pipeline's actual layout: the emitted crate
    lives at mixins/<id>/ inside the bundle zip)."""
    sources: list[tuple[str, str]] = []
    for path in sorted(output.rglob("*.rs")):
        if "target" in path.parts:
            continue
        sources.append(
            (str(path.relative_to(output)), path.read_text(encoding="utf-8", errors="replace"))
        )
    for hnmod in sorted(output.rglob("*.hnmod")):
        with zipfile.ZipFile(hnmod) as z:
            for name in z.namelist():
                if name.endswith(".rs") and not name.endswith("/"):
                    sources.append(
                        (f"{hnmod.name}!/{name}", z.read(name).decode("utf-8", errors="replace"))
                    )
    return sources


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("output", type=Path, help="the port's output dir (emitted crate sources)")
    ap.add_argument("--markers", type=Path, help="write broadcast marker prefixes here, one per line")
    ap.add_argument("--json", dest="as_json", type=Path, help="write the JSON summary here")
    args = ap.parse_args()

    sources = collect_rs_sources(args.output)
    if not sources:
        print(
            f"no emitted Rust found in {args.output}: no loose .rs files and "
            "no .hnmod bundle — nothing was scanned, so no claim about the "
            "body's observables can be made (extraction error, not a "
            "tell-only body)",
            file=sys.stderr,
        )
        return 2
    broadcast: list[str] = []
    tell: list[str] = []
    for _, text in sources:
        b, t = scan_text(text)
        broadcast += b
        tell += t

    # Deduplicate, keep order; drop prefixes that carry no stable text.
    markers = []
    for lit in broadcast:
        prefix = stable_prefix(lit)
        if prefix and prefix not in markers:
            markers.append(prefix)

    summary = {
        "files_scanned": len(sources),
        "broadcast_calls": len(broadcast),
        "tell_calls": len(tell),
        "log_observable": bool(markers),
        "broadcast_markers": markers,
        "tell_literals": [stable_prefix(t) for t in tell],
    }
    if args.as_json:
        args.as_json.parent.mkdir(parents=True, exist_ok=True)
        args.as_json.write_text(json.dumps(summary, indent=1) + "\n", encoding="utf-8")
    if args.markers:
        args.markers.parent.mkdir(parents=True, exist_ok=True)
        args.markers.write_text("".join(m + "\n" for m in markers), encoding="utf-8")

    print(
        f"scanned {len(sources)} emitted Rust source(s): "
        f"{len(broadcast)} broadcast call(s), {len(tell)} tell call(s); "
        f"log-observable markers: {len(markers)}"
    )
    for m in markers:
        print(f"  broadcast marker: {m!r}")
    for t in summary["tell_literals"]:
        print(f"  tell (HUD-only) literal: {t!r}")
    if not markers and tell:
        print(
            "the real port's messages are tell-only (player chat HUD, not the "
            "server's log): dispatch evidence stays with the compile gate's "
            "dispatch smoke and the state-poll assertions"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
