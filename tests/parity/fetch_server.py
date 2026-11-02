#!/usr/bin/env python3
"""Fetch the Java oracle server from Mojang's official version manifest.

The server jar is never committed (gitignored) and the EULA must be accepted
explicitly on the command line (spec §6.2).
"""
import argparse, json, sys, urllib.request

MANIFEST = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True, help="e.g. 26.3")
    ap.add_argument("--out", required=True)
    ap.add_argument("--accept-eula", action="store_true",
                    help="explicitly accept the Minecraft EULA for this scratch dir")
    args = ap.parse_args()
    if not args.accept_eula:
        sys.exit("refusing to prepare an oracle server without --accept-eula")

    try:
        with urllib.request.urlopen(MANIFEST, timeout=30) as r:
            manifest = json.load(r)
    except Exception as e:  # noqa: BLE001
        print(json.dumps({"error": f"version manifest unreachable: {e}"}))
        sys.exit(3)

    entry = next((v for v in manifest.get("versions", []) if v["id"] == args.version), None)
    if entry is None:
        print(json.dumps({"error": f"version {args.version} not in the Mojang manifest",
                          "oracle": "unavailable"}))
        sys.exit(3)

    with urllib.request.urlopen(entry["url"], timeout=30) as r:
        details = json.load(r)
    url = details.get("downloads", {}).get("server", {}).get("url")
    if not url:
        sys.exit(f"no server download for {args.version}")

    import os
    os.makedirs(args.out, exist_ok=True)
    jar_path = os.path.join(args.out, "server.jar")
    urllib.request.urlretrieve(url, jar_path)
    # EULA accepted explicitly by the caller
    with open(os.path.join(args.out, "eula.txt"), "w") as f:
        f.write("eula=true\n")
    with open(os.path.join(args.out, "server-info.json"), "w") as f:
        json.dump({"version": args.version, "source": url}, f, indent=2)
    print(f"oracle server {args.version} -> {jar_path}")

if __name__ == "__main__":
    main()
