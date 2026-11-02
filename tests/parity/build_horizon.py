#!/usr/bin/env python3
"""Build Horizon (client + horizon-ctl) at a pinned revision (spec §6, §8).

Used by the self-hosted `parity-tests` / `in-game-tests` / `modpack-sweep`
jobs and by `nightly-ecosystem.yml` in full-verify mode. Produces:

    <out>/horizon-ctl       the control CLI the harnesses drive
    <out>/horizon-client    the game client (when built)
    <out>/revision.txt      the full 40-hex SHA actually built

This script needs a machine that can build Horizon (Rust toolchain plus
Horizon's system dependencies). It never ships binaries and never patches
Horizon source: what is at the pinned revision upstream is what is built.
"""
import argparse
import subprocess
import sys
import urllib.request
from pathlib import Path

HORIZON_GIT = "https://github.com/horizon-rust/horizon.git"


def run(cmd, **kw):
    print(f"+ {' '.join(cmd)}", flush=True)
    return subprocess.run(cmd, check=True, **kw)


def full_sha(repo: Path, rev: str) -> str:
    out = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", rev],
        check=True, capture_output=True, text=True,
    ).stdout.strip()
    if len(out) != 40 or any(c not in "0123456789abcdef" for c in out):
        sys.exit(f"error: {rev!r} did not resolve to a full 40-hex SHA (got {out!r})")
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--rev", required=True,
                    help="branch name, tag or commit SHA to build")
    ap.add_argument("--out", required=True, help="output directory")
    ap.add_argument("--client", action="store_true", default=True,
                    help="also build the client (default)")
    args = ap.parse_args()

    out = Path(args.out)
    src = out / "src"
    out.mkdir(parents=True, exist_ok=True)

    # ---- fetch / refresh the source tree
    if (src / ".git").is_dir():
        run(["git", "-C", str(src), "fetch", "--all", "--tags"], stdout=sys.stdout)
        run(["git", "-C", str(src), "checkout", "--force", args.rev], stdout=sys.stdout)
    else:
        # Sanity-check the remote before the expensive clone.
        try:
            urllib.request.urlopen(HORIZON_GIT, timeout=30).read(1)
        except Exception as e:  # noqa: BLE001
            sys.exit(
                f"error: Horizon upstream {HORIZON_GIT} is not reachable ({e}). "
                "The parity/in-game layers cannot run without it; this is the "
                "honest failure, not a skip."
            )
        run(["git", "clone", HORIZON_GIT, str(src)], stdout=sys.stdout)
        run(["git", "-C", str(src), "checkout", args.rev], stdout=sys.stdout)

    sha = full_sha(src, args.rev)
    (out / "revision.txt").write_text(sha + "\n")
    print(f"building Horizon at {sha}")

    # ---- build; keep artifacts in <out> rather than the source tree
    target_dir = out.resolve() / "target"
    packages = ["horizon-ctl", "horizon-client"] if args.client else ["horizon-ctl"]
    env = {"CARGO_TARGET_DIR": str(target_dir)}
    run(["cargo", "build", "--release", *sum((["-p", p] for p in packages), [])],
        env={**dict(__import__("os").environ), **env}, stdout=sys.stdout)

    for pkg in packages:
        built = target_dir / "release" / pkg
        if not built.is_file():
            sys.exit(f"error: cargo built no {pkg} binary at {built}")
        link = out / pkg
        if link.exists() or link.is_symlink():
            link.unlink()
        built.rename(link)
        print(f"built {link}")

    print(f"done: {out}/horizon-ctl at revision {sha}")


if __name__ == "__main__":
    main()
