#!/usr/bin/env python3
"""Nightly ecosystem sweep downloader (spec §8.2).

Modrinth works from the public API. CurseForge requires an API key
(CURSEFORGE_API_KEY); without one the CurseForge source is skipped and the
report says so."""
import argparse, json, os, sys, urllib.request

UA = {"User-Agent": "auto-porter/0.1 (ecosystem sweep)"}

def get_json(url, headers=None):
    req = urllib.request.Request(url, headers={**UA, **(headers or {})})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sources", default="modrinth")
    ap.add_argument("--count", type=int, default=100)
    ap.add_argument("--output", required=True)
    ap.add_argument("--game-version", default="26.3")
    args = ap.parse_args()

    os.makedirs(args.output, exist_ok=True)
    downloaded = 0
    notes = []

    if "modrinth" in args.sources:
        try:
            search = get_json(
                f"https://api.modrinth.com/v2/search?limit={args.count}"
                f'&facets=[["project_type:mod"],["versions:{args.game_version}"]]')
            for hit in search.get("hits", []):
                slug = hit["slug"]
                try:
                    versions = get_json(
                        f"https://api.modrinth.com/v2/project/{slug}/version"
                        f'?game_versions=["{args.game_version}"]')
                    if not versions:
                        continue
                    files = versions[0].get("files", [])
                    primary = next((f for f in files if f.get("primary")), files[0] if files else None)
                    if primary:
                        urllib.request.urlretrieve(primary["url"], os.path.join(args.output, f"{slug}.jar"))
                        downloaded += 1
                except Exception as e:  # noqa: BLE001
                    notes.append(f"modrinth:{slug}: {e}")
        except Exception as e:  # noqa: BLE001
            notes.append(f"modrinth search failed: {e}")

    if "curseforge" in args.sources:
        key = os.environ.get("CURSEFORGE_API_KEY", "")
        if not key:
            notes.append("curseforge skipped: CURSEFORGE_API_KEY not set (the CurseForge API requires a key)")
        else:
            notes.append("curseforge: implement via https://api.curseforge.com/v1/mods/search with the provided key")

    print(json.dumps({"downloaded": downloaded, "notes": notes, "dir": args.output}, indent=2))

if __name__ == "__main__":
    main()
