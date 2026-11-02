#!/usr/bin/env bash
# End-to-end test against REAL mods pulled from Modrinth (pinned URLs).
#
# The committed fixtures are small synthetic jars; these two real mods cover
# shapes they cannot: a Balm-wrapped event surface (Waystones — no
# @SubscribeEvent anywhere) and a large direct-API surface (Tombstone — 590
# classes, phase-split events, tick events, command registration).
#
# Nothing third-party is committed: jars are downloaded to a temp dir, and the
# assertions check pipeline outcomes, not content. Third-party jars stay out
# of the repo (license + size).
#
# Usage: scripts/test_real_mods.sh   (network required; cargo on PATH)
set -euo pipefail

cd "$(dirname "$0")/.."

UA="auto-porter-e2e/0.1"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fetch() { # name url
    echo "== fetching $1"
    curl -sS --max-time 180 -L -H "User-Agent: $UA" -o "$WORK/$1" "$2"
}

fail() { echo "FAIL: $*" >&2; exit 1; }

# --- Waystones (Balm wrapper library) --------------------------------------
WAYSTONES_URL="https://cdn.modrinth.com/data/LOpKHB2A/versions/6Z6MQ6os/waystones-neoforge-1.21.1-21.1.46.jar"
fetch waystones.jar "$WAYSTONES_URL"

# --- Corail Tombstone (direct NeoForge API, 590 classes) -------------------
TOMBSTONE_URL="https://cdn.modrinth.com/data/YT5K34AB/versions/e68NhQJm/tombstone-neoforge-1.21.1-9.5.6.jar"
fetch tombstone.jar "$TOMBSTONE_URL"

run() { cargo run --release -q -p cli -- "$@"; }

# --- Waystones: Balm events must be detected and mapped --------------------
run analyze "$WORK/waystones.jar" --output "$WORK/waystones-analyze" >/dev/null
python3 - "$WORK/waystones-analyze/classified.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
plan = d["plan"]
tiers = plan["tier_summary"]
assert tiers["B"] >= 10, f"Balm damage/death/use-block events unmapped: {tiers}"
assert tiers["D"] >= 10, f"unmapped Balm events must yield honest gaps: {tiers}"
hooks = {f["hook"]["name"] for f in plan["features"] if f["hook"]}
assert {"living_incoming_damage", "living_death", "right_click_block"} <= hooks, hooks
assert len(d["gaps"]) == tiers["D"], "Tier D features must each carry a GapReport (spec §3)"
print("waystones: tiers", tiers, "- Balm detection OK")
PY

# --- Tombstone: phase-split events + the LLM path --------------------------
run analyze "$WORK/tombstone.jar" --output "$WORK/tombstone-analyze" >/dev/null
python3 - "$WORK/tombstone-analyze/classified.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
plan = d["plan"]
feats = plan["features"]
# LivingDamageEvent$Pre (NeoForge 1.21.1 phase split) maps to the damage hook
dam = [f for f in feats if f["id"].endswith("livingdamageevent-pre-onlivingdamagepre")]
assert dam and dam[0]["hook"] and dam[0]["hook"]["name"] == "living_incoming_damage", \
    "LivingDamageEvent$Pre must map to living_incoming_damage"
# LeftClickBlock resolves left_click_block but has no template -> spec §10 LLM
llm = [f for f in feats if "leftclickblock" in f["id"]]
assert llm and llm[0]["hook"] and llm[0]["hook"]["name"] == "left_click_block"
assert llm[0]["template"] is None, "left-click must route to the LLM fallback"
assert plan["tier_summary"]["D"] >= 40, "tick/lifecycle events must be honest gaps"
print("tombstone: tiers", plan["tier_summary"], "- LLM-path + phase mapping OK")
PY

# --- Full port + static verify on both -------------------------------------
run port "$WORK/waystones.jar" --output "$WORK/waystones-port" >/dev/null
v=$(run verify "$WORK/waystones-port" --static)
v="${v%%$'\n'*}"
case "$v" in *"overall=PASS_WITH_WARNINGS"*) ;; *) fail "waystones verify: $v" ;; esac
echo "waystones verify: $v"

run port "$WORK/tombstone.jar" --output "$WORK/tombstone-port" >/dev/null
v=$(run verify "$WORK/tombstone-port" --static)
v="${v%%$'\n'*}"
case "$v" in *"overall=PASS_WITH_WARNINGS"*) ;; *) fail "tombstone verify: $v" ;; esac
echo "tombstone verify: $v"

# The LLM-path feature must report an honest llm_* error (unavailable without
# a key, llm_failed if the key is rejected) — never a silent skip.
python3 - "$WORK/tombstone-port/tombstone-report.json" <<'PY'
import json, sys
r = json.load(open(sys.argv[1]))
errs = [e for e in r["errors"]
        if e.get("feature") == "tombstone:event-playerinteractevent-leftclickblock-onleftclickblock"
        and str(e.get("error_type", "")).startswith("llm_")]
assert errs, "left-click feature must attempt the LLM fallback and report llm_*"
print("tombstone LLM path:", errs[0]["error_type"], "-", errs[0]["message"][:90])
PY

echo
echo "real-mod e2e: OK (waystones + tombstone)"
