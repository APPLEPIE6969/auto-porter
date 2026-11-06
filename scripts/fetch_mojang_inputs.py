#!/usr/bin/env python3
"""Fetch the pinned Minecraft files Horizon's preparer needs, from Mojang's
public distribution endpoints — no account and no launcher required.

`horizon-prepare` verifies every input against the sha1 pins in
research-lock.json, but it never downloads anything itself ("Horizon never
downloads Mojang's files" is the EULA posture of its own launcher, which
downloads them for a signed-in owner). Every file a signed-in launcher would
hand it is publicly served by Mojang with no authentication:

  version JSON   https://piston-meta.mojang.com/v1/packages/<sha1>/<id>.json
  client jar     https://piston-data.mojang.com/v1/objects/<sha1>/client.jar
  server jar     https://piston-data.mojang.com/v1/objects/<sha1>/server.jar
  libraries      per-entry downloads.artifact.url (Maven layout)
  asset index    https://piston-meta.mojang.com/v1/packages/<sha1>/<id>.json
  assets         https://resources.download.minecraft.net/<2>/<sha1>

This script fetches the first four (the minimal, sound-less set the game
boots with) and verifies each file against the pins. Version-JSON
republishes change only the assetIndex, and the pinned package blob stays
fetchable by its content address, so the script downloads the PINNED blob
directly instead of whatever the live manifest currently points at.

Usage:
  fetch_mojang_inputs.py --lock RESEARCH_LOCK.json --out DIR [--jobs N]

Writes:
  DIR/version.json  DIR/client.jar  DIR/server.jar  DIR/libraries/<maven path>
"""

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import pathlib
import sys
import tempfile
import time
import urllib.error
import urllib.request

MANIFEST = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
CHUNK = 1 << 20
USER_AGENT = "auto-porter-offline-prepare/1.0"


def fail(message: str) -> None:
    print(f"fetch_mojang_inputs: error: {message}", file=sys.stderr)
    sys.exit(1)


def sha1_of(path: pathlib.Path) -> str:
    digest = hashlib.sha1()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(CHUNK), b""):
            digest.update(block)
    return digest.hexdigest()


def fetch(url: str, dest: pathlib.Path, sha1: str | None, tries: int = 4) -> tuple[str, int]:
    """Stream `url` to `dest` atomically, verifying `sha1` when given.

    Returns (kind, bytes) for the summary line; raises on exhaustion.
    """
    last: Exception | None = None
    for attempt in range(1, tries + 1):
        try:
            request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
            with urllib.request.urlopen(request, timeout=120) as response:
                if response.status != 200:
                    raise urllib.error.HTTPError(url, response.status, "not OK", None, None)
                dest.parent.mkdir(parents=True, exist_ok=True)
                with tempfile.NamedTemporaryFile(
                    dir=dest.parent, prefix=f".{dest.name}.", delete=False
                ) as tmp:
                    size = 0
                    while block := response.read(CHUNK):
                        tmp.write(block)
                        size += len(block)
                    tmp_path = pathlib.Path(tmp.name)
            if sha1 is not None:
                got = sha1_of(tmp_path)
                if got != sha1:
                    tmp_path.unlink(missing_ok=True)
                    raise ValueError(f"sha1 mismatch: got {got}, want {sha1}")
            dest.parent.mkdir(parents=True, exist_ok=True)
            tmp_path.replace(dest)
            return ("pinned" if sha1 else "plain", size)
        except Exception as error:  # retry any failure: 429s and resets happen
            last = error
            if attempt < tries:
                time.sleep(2 * attempt)
    raise RuntimeError(f"{url}: {last}") from last


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--lock", required=True, type=pathlib.Path, help="research-lock.json")
    parser.add_argument("--out", required=True, type=pathlib.Path, help="output directory")
    parser.add_argument("--jobs", type=int, default=8, help="parallel downloads")
    args = parser.parse_args()

    lock = json.loads(args.lock.read_text())
    minecraft = lock.get("minecraft") or fail("research-lock.json has no [minecraft]")
    version = minecraft.get("version") or fail("no pinned minecraft version")
    manifest_sha1 = minecraft.get("manifest_sha1") or fail("no pinned manifest_sha1")
    accepted = set(minecraft.get("accepted_manifest_sha1", []))
    client_sha1 = minecraft.get("client_sha1") or fail("no pinned client_sha1")
    server_sha1 = minecraft.get("server_sha1") or fail("no pinned server_sha1")
    args.out.mkdir(parents=True, exist_ok=True)

    # 1. The PINNED version JSON by its content address (stable against
    #    republishes); fall back to the live manifest, accepting the pin or
    #    any republish the lock lists.
    version_json = args.out / "version.json"
    pinned_url = f"https://piston-meta.mojang.com/v1/packages/{manifest_sha1}/{version}.json"
    try:
        fetch(pinned_url, version_json, manifest_sha1)
    except RuntimeError as first:
        print(f"pinned blob unavailable ({first}); falling back to the live manifest")
        try:
            with urllib.request.urlopen(
                urllib.request.Request(MANIFEST, headers={"User-Agent": USER_AGENT}), timeout=60
            ) as response:
                entries = json.loads(response.read())["versions"]
        except Exception as error:
            fail(f"cannot read the version manifest: {error}")
        match = next((e for e in entries if e["id"] == version), None)
        if match is None:
            fail(f"{version} is not in the version manifest")
        fetch(match["url"], version_json, None)
        got = sha1_of(version_json)
        if got != manifest_sha1 and got not in accepted:
            fail(f"live {version}.json sha1 {got} is neither the pin nor an accepted republish")

    parsed = json.loads(version_json.read_text())
    downloads = parsed.get("downloads") or fail("version JSON has no downloads")
    client = downloads.get("client") or fail("version JSON has no client download")
    server = downloads.get("server") or fail("version JSON has no server download")
    if client.get("sha1") != client_sha1:
        fail(f"version JSON's client sha1 {client.get('sha1')} != pin {client_sha1}")
    if server.get("sha1") != server_sha1:
        fail(f"version JSON's server sha1 {server.get('sha1')} != pin {server_sha1}")

    jobs: list[tuple[str, pathlib.Path, str | None, str]] = [
        (client["url"], args.out / "client.jar", client_sha1, "client.jar"),
        (server["url"], args.out / "server.jar", server_sha1, "server.jar"),
    ]
    skipped: list[str] = []
    for library in parsed.get("libraries", []):
        artifact = (library.get("downloads") or {}).get("artifact") or {}
        path, url = artifact.get("path"), artifact.get("url")
        if not (path and url):
            skipped.append(library.get("name", "?"))
            continue
        jobs.append((url, args.out / "libraries" / path, artifact.get("sha1"), path))

    print(f"{version}: {len(jobs)} downloads (client, server, {len(jobs) - 2} libraries)")
    failures: list[str] = []
    total = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
        futures = {pool.submit(fetch, url, dest, sha1): name for url, dest, sha1, name in jobs}
        for future in concurrent.futures.as_completed(futures):
            name = futures[future]
            try:
                _, size = future.result()
                total += size
            except Exception as error:
                failures.append(f"{name}: {error}")
                print(f"  FAILED {name}: {error}", file=sys.stderr)
    if skipped:
        print(f"skipped {len(skipped)} libraries without a downloadable artifact: {skipped}")
    if failures:
        fail(f"{len(failures)} of {len(jobs)} downloads failed")
    print(f"done: {len(jobs)} files, {total / 1e6:.1f} MB, all sha1 pins verified")


if __name__ == "__main__":
    main()
