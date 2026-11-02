#!/usr/bin/env python3
"""Download the 20-mod reference set from Modrinth (spec §8.1 modpack-sweep).

Jars are third-party works: they land in runner scratch space and are never
committed or redistributed."""
import argparse, json, os, sys, urllib.request

API = "https://api.modrinth.com/v2"
UA = {"User-Agent": "auto-porter/0.1 (fixture download)"}

def get(url):
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--count", type=int, default=20)
    ap.add_argument("--output", required=True)
    ap.add_argument("--game-version", default="26.3")
    args = ap.parse_args()

    os.makedirs(args.output, exist_ok=True)
    search = get(f"{API}/search?limit={args.count}"
                 f'&facets=[["project_type:mod"],["versions:{args.game_version}"]]')
    hits = search.get("hits", [])
    ok, failed = 0, 0
    for hit in hits:
        slug = hit["slug"]
        try:
            versions = get(f"{API}/project/{slug}/version?game_versions=[\"{args.game_version}\"]")
            if not versions:
                continue
            files = versions[0].get("files", [])
            primary = next((f for f in files if f.get("primary")), files[0] if files else None)
            if not primary:
                continue
            dest = os.path.join(args.output, f"{slug}.jar")
            urllib.request.urlretrieve(primary["url"], dest)
            ok += 1
        except Exception as e:  # noqa: BLE001
            print(f"warn: {slug}: {e}", file=sys.stderr)
            failed += 1
    print(json.dumps({"downloaded": ok, "failed": failed, "dir": args.output}))

if __name__ == "__main__":
    main()
